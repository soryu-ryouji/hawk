//! 元数据存储：内存权威副本（含 path → hash 反查表）+ 可插拔的权威持久层（两种模式）。
//!
//! 模式（打开库时按内容探测，见 detect_storage_mode）：
//! - Toml（配置文件）：`.hawk/metadata/<hash>.toml` 为权威层——网盘同步友好（冲突粒度为单素材）。
//!   写入铁律：先 TOML 原子写（tmp+rename）成功后再写缓存与内存副本，中途崩溃自然朝 TOML 收敛。
//!   系统缓存目录的 index.db 是该模式的派生加速器（含文件夹快照/对账 mtime）。
//! - Db（数据库，新库默认）：`.hawk/metadata.db` 为权威层——同步写、批量单事务，无小文件 IO 开销。
//!   该模式不写 TOML、不做元数据对账（无外部可写源），文件夹快照等扫描簿记也存于 metadata.db；
//!   系统缓存的 index.db 不使用（存在即启动时删除，避免误读陈旧镜像）。
//!
//! 副本注水来源：Db 模式直接读 metadata.db；Toml 模式走 index.db 快路径 → TOML 全量解析回退。

use crate::core::index_db::IndexDb;
use crate::core::metadata::{self, ItemMetadata};
use crate::core::paths::{file_mtime_ms, LibraryPaths};
use crate::core::registry_file::atomic_write;
use crate::core::startup::StartupState;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 缓存写冲刷的批量阈值（单事务摊薄 fsync）
const CACHE_BATCH: usize = 256;
/// 缓存写冲刷的时间阈值：滞留超时即使未达批量也冲刷（限制缓存滞后窗口）
const CACHE_FLUSH_AFTER: Duration = Duration::from_millis(200);

/// 元数据权威层模式（打开库时探测，见 detect_storage_mode）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StorageMode {
    /// 数据库模式（.hawk/metadata.db，本地默认）
    Db,
    /// 配置文件模式（.hawk/metadata/*.toml，网盘同步友好）
    Toml,
}

/// 存储模式探测：先看标记文件（迁移的最后一步写入，权威声明）；无标记按内容探测——
/// metadata.db 存在（非空）→ Db；有 metadata/*.toml → Toml；都没有 → 新库默认 Db。
/// 两者皆存（迁移中断的窗口）：db 非空优先 Db（迁移到 db 是「写新删旧」，非空 db 是完整快照）；
/// 空 db 视为未完成迁移的残留 → Toml（调用方随后清理残留文件）
pub fn detect_storage_mode(paths: &LibraryPaths) -> StorageMode {
    if let Ok(text) = std::fs::read_to_string(&paths.storage_mode_file) {
        match text.trim() {
            "database" => return StorageMode::Db,
            "toml" => return StorageMode::Toml,
            _ => {}
        }
    }
    let db_exists = std::path::Path::new(&paths.metadata_db_file).is_file();
    let toml_exists = std::fs::read_dir(&paths.metadata_dir)
        .map(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().ends_with(".toml")))
        .unwrap_or(false);
    match (db_exists, toml_exists) {
        (true, true) => {
            let probe = IndexDb::open_authority(&paths.metadata_db_file);
            if probe.item_count() > 0 {
                StorageMode::Db
            } else {
                StorageMode::Toml
            }
        }
        (true, false) => StorageMode::Db,
        (false, true) => StorageMode::Toml,
        (false, false) => StorageMode::Db,
    }
}

pub struct MetadataStore {
    paths: LibraryPaths,
    db: IndexDb,
    mode: StorageMode,
    inner: Mutex<MetadataInner>,
}

#[derive(Default)]
struct MetadataInner {
    by_hash: HashMap<String, ItemMetadata>,
    hash_by_path: HashMap<String, String>,
    /// 待冲刷的 SQLite 缓存写（内存副本已即时更新）
    pending_db: Vec<(String, ItemMetadata, i64)>,
    pending_since: Option<Instant>,
}

impl MetadataStore {
    /// 构造：探测存储模式（或显式指定）→ 清理另一模式的残留文件 → 注水内存副本。
    /// Db 模式直接读 metadata.db；Toml 模式走缓存快路径 → TOML 全量解析回退（顺带建缓存）。
    /// TOML 回退解析期间经 StartupState 上报进度（每 1000 文件一帧，phase=sync），
    /// 大库首次启动（缓存缺失）时启动屏有实时反馈
    pub fn new(paths: LibraryPaths, startup: &StartupState) -> MetadataStore {
        Self::with_mode(paths, startup, None)
    }

    /// 显式指定模式（测试用）；None = 按 detect_storage_mode 探测
    pub fn with_mode(paths: LibraryPaths, startup: &StartupState, mode: Option<StorageMode>) -> MetadataStore {
        let mode = mode.unwrap_or_else(|| detect_storage_mode(&paths));
        // 清理另一模式的残留（迁移/切换中断的收尾；此刻两个 db 均未打开，删除无占用问题）
        match mode {
            StorageMode::Db => {
                // 系统缓存 index.db 在 Db 模式不使用：删除避免误读陈旧镜像
                if std::fs::remove_file(&paths.index_db_file).is_ok() {
                    tracing::info!("已删除缓存镜像（数据库模式不使用）: {}", paths.index_db_file);
                }
                // 迁移残留的 TOML（db 已是完整快照，残留只会干扰探测）
                if let Ok(rd) = std::fs::read_dir(&paths.metadata_dir) {
                    for entry in rd.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.ends_with(".toml") {
                            let _ = std::fs::remove_file(entry.path());
                        }
                    }
                }
            }
            StorageMode::Toml => {
                if std::fs::remove_file(&paths.metadata_db_file).is_ok() {
                    tracing::info!("已删除残留的 metadata.db（配置文件模式不使用）");
                }
            }
        }
        // 标记文件最后写：探测结果的权威声明（下次启动免于探测，迁移中断窗口由其定音）
        atomic_write(
            &paths.storage_mode_file,
            match mode {
                StorageMode::Db => "database\n",
                StorageMode::Toml => "toml\n",
            },
        );

        let db = match mode {
            StorageMode::Db => IndexDb::open_authority(&paths.metadata_db_file),
            StorageMode::Toml => IndexDb::open(&paths.index_db_file),
        };

        let mut entries: Option<Vec<(String, ItemMetadata, i64)>> = None;
        if mode == StorageMode::Toml && db.hydrated.load(std::sync::atomic::Ordering::SeqCst) {
            match db.load_all() {
                Ok(loaded) => {
                    tracing::info!("元数据副本已从 SQLite 缓存注水 {} 条", loaded.len());
                    entries = Some(loaded);
                }
                Err(e) => tracing::error!("元数据缓存读取失败，改由 TOML 全量解析: {e}"),
            }
        }

        let entries = match (mode, entries) {
            (StorageMode::Db, _) => match db.load_all() {
                Ok(loaded) => {
                    tracing::info!("元数据已从数据库加载 {} 条", loaded.len());
                    loaded
                }
                Err(e) => {
                    tracing::error!("元数据数据库读取失败，按空库启动: {e}");
                    Vec::new()
                }
            },
            (StorageMode::Toml, Some(e)) => e,
            (StorageMode::Toml, None) => {
                let from_toml = load_all_from_toml(&paths, startup);
                db.hydrate(&from_toml);
                from_toml
            }
        };

        let mut inner = MetadataInner::default();
        for (hash, meta, _) in entries {
            inner.by_hash.insert(hash.clone(), meta.clone());
            for p in &meta.paths {
                inner.hash_by_path.insert(p.path.clone(), hash.clone());
            }
        }
        MetadataStore {
            paths,
            db,
            mode,
            inner: Mutex::new(inner),
        }
    }

    /// 当前权威层模式（探测/显式指定结果）
    pub fn mode(&self) -> StorageMode {
        self.mode
    }

    pub fn try_get(&self, hash: &str) -> Option<ItemMetadata> {
        self.inner.lock().unwrap().by_hash.get(hash).cloned()
    }

    /// 全部元数据条目快照（批量迁移用）
    pub fn snapshot(&self) -> Vec<(String, ItemMetadata)> {
        self.inner.lock().unwrap().by_hash.iter().map(|(h, m)| (h.clone(), m.clone())).collect()
    }

    /// 按库内路径反查所属内容哈希（元数据 paths 记录）
    pub fn find_hash_by_path(&self, library_path: &str) -> Option<String> {
        self.inner.lock().unwrap().hash_by_path.get(library_path).cloned()
    }

    /// 调色板缺失（未提炼）的 hash 列表——派生缓存的自愈依据。
    /// palette 缺失是「缩略图/调色板派生工作未完成」的可靠信号（两者由同一 worker 任务补齐），
    /// 纯内存扫描，不碰文件系统
    pub fn hashes_with_missing_palette(&self) -> Vec<String> {
        self.inner
            .lock()
            .unwrap()
            .by_hash
            .iter()
            .filter(|(_, m)| m.palette.is_none())
            .map(|(h, _)| h.clone())
            .collect()
    }

    /// 宽高缺失（入库时解码暂时失败的遗留）的 hash 列表——周期对账宽高自愈的依据。
    /// 纯内存扫描，不碰文件系统
    pub fn hashes_with_zero_dim(&self) -> Vec<String> {
        self.inner
            .lock()
            .unwrap()
            .by_hash
            .iter()
            .filter(|(_, m)| m.width == 0 || m.height == 0)
            .map(|(h, _)| h.clone())
            .collect()
    }

    /// 全量文件夹快照（增量扫描的对比基准；缓存不可用时返回空表 = 首轮全量深入）
    pub fn load_folder_snapshots(&self) -> HashMap<String, (i64, i64)> {
        self.db.load_folder_snapshots()
    }

    /// 整体替换文件夹快照（每轮扫描一次；遍历不完整时不调用）。
    /// 先冲刷缓冲：快照替换标记「本轮已收敛」，缓存滞后于快照会在下次启动时多对账一轮
    pub fn replace_folder_snapshots(&self, snapshots: &HashMap<String, (i64, i64)>) {
        self.flush_cache();
        self.db.replace_folder_snapshots(snapshots);
    }

    /// 各 TOML 源文件 mtime 快照（后台对账比对依据）；缓存不可用时返回 None（本轮对账跳过）
    pub fn source_mtimes(&self) -> Option<HashMap<String, i64>> {
        self.db.load_source_mtimes()
    }

    /// 保存元数据：权威层先写（Toml=原子写小文件，Db=strict 单事务），成功后更新内存副本；
    /// Toml 模式的缓存写进待冲刷缓冲（达批量/时限由消费循环或 flush_cache 冲刷）。
    /// 权威层写失败返回 Err（调用方决定回传 API 或仅记录）
    pub fn save(&self, hash: &str, meta: &ItemMetadata) -> Result<(), String> {
        if self.mode == StorageMode::Db {
            self.db.save_strict(hash, meta)?;
            let mut inner = self.inner.lock().unwrap();
            inner.by_hash.insert(hash.to_string(), meta.clone());
            rebuild_path_index(&mut inner, hash, meta);
            return Ok(());
        }
        let source_mtime = self.save_toml(hash, meta)?;
        let mut inner = self.inner.lock().unwrap();
        inner.by_hash.insert(hash.to_string(), meta.clone());
        rebuild_path_index(&mut inner, hash, meta);
        if inner.pending_db.is_empty() {
            inner.pending_since = Some(Instant::now());
        }
        let entry = (hash.to_string(), meta.clone(), source_mtime);
        match inner.pending_db.iter_mut().find(|(h, _, _)| h == hash) {
            Some(slot) => *slot = entry,
            None => inner.pending_db.push(entry),
        }
        let due = inner.pending_db.len() >= CACHE_BATCH
            || inner
                .pending_since
                .is_some_and(|t| t.elapsed() >= CACHE_FLUSH_AFTER);
        let batch = if due { std::mem::take(&mut inner.pending_db) } else { Vec::new() };
        inner.pending_since = if due { None } else { inner.pending_since };
        drop(inner);
        if !batch.is_empty() {
            self.db.save_batch(&batch);
        }
        Ok(())
    }

    /// 取出待冲刷缓冲（锁内取值，锁外落盘）
    fn take_pending(&self) -> Vec<(String, ItemMetadata, i64)> {
        let mut inner = self.inner.lock().unwrap();
        inner.pending_since = None;
        std::mem::take(&mut inner.pending_db)
    }

    /// 立即冲刷待冲刷的缓存写（单事务）。Db 模式无缓存缓冲，恒无动作
    pub fn flush_cache(&self) {
        if self.mode == StorageMode::Db {
            return;
        }
        let batch = self.take_pending();
        if !batch.is_empty() {
            self.db.save_batch(&batch);
        }
    }

    /// 时间阈值检查冲刷（消费循环每任务后调用，限制安静期的缓存滞后）
    pub fn maybe_flush_cache(&self) {
        if self.mode == StorageMode::Db {
            return;
        }
        let due = {
            let inner = self.inner.lock().unwrap();
            !inner.pending_db.is_empty()
                && inner
                    .pending_since
                    .is_some_and(|t| t.elapsed() >= CACHE_FLUSH_AFTER)
        };
        if due {
            self.flush_cache();
        }
    }

    /// 仅写 TOML（原子写，权威层），返回源文件 mtime。批量回写路径用：
    /// 先逐条落 TOML（铁律：TOML 先行），再统一刷内存与 SQLite 单事务
    pub fn save_toml(&self, hash: &str, meta: &ItemMetadata) -> Result<i64, String> {
        write_toml_file(&self.paths, hash, meta)
    }

    /// 权威层批量持久化（两模式分发）：Db → 单事务 strict 写（mtime 记 0，无 TOML 源文件可言）；
    /// Toml → 并行小文件落盘。返回（成功项含 mtime，失败 hash 列表）
    pub fn persist_batch(&self, entries: &[(String, ItemMetadata)]) -> (Vec<(String, ItemMetadata, i64)>, Vec<String>) {
        match self.mode {
            StorageMode::Db => match self.db.save_batch_strict(entries) {
                Ok(()) => (entries.iter().map(|(h, m)| (h.clone(), m.clone(), 0)).collect(), Vec::new()),
                Err(e) => {
                    tracing::error!("元数据数据库批量写入失败: {e}");
                    (Vec::new(), entries.iter().map(|(h, _)| h.clone()).collect())
                }
            },
            StorageMode::Toml => write_toml_batch(&self.paths, entries),
        }
    }


    /// 批量应用内存副本。Toml 模式：先冲刷待冲刷缓冲再写缓存单事务（避免旧值覆盖新值）；
    /// Db 模式：权威层已由 persist_batch 写完，此处只刷内存
    pub fn apply_batch(&self, entries: &[(String, ItemMetadata, i64)]) {
        if entries.is_empty() {
            return;
        }
        if self.mode == StorageMode::Toml {
            self.flush_cache();
        }
        {
            let mut inner = self.inner.lock().unwrap();
            for (hash, meta, _) in entries {
                inner.by_hash.insert(hash.clone(), meta.clone());
                rebuild_path_index(&mut inner, hash, meta);
            }
        }
        if self.mode == StorageMode::Toml {
            self.db.save_batch(entries);
        }
    }

    pub fn delete(&self, hash: &str) {
        if self.mode == StorageMode::Db {
            // 权威层先行：删除成功才动内存副本
            if let Err(e) = self.db.delete_strict(hash) {
                tracing::error!("{e}，内存副本保留");
                return;
            }
            let mut inner = self.inner.lock().unwrap();
            if let Some(meta) = inner.by_hash.remove(hash) {
                for p in &meta.paths {
                    if inner.hash_by_path.get(&p.path).map(|h| h == hash).unwrap_or(false) {
                        inner.hash_by_path.remove(&p.path);
                    }
                }
            }
            return;
        }
        // 先冲刷缓冲：缓冲中该 hash 的待写项会在 db.delete 后复活行，先落盘再删
        self.flush_cache();
        let file = self.file_path(hash);
        if std::path::Path::new(&file).is_file() {
            let _ = std::fs::remove_file(&file);
        }
        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(meta) = inner.by_hash.remove(hash) {
                for p in &meta.paths {
                    if inner.hash_by_path.get(&p.path).map(|h| h == hash).unwrap_or(false) {
                        inner.hash_by_path.remove(&p.path);
                    }
                }
            }
        }
        self.db.delete(hash);
    }

    /// 对账应用（只进不出）：TOML 被外部新增/修改后载入。仅 Toml 模式有意义（Db 模式无对账）
    /// 解析失败返回 false（跳过该文件，不清空任何状态，下一轮对账重试）
    pub fn apply_external_toml(&self, hash: &str, file: &str, source_mtime: i64) -> bool {
        if self.mode == StorageMode::Db {
            return false;
        }
        let text = match std::fs::read_to_string(file) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("元数据读取失败，对账跳过: {file}: {e}");
                return false;
            }
        };
        let meta = match metadata::parse(&text) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("元数据解析失败，对账跳过: {file}: {e}");
                return false;
            }
        };
        {
            let mut inner = self.inner.lock().unwrap();
            inner.by_hash.insert(hash.to_string(), meta.clone());
            rebuild_path_index(&mut inner, hash, &meta);
            // 待冲刷缓冲里的旧值不得覆盖对账载入的新值：移除同 hash 待写项
            inner.pending_db.retain(|(h, _, _)| h != hash);
        }
        self.db.save(hash, &meta, source_mtime);
        true
    }

    /// 对账应用（只进不出）：TOML 已消失 → 清空素材参数字段（等价于重启后无元数据的语义）。
    /// 仅 Toml 模式有意义
    pub fn clear_external(&self, hash: &str) {
        if self.mode == StorageMode::Db {
            return;
        }
        let meta = {
            let mut inner = self.inner.lock().unwrap();
            match inner.by_hash.get_mut(hash) {
                Some(m) => {
                    m.url = None;
                    m.star = 0;
                    m.annotation = None;
                    m.tags.clear();
                    m.categories.clear();
                    m.paths.clear();
                    m.clone()
                }
                None => return,
            }
        };
        // 待冲刷缓冲里的旧值不得覆盖清空结果：移除同 hash 待写项
        {
            let mut inner = self.inner.lock().unwrap();
            inner.pending_db.retain(|(h, _, _)| h != hash);
        }
        self.db.save(hash, &meta, 0);
    }

    fn file_path(&self, hash: &str) -> String {
        format!("{}/{}.toml", self.paths.metadata_dir, hash)
    }
}

fn rebuild_path_index(inner: &mut MetadataInner, hash: &str, meta: &ItemMetadata) {
    inner.hash_by_path.retain(|_, h| h != hash);
    for p in &meta.paths {
        inner.hash_by_path.insert(p.path.clone(), hash.to_string());
    }
}

/// 单条 TOML 原子写（tmp+rename），返回源文件 mtime
fn write_toml_file(paths: &LibraryPaths, hash: &str, meta: &ItemMetadata) -> Result<i64, String> {
    let file = format!("{}/{hash}.toml", paths.metadata_dir);
    let tmp = format!("{file}.tmp");
    std::fs::write(&tmp, metadata::serialize(meta)).map_err(|e| format!("元数据 TOML 写入失败 {tmp}: {e}"))?;
    if let Err(e) = std::fs::rename(&tmp, &file) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("元数据 TOML 替换失败 {file}: {e}"));
    }
    Ok(file_mtime_ms(&file))
}

/// 批量 TOML 落盘：大批量时多线程并行（不同 hash 不同文件，写入互相独立）。
/// 单条失败不中断整批：失败 hash 记入返回值第二分量并 tracing 记录
fn write_toml_batch(paths: &LibraryPaths, entries: &[(String, ItemMetadata)]) -> (Vec<(String, ItemMetadata, i64)>, Vec<String>) {
    /// 低于该阈值走串行（线程开销不值得）
    const PARALLEL_THRESHOLD: usize = 64;
    /// 写线程上限（机械盘/杀软环境下更多线程无收益）
    const MAX_WRITE_THREADS: usize = 8;

    fn write_chunk(
        paths: &LibraryPaths,
        chunk: &[(String, ItemMetadata)],
    ) -> (Vec<(String, ItemMetadata, i64)>, Vec<String>) {
        let mut ok = Vec::new();
        let mut failed = Vec::new();
        for (hash, meta) in chunk {
            match write_toml_file(paths, hash, meta) {
                Ok(mtime) => ok.push((hash.clone(), meta.clone(), mtime)),
                Err(e) => {
                    tracing::warn!("批量元数据落盘失败: {hash}: {e}");
                    failed.push(hash.clone());
                }
            }
        }
        (ok, failed)
    }

    if entries.len() < PARALLEL_THRESHOLD {
        return write_chunk(paths, entries);
    }

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(MAX_WRITE_THREADS);
    let chunk_size = entries.len().div_ceil(threads);
    std::thread::scope(|s| {
        let handles: Vec<_> = entries
            .chunks(chunk_size)
            .map(|chunk| s.spawn(|| write_chunk(paths, chunk)))
            .collect();
        let mut ok = Vec::new();
        let mut failed = Vec::new();
        for h in handles {
            let (mut o, mut f) = h.join().expect("批量落盘线程 panic");
            ok.append(&mut o);
            failed.append(&mut f);
        }
        (ok, failed)
    })
}

/// 迁移到目标存储模式（调用方须保证无并发写——在流水线单写者 Job 内执行）。
/// 全量副本写入新权威层 → 删除旧权威层文件（进程占用删不掉的留给启动清理兜底）。
/// 成功后新模式在下次启动探测时生效（调用方负责引导重启）
pub fn migrate_authority(
    paths: &LibraryPaths,
    snapshot: &[(String, ItemMetadata)],
    target: StorageMode,
) -> Result<(), String> {
    match target {
        StorageMode::Db => {
            let db = IndexDb::open_authority(&paths.metadata_db_file);
            let entries: Vec<(String, ItemMetadata)> = snapshot.to_vec();
            db.save_batch_strict(&entries)?;
            drop(db);
            // 旧 TOML 删除（失败不中断：残留由 Db 模式启动清理兜底）
            if let Ok(rd) = std::fs::read_dir(&paths.metadata_dir) {
                for entry in rd.flatten() {
                    if entry.file_name().to_string_lossy().ends_with(".toml") {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
        StorageMode::Toml => {
            let _ = std::fs::create_dir_all(&paths.metadata_dir);
            let (_ok, failed) = write_toml_batch(paths, snapshot);
            if !failed.is_empty() {
                return Err(format!("{} 个条目的 TOML 写入失败", failed.len()));
            }
            // metadata.db 被当前进程持有时删除可能失败（Windows）：标记文件 + 启动清理兜底
            let _ = std::fs::remove_file(&paths.metadata_db_file);
        }
    }
    // 标记文件是迁移的最后一步：探测时优先于文件存在性（解决旧文件删不掉的模式翻转问题）
    atomic_write(
        &paths.storage_mode_file,
        match target {
            StorageMode::Db => "database\n",
            StorageMode::Toml => "toml\n",
        },
    );
    Ok(())
}

/// TOML 全量解析（缓存缺失时的权威回退路径）
fn load_all_from_toml(paths: &LibraryPaths, startup: &StartupState) -> Vec<(String, ItemMetadata, i64)> {
    let mut entries = Vec::new();
    let dir = &paths.metadata_dir;
    let read_dir = match std::fs::read_dir(dir) {
        Ok(d) => d,
        Err(_) => return entries,
    };
    for entry in read_dir.flatten() {
        let file_name = entry.file_name().to_string_lossy().to_string();
        if !file_name.ends_with(".toml") {
            continue;
        }
        let hash = file_name.strip_suffix(".toml").unwrap();
        if !metadata::is_valid_hash_file_name(hash) {
            continue;
        }
        let file = entry.path().to_string_lossy().to_string();
        let text = match std::fs::read_to_string(&file) {
            Ok(t) => t,
            Err(_) => continue,
        };
        match metadata::parse(&text) {
            Ok(meta) => {
                let mtime = file_mtime_ms(&file);
                entries.push((hash.to_string(), meta, mtime));
            }
            Err(e) => tracing::warn!("元数据解析失败，已跳过: {file}: {e}"),
        }
        // 大库回退解析可达分钟级：每 1000 文件一帧进度，启动屏实时反馈
        if entries.len() % 1000 == 0 {
            startup.report("sync", entries.len() as i32, 0);
        }
    }
    tracing::info!("已从 TOML 全量解析 {} 条元数据", entries.len());
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手动性能测量（`cargo test bench_save_toml_batch -- --ignored --nocapture`）：
    /// 串行逐条 save_toml vs persist_batch（Toml 并行路径）vs persist_batch（Db 单事务）的耗时对比
    #[test]
    #[ignore]
    fn bench_save_toml_batch() {
        let dir = std::env::temp_dir().join(format!("hawk-bench-meta-{}", std::process::id()));
        let root = dir.join("lib");
        std::fs::create_dir_all(root.join(".hawk/metadata")).unwrap();
        let paths = LibraryPaths::new(root.to_str().unwrap(), Some(dir.join("cache").to_string_lossy().to_string()));
        let startup = StartupState::default();
        let store = MetadataStore::with_mode(paths.clone(), &startup, Some(StorageMode::Toml));

        const N: usize = 2000;
        let entries: Vec<(String, ItemMetadata)> = (0..N)
            .map(|i| {
                let mut meta = ItemMetadata::default();
                meta.tags = vec!["基准".to_string()];
                (format!("{i:064x}"), meta)
            })
            .collect();

        // 串行基线
        let t0 = Instant::now();
        for (hash, meta) in &entries {
            store.save_toml(hash, meta).unwrap();
        }
        let serial = t0.elapsed();

        // 并行（覆盖写同批文件）
        let t1 = Instant::now();
        let (ok, failed) = store.persist_batch(&entries);
        let parallel = t1.elapsed();
        assert_eq!(ok.len(), N);
        assert!(failed.is_empty());

        // 纯写临时文件（不 rename）：定位成本大头在写还是原子替换
        let t2 = Instant::now();
        for (hash, meta) in &entries {
            let tmp = format!("{}/{hash}.toml.tmp", paths.metadata_dir);
            std::fs::write(&tmp, crate::core::metadata::serialize(meta)).unwrap();
            let _ = std::fs::remove_file(&tmp);
        }
        let write_only = t2.elapsed();

        println!("串行 {N} 条: {serial:?}；并行 {N} 条: {parallel:?}（加速比 {:.1}x）；纯写(不 rename): {write_only:?}", serial.as_secs_f64() / parallel.as_secs_f64());

        // 数据库模式对照：单事务落盘
        let root_db = dir.join("lib-db");
        std::fs::create_dir_all(root_db.join(".hawk/metadata")).unwrap();
        let paths_db = LibraryPaths::new(root_db.to_str().unwrap(), Some(dir.join("cache-db").to_string_lossy().to_string()));
        let store_db = MetadataStore::with_mode(paths_db, &startup, Some(StorageMode::Db));
        let t3 = Instant::now();
        let (ok_db, failed_db) = store_db.persist_batch(&entries);
        let db_time = t3.elapsed();
        assert_eq!(ok_db.len(), N);
        assert!(failed_db.is_empty());
        println!("数据库单事务 {N} 条: {db_time:?}（vs 串行 {:.1}x）", serial.as_secs_f64() / db_time.as_secs_f64());

        let _ = std::fs::remove_dir_all(&dir);
    }
}

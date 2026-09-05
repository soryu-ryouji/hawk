//! 元数据存储：.hawk/metadata/<hash>.toml 的读写 + 内存权威副本（含 path → hash 反查表）。
//! 写入采用「临时文件 + rename」的原子写；写入顺序铁律：先 TOML 成功后再写缓存与内存副本，
//! 中途崩溃自然朝 TOML 收敛。副本注水来源：IndexDb 快路径 → TOML 全量解析回退（顺带建缓存）。
//!
//! SQLite 缓存写采用**待冲刷缓冲**：内存副本即时更新，缓存写累积后按批单事务落盘
//! （≥CACHE_BATCH 条或滞留 CACHE_FLUSH_AFTER）。安全性：缓存是可重建的派生层，
//! 权威 TOML 已逐条原子落盘——崩溃/退出后启动期元数据对账按 mtime 差异从 TOML 补齐，
//! 无需重算哈希。所有直写缓存的路径先冲刷缓冲，避免旧值覆盖新值。

use crate::core::index_db::IndexDb;
use crate::core::metadata::{self, ItemMetadata};
use crate::core::paths::{file_mtime_ms, LibraryPaths};
use crate::core::startup::StartupState;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 缓存写冲刷的批量阈值（单事务摊薄 fsync）
const CACHE_BATCH: usize = 256;
/// 缓存写冲刷的时间阈值：滞留超时即使未达批量也冲刷（限制缓存滞后窗口）
const CACHE_FLUSH_AFTER: Duration = Duration::from_millis(200);

pub struct MetadataStore {
    paths: LibraryPaths,
    db: std::sync::Arc<IndexDb>,
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
    /// 构造即注水：SQLite 缓存快路径 → TOML 全量解析回退（顺带建好缓存）。
    /// TOML 回退解析期间经 StartupState 上报进度（每 1000 文件一帧，phase=sync），
    /// 大库首次启动（缓存缺失）时启动屏有实时反馈
    pub fn new(paths: LibraryPaths, db: std::sync::Arc<IndexDb>, startup: &StartupState) -> MetadataStore {
        let mut entries: Option<Vec<(String, ItemMetadata, i64)>> = None;
        if db.hydrated.load(std::sync::atomic::Ordering::SeqCst) {
            match db.load_all() {
                Ok(loaded) => {
                    tracing::info!("元数据副本已从 SQLite 缓存注水 {} 条", loaded.len());
                    entries = Some(loaded);
                }
                Err(e) => tracing::error!("元数据缓存读取失败，改由 TOML 全量解析: {e}"),
            }
        }

        let entries = match entries {
            Some(e) => e,
            None => {
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
            inner: Mutex::new(inner),
        }
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

    /// 保存元数据：先 TOML 原子写（权威层），成功后即时更新内存副本，
    /// SQLite 缓存写进入待冲刷缓冲（达批量/时限由消费循环或 flush_cache 冲刷）。
    /// TOML 写失败返回 Err（调用方决定回传 API 或仅记录）
    pub fn save(&self, hash: &str, meta: &ItemMetadata) -> Result<(), String> {
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

    /// 立即冲刷待冲刷的缓存写（单事务）
    pub fn flush_cache(&self) {
        let batch = self.take_pending();
        if !batch.is_empty() {
            self.db.save_batch(&batch);
        }
    }

    /// 时间阈值检查冲刷（消费循环每任务后调用，限制安静期的缓存滞后）
    pub fn maybe_flush_cache(&self) {
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
        let file = self.file_path(hash);
        let tmp = format!("{file}.tmp");
        std::fs::write(&tmp, metadata::serialize(meta)).map_err(|e| format!("元数据 TOML 写入失败 {tmp}: {e}"))?;
        if let Err(e) = std::fs::rename(&tmp, &file) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("元数据 TOML 替换失败 {file}: {e}"));
        }
        Ok(file_mtime_ms(&file))
    }

    /// 批量 TOML 落盘（只写权威层；成功后由调用方 apply_batch 刷内存与缓存）。
    /// 大批量时多线程并行：不同 hash 不同文件，写入互相独立（thread::scope 借用 &self）。
    /// 单条失败不中断整批：失败 hash 记入返回值第二分量并 tracing 记录
    pub fn save_toml_batch(&self, entries: &[(String, ItemMetadata)]) -> (Vec<(String, ItemMetadata, i64)>, Vec<String>) {
        /// 低于该阈值走串行（线程开销不值得）
        const PARALLEL_THRESHOLD: usize = 64;
        /// 写线程上限（机械盘/杀软环境下更多线程无收益）
        const MAX_WRITE_THREADS: usize = 8;

        fn write_chunk(
            store: &MetadataStore,
            chunk: &[(String, ItemMetadata)],
        ) -> (Vec<(String, ItemMetadata, i64)>, Vec<String>) {
            let mut ok = Vec::new();
            let mut failed = Vec::new();
            for (hash, meta) in chunk {
                match store.save_toml(hash, meta) {
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
            return write_chunk(self, entries);
        }

        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(MAX_WRITE_THREADS);
        let chunk_size = entries.len().div_ceil(threads);
        std::thread::scope(|s| {
            let handles: Vec<_> = entries
                .chunks(chunk_size)
                .map(|chunk| s.spawn(|| write_chunk(self, chunk)))
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

    /// 批量应用（内存副本 + SQLite 单事务）。TOML 须已由 save_toml 逐条落盘。
    /// 先冲刷待冲刷缓冲，避免旧值覆盖本批新值
    pub fn apply_batch(&self, entries: &[(String, ItemMetadata, i64)]) {
        if entries.is_empty() {
            return;
        }
        self.flush_cache();
        {
            let mut inner = self.inner.lock().unwrap();
            for (hash, meta, _) in entries {
                inner.by_hash.insert(hash.clone(), meta.clone());
                rebuild_path_index(&mut inner, hash, meta);
            }
        }
        self.db.save_batch(entries);
    }

    pub fn delete(&self, hash: &str) {
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

    /// 对账应用（只进不出）：TOML 被外部新增/修改后载入。
    /// 解析失败返回 false（跳过该文件，不清空任何状态，下一轮对账重试）
    pub fn apply_external_toml(&self, hash: &str, file: &str, source_mtime: i64) -> bool {
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

    /// 对账应用（只进不出）：TOML 已消失 → 清空素材参数字段（等价于重启后无元数据的语义）
    pub fn clear_external(&self, hash: &str) {
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
    /// 串行逐条 save_toml vs save_toml_batch 并行路径的耗时对比
    #[test]
    #[ignore]
    fn bench_save_toml_batch() {
        let dir = std::env::temp_dir().join(format!("hawk-bench-meta-{}", std::process::id()));
        let root = dir.join("lib");
        std::fs::create_dir_all(root.join(".hawk/metadata")).unwrap();
        let paths = LibraryPaths::new(root.to_str().unwrap(), Some(dir.join("cache").to_string_lossy().to_string()));
        let db = std::sync::Arc::new(IndexDb::open(&paths.index_db_file));
        let startup = StartupState::default();
        let store = MetadataStore::new(paths.clone(), db, &startup);

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
        let (ok, failed) = store.save_toml_batch(&entries);
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
        let _ = std::fs::remove_dir_all(&dir);
    }
}

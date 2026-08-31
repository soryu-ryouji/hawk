//! 元数据存储：.hawk/metadata/<hash>.toml 的读写 + 内存权威副本（含 path → hash 反查表）。
//! 写入采用「临时文件 + rename」的原子写；写入顺序铁律：先 TOML 成功后再写缓存与内存副本，
//! 中途崩溃自然朝 TOML 收敛。副本注水来源：IndexDb 快路径 → TOML 全量解析回退（顺带建缓存）。

use crate::core::index_db::IndexDb;
use crate::core::metadata::{self, ItemMetadata};
use crate::core::paths::{file_mtime_ms, LibraryPaths};
use crate::core::startup::StartupState;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct MetadataStore {
    paths: LibraryPaths,
    db: std::sync::Arc<IndexDb>,
    inner: Mutex<MetadataInner>,
}

#[derive(Default)]
struct MetadataInner {
    by_hash: HashMap<String, ItemMetadata>,
    hash_by_path: HashMap<String, String>,
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

    /// 整体替换文件夹快照（每轮扫描一次；遍历不完整时不调用）
    pub fn replace_folder_snapshots(&self, snapshots: &HashMap<String, (i64, i64)>) {
        self.db.replace_folder_snapshots(snapshots);
    }

    /// 各 TOML 源文件 mtime 快照（后台对账比对依据）；缓存不可用时返回 None（本轮对账跳过）
    pub fn source_mtimes(&self) -> Option<HashMap<String, i64>> {
        self.db.load_source_mtimes()
    }

    /// 保存元数据：先 TOML 原子写（权威层），成功后更新内存副本与 SQLite 缓存。
    /// TOML 写失败返回 Err（调用方决定回传 API 或仅记录）
    pub fn save(&self, hash: &str, meta: &ItemMetadata) -> Result<(), String> {
        let source_mtime = self.save_toml(hash, meta)?;
        self.apply_in_memory(hash, meta);
        self.db.save(hash, meta, source_mtime);
        Ok(())
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

    /// 批量应用（内存副本 + SQLite 单事务）。TOML 须已由 save_toml 逐条落盘
    pub fn apply_batch(&self, entries: &[(String, ItemMetadata, i64)]) {
        if entries.is_empty() {
            return;
        }
        {
            let mut inner = self.inner.lock().unwrap();
            for (hash, meta, _) in entries {
                inner.by_hash.insert(hash.clone(), meta.clone());
                rebuild_path_index(&mut inner, hash, meta);
            }
        }
        self.db.save_batch(entries);
    }

    fn apply_in_memory(&self, hash: &str, meta: &ItemMetadata) {
        let mut inner = self.inner.lock().unwrap();
        inner.by_hash.insert(hash.to_string(), meta.clone());
        rebuild_path_index(&mut inner, hash, meta);
    }

    pub fn delete(&self, hash: &str) {
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

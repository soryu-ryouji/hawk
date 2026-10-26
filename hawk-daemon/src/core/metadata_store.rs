//! 元数据存储：内存查询副本 + `.hawk/metadata.db` 唯一权威持久层。
//!
//! 写入先提交 SQLite，成功后更新内存；批量写同理。没有 TOML、双模式或缓存回退。
//! 启动只从 metadata.db 注水；读取失败写入 StartupState，由启动页呈现，不能静默按空库继续。

use crate::core::metadata::ItemMetadata;
use crate::core::metadata_db::MetadataDb;
use crate::core::paths::LibraryPaths;
use crate::core::startup::StartupState;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct MetadataStore {
    db: MetadataDb,
    inner: Mutex<MetadataInner>,
}

#[derive(Default)]
struct MetadataInner {
    by_hash: HashMap<String, ItemMetadata>,
    hash_by_path: HashMap<String, String>,
}

impl MetadataStore {
    pub fn new(paths: LibraryPaths, startup: &StartupState) -> MetadataStore {
        let db = MetadataDb::open(&paths.metadata_db_file);
        let entries = match db.load_all() {
            Ok(entries) => {
                tracing::info!("元数据已从 SQLite 加载 {} 条", entries.len());
                entries
            }
            Err(e) => {
                let message = format!("元数据数据库读取失败: {e}");
                tracing::error!("{message}");
                startup.fail(&message);
                Vec::new()
            }
        };
        let mut inner = MetadataInner::default();
        for (hash, meta) in entries {
            for path in &meta.paths {
                inner.hash_by_path.insert(path.path.clone(), hash.clone());
            }
            inner.by_hash.insert(hash, meta);
        }
        MetadataStore {
            db,
            inner: Mutex::new(inner),
        }
    }

    pub fn try_get(&self, hash: &str) -> Option<ItemMetadata> {
        self.inner.lock().unwrap().by_hash.get(hash).cloned()
    }

    pub fn palette_negative_cached(&self, hash: &str) -> bool {
        self.inner
            .lock()
            .unwrap()
            .by_hash
            .get(hash)
            .is_some_and(|meta| meta.palette_negative_cached())
    }

    /// 全量快照：启动注水、分类/标签级联与派生补全使用
    pub fn snapshot(&self) -> Vec<(String, ItemMetadata)> {
        self.inner
            .lock()
            .unwrap()
            .by_hash
            .iter()
            .map(|(hash, meta)| (hash.clone(), meta.clone()))
            .collect()
    }

    pub fn find_hash_by_path(&self, library_path: &str) -> Option<String> {
        self.inner
            .lock()
            .unwrap()
            .hash_by_path
            .get(library_path)
            .cloned()
    }

    pub fn hashes_with_missing_palette(&self) -> Vec<String> {
        self.inner
            .lock()
            .unwrap()
            .by_hash
            .iter()
            .filter(|(_, meta)| meta.palette.is_none())
            .map(|(hash, _)| hash.clone())
            .collect()
    }

    pub fn hashes_with_missing_phash(&self) -> Vec<String> {
        self.inner
            .lock()
            .unwrap()
            .by_hash
            .iter()
            .filter(|(_, meta)| {
                meta.phash
                    .as_ref()
                    .is_none_or(|hash| hash.version != crate::core::perceptual_hash::PHASH_VERSION)
            })
            .map(|(hash, _)| hash.clone())
            .collect()
    }

    pub fn hashes_with_zero_dim(&self) -> Vec<String> {
        self.inner
            .lock()
            .unwrap()
            .by_hash
            .iter()
            .filter(|(_, meta)| meta.width == 0 || meta.height == 0)
            .map(|(hash, _)| hash.clone())
            .collect()
    }

    pub fn load_folder_snapshots(&self) -> HashMap<String, (i64, i64)> {
        self.db.load_folder_snapshots()
    }

    pub fn replace_folder_snapshots(&self, snapshots: &HashMap<String, (i64, i64)>) {
        if let Err(e) = self.db.replace_folder_snapshots(snapshots) {
            tracing::error!("{e}");
        }
    }

    /// SQLite 提交成功后才更新内存副本
    pub fn save(&self, hash: &str, meta: &ItemMetadata) -> Result<(), String> {
        self.db.save(hash, meta)?;
        let mut inner = self.inner.lock().unwrap();
        inner.by_hash.insert(hash.to_string(), meta.clone());
        rebuild_path_index(&mut inner, hash, meta);
        Ok(())
    }

    /// 单事务批量提交；成功项已同步内存，返回（成功项，失败 hash）。
    /// 任一条失败导致事务回滚，因此当前实现的成功项为空或全部条目。
    pub fn persist_batch(
        &self,
        entries: &[(String, ItemMetadata)],
    ) -> (Vec<(String, ItemMetadata)>, Vec<String>) {
        match self.db.save_batch(entries) {
            Ok(()) => {
                let mut inner = self.inner.lock().unwrap();
                for (hash, meta) in entries {
                    inner.by_hash.insert(hash.clone(), meta.clone());
                    rebuild_path_index(&mut inner, hash, meta);
                }
                (entries.to_vec(), Vec::new())
            }
            Err(e) => {
                tracing::error!("{e}");
                (
                    Vec::new(),
                    entries.iter().map(|(hash, _)| hash.clone()).collect(),
                )
            }
        }
    }

    pub fn delete(&self, hash: &str) {
        if let Err(e) = self.db.delete(hash) {
            tracing::error!("{e}，内存副本保留");
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        if let Some(meta) = inner.by_hash.remove(hash) {
            for path in &meta.paths {
                if inner
                    .hash_by_path
                    .get(&path.path)
                    .is_some_and(|current| current == hash)
                {
                    inner.hash_by_path.remove(&path.path);
                }
            }
        }
    }
}

fn rebuild_path_index(inner: &mut MetadataInner, hash: &str, meta: &ItemMetadata) {
    inner.hash_by_path.retain(|_, current| current != hash);
    for path in &meta.paths {
        inner
            .hash_by_path
            .insert(path.path.clone(), hash.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_v1_sqlite_is_preserved_when_adding_phash_columns() {
        let base = std::env::temp_dir().join(format!("hawk-sqlite-v1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("library");
        std::fs::create_dir_all(root.join(".hawk")).unwrap();
        let db_file = root.join(".hawk/metadata.db");
        let hash = "c".repeat(64);
        {
            let conn = rusqlite::Connection::open(&db_file).unwrap();
            conn.execute_batch(&format!(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE items (hash TEXT PRIMARY KEY, url TEXT, star INTEGER NOT NULL DEFAULT 0, annotation TEXT, source_mtime INTEGER NOT NULL DEFAULT 0, width INTEGER NOT NULL DEFAULT 0, height INTEGER NOT NULL DEFAULT 0, palette TEXT);
                 CREATE TABLE paths (hash TEXT NOT NULL, path TEXT NOT NULL, size INTEGER NOT NULL, mtime INTEGER NOT NULL, PRIMARY KEY (hash, path));
                 CREATE TABLE tags (hash TEXT NOT NULL, tag TEXT NOT NULL, PRIMARY KEY (hash, tag));
                 CREATE TABLE categories (hash TEXT NOT NULL, category TEXT NOT NULL, PRIMARY KEY (hash, category));
                 CREATE TABLE folders (path TEXT PRIMARY KEY, mtime INTEGER NOT NULL, entries INTEGER NOT NULL);
                 INSERT INTO meta VALUES ('schema_version', '1');
                 INSERT INTO items (hash, star, width, height) VALUES ('{hash}', 4, 10, 20);
                 INSERT INTO paths VALUES ('{hash}', 'a.png', 100, 200);
                 INSERT INTO tags VALUES ('{hash}', '保留');"
            ))
            .unwrap();
        }
        let paths = LibraryPaths::new(
            &root.to_string_lossy(),
            Some(base.join("cache").to_string_lossy().to_string()),
        );
        let startup = StartupState::default();
        let store = MetadataStore::new(paths, &startup);
        let meta = store.try_get(&hash).expect("v1 SQLite 条目必须保留");
        assert_eq!(meta.tags, vec!["保留"]);
        assert_eq!(meta.star, 4);
        assert_eq!(meta.paths.len(), 1);
        assert!(meta.phash.is_none());
        assert!(startup.snapshot().1.is_none());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn sqlite_is_the_only_startup_source_and_preserves_metadata() {
        let base = std::env::temp_dir().join(format!("hawk-sqlite-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("library");
        std::fs::create_dir_all(&root).unwrap();
        let paths = LibraryPaths::new(
            &root.to_string_lossy(),
            Some(base.join("cache").to_string_lossy().to_string()),
        );
        paths.ensure_layout();
        let startup = StartupState::default();
        let store = MetadataStore::new(paths.clone(), &startup);
        let hash = "a".repeat(64);
        let meta = ItemMetadata {
            tags: vec!["SQLite".to_string()],
            paths: vec![crate::core::metadata::PathEntry {
                path: "a.png".to_string(),
                size: 1,
                modification_time: 2,
            }],
            ..Default::default()
        };
        store.save(&hash, &meta).unwrap();
        drop(store);

        // 旧 TOML 路径即使存在也不是启动来源；无需解析或迁移它
        let old_toml = root
            .join(".hawk/metadata")
            .join(format!("{}.toml", "b".repeat(64)));
        std::fs::create_dir_all(old_toml.parent().unwrap()).unwrap();
        std::fs::write(&old_toml, "tags = [\" abandoned \"]\n").unwrap();
        let reopened = MetadataStore::new(paths, &startup);
        assert_eq!(reopened.try_get(&hash), Some(meta));
        assert!(reopened.try_get(&"b".repeat(64)).is_none());
        let _ = std::fs::remove_dir_all(base);
    }
}

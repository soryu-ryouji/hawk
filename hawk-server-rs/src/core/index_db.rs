//! 元数据的本地 SQLite 派生缓存（库外缓存目录 index.db）。
//! .hawk/metadata/*.toml 是唯一权威数据源；本库只是本机加速器，对账只进不出。
//! 任何故障（打不开/写失败/读失败）只退化性能：回到纯 TOML 行为，绝不影响权威数据。
//! schema v1 与 C# 版逐字一致（现有缓存直接可读）；journal_mode=DELETE 不产生 -wal/-shm。

use crate::core::metadata::{ItemMetadata, PaletteEntry};
use rusqlite::{Connection, OptionalExtension};
use std::collections::HashMap;
use std::sync::Mutex;

const SCHEMA_VERSION: &str = "1";
const HYDRATED_KEY: &str = "hydrated";

pub struct IndexDb {
    conn: Mutex<Option<Connection>>,
    /// 写失败后熔断：后续操作一律短路，等价于缓存不存在（纯 TOML 行为）
    poisoned: Mutex<bool>,
    /// 缓存是否已完成至少一次全量注水。false 时内容不可信，必须由 TOML 全量重建
    pub hydrated: std::sync::atomic::AtomicBool,
}

impl IndexDb {
    pub fn open(index_db_file: &str) -> IndexDb {
        let result = (|| -> rusqlite::Result<Connection> {
            let conn = Connection::open(index_db_file)?;
            conn.pragma_update(None, "journal_mode", "DELETE")?;
            conn.pragma_update(None, "busy_timeout", 5000)?;
            conn.pragma_update(None, "synchronous", "NORMAL")?;
            Ok(conn)
        })();

        match result {
            Ok(conn) => {
                let db = IndexDb {
                    conn: Mutex::new(Some(conn)),
                    poisoned: Mutex::new(false),
                    hydrated: std::sync::atomic::AtomicBool::new(false),
                };
                db.init_schema();
                db
            }
            Err(e) => {
                tracing::error!("元数据缓存打开失败，退化为纯 TOML 模式（仅影响启动与查询速度）: {e}");
                IndexDb {
                    conn: Mutex::new(None),
                    poisoned: Mutex::new(false),
                    hydrated: std::sync::atomic::AtomicBool::new(false),
                }
            }
        }
    }

    /// 缓存可用（打开成功且未熔断）。不可用时全部方法安全短路
    pub fn enabled(&self) -> bool {
        let poisoned = self.poisoned.lock().unwrap();
        self.conn.lock().unwrap().is_some() && !*poisoned
    }

    /// 全量注水（仅启动时、缓存缺失/重建后调用一次）：事务内清空并写入全部条目，最后置 hydrated 标记
    pub fn hydrate(&self, entries: &[(String, ItemMetadata, i64)]) {
        if !self.enabled() {
            return;
        }
        let result = self.with_conn(|conn| {
            let tx = conn.transaction()?;
            tx.execute("DELETE FROM paths", [])?;
            tx.execute("DELETE FROM tags", [])?;
            tx.execute("DELETE FROM categories", [])?;
            tx.execute("DELETE FROM items", [])?;
            for (hash, meta, mtime) in entries {
                insert_item(&tx, hash, meta, *mtime)?;
            }
            tx.commit()?;
            Ok(())
        });
        match result {
            Ok(()) => {
                self.write_meta(HYDRATED_KEY, "1");
                self.hydrated.store(true, std::sync::atomic::Ordering::SeqCst);
                tracing::info!("元数据缓存已注水 {} 条", entries.len());
            }
            Err(e) => self.poison(&format!("元数据缓存注水失败，保持未注水状态: {e}")),
        }
    }

    /// 全量读取（启动注水内存用）。缓存不可用或损坏时返回 Err，调用方回退 TOML 全量解析
    pub fn load_all(&self) -> rusqlite::Result<Vec<(String, ItemMetadata, i64)>> {
        let conn = self.conn.lock().unwrap();
        let conn = conn.as_ref().ok_or_else(|| rusqlite::Error::InvalidQuery)?;
        let mut items: HashMap<String, (ItemMetadata, i64)> = HashMap::new();
        {
            let mut stmt = conn.prepare(
                "SELECT hash, url, star, annotation, source_mtime, width, height, palette, palette_version FROM items",
            )?;
            let rows = stmt.query_map([], |row| {
                let palette_json: Option<String> = row.get(7)?;
                Ok((
                    row.get::<_, String>(0)?,
                    ItemMetadata {
                        url: row.get(1)?,
                        star: row.get::<_, i64>(2)? as i32,
                        annotation: row.get(3)?,
                        palette_version: row.get::<_, i64>(8)? as i32,
                        palette: palette_json.as_deref().and_then(parse_palette_json),
                        width: row.get::<_, i64>(5)? as i32,
                        height: row.get::<_, i64>(6)? as i32,
                        ..ItemMetadata::default()
                    },
                    row.get::<_, i64>(4)?,
                ))
            })?;
            for row in rows {
                let (hash, meta, mtime) = row?;
                items.insert(hash, (meta, mtime));
            }
        }
        {
            let mut stmt = conn.prepare("SELECT hash, path, size, mtime FROM paths")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?;
            for row in rows {
                let (hash, path, size, mtime) = row?;
                if let Some((meta, _)) = items.get_mut(&hash) {
                    meta.paths.push(crate::core::metadata::PathEntry {
                        path,
                        size,
                        modification_time: mtime,
                    });
                }
            }
        }
        load_child_rows(conn, "SELECT hash, tag FROM tags", &mut items, |meta, value| meta.tags.push(value))?;
        load_child_rows(conn, "SELECT hash, category FROM categories", &mut items, |meta, value| {
            meta.categories.push(value)
        })?;

        Ok(items.into_iter().map(|(h, (m, t))| (h, m, t)).collect())
    }

    /// 写穿：镜像一条元数据及其 TOML 源文件 mtime（对账比对依据）。失败熔断，不影响权威数据
    pub fn save(&self, hash: &str, meta: &ItemMetadata, source_mtime: i64) {
        if !self.enabled() {
            return;
        }
        let result = self.with_conn(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO items (hash, url, star, annotation, source_mtime, width, height, palette, palette_version) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
                 ON CONFLICT(hash) DO UPDATE SET url=?2, star=?3, annotation=?4, source_mtime=?5, width=?6, height=?7, palette=?8, palette_version=?9",
                rusqlite::params![
                    hash,
                    meta.url,
                    meta.star,
                    meta.annotation,
                    source_mtime,
                    meta.width,
                    meta.height,
                    meta.palette.as_ref().map(|p| serde_json::to_string(p).unwrap_or_default()),
                    meta.palette_version,
                ],
            )?;
            delete_child_rows(&tx, hash)?;
            insert_child_rows(&tx, hash, meta)?;
            tx.commit()?;
            Ok(())
        });
        if let Err(e) = result {
            self.poison(&format!("元数据缓存写入失败 {hash}: {e}"));
        }
    }

    /// 删除一条镜像。失败熔断，不影响权威数据
    pub fn delete(&self, hash: &str) {
        if !self.enabled() {
            return;
        }
        let result = self.with_conn(|conn| {
            let tx = conn.transaction()?;
            tx.execute("DELETE FROM items WHERE hash=?1", [hash])?;
            delete_child_rows(&tx, hash)?;
            tx.commit()?;
            Ok(())
        });
        if let Err(e) = result {
            self.poison(&format!("元数据缓存删除失败 {hash}: {e}"));
        }
    }

    /// 全量文件夹快照（增量扫描的对比基准）。缓存不可用时返回空表（首轮 = 全量深入）
    pub fn load_folder_snapshots(&self) -> HashMap<String, (i64, i64)> {
        if !self.enabled() {
            return HashMap::new();
        }
        let guard = self.conn.lock().unwrap();
        let conn = match guard.as_ref() {
            Some(c) => c,
            None => return HashMap::new(),
        };
        let result = (|| -> rusqlite::Result<HashMap<String, (i64, i64)>> {
            let mut stmt = conn.prepare("SELECT path, mtime, entries FROM folders")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))
            })?;
            let mut out = HashMap::new();
            for row in rows {
                let (path, mtime, entries) = row?;
                out.insert(path, (mtime, entries));
            }
            Ok(out)
        })();
        drop(guard);
        match result {
            Ok(map) => map,
            Err(e) => {
                self.poison(&format!("文件夹快照读取失败: {e}"));
                HashMap::new()
            }
        }
    }

    /// 整体替换文件夹快照（每轮扫描一次；遍历不完整时不调用，保留旧快照）
    pub fn replace_folder_snapshots(&self, snapshots: &HashMap<String, (i64, i64)>) {
        if !self.enabled() {
            return;
        }
        let result = self.with_conn(|conn| {
            let tx = conn.transaction()?;
            tx.execute("DELETE FROM folders", [])?;
            for (path, (mtime, entries)) in snapshots {
                tx.execute(
                    "INSERT INTO folders (path, mtime, entries) VALUES (?1, ?2, ?3)",
                    rusqlite::params![path, mtime, entries],
                )?;
            }
            tx.commit()?;
            Ok(())
        });
        if let Err(e) = result {
            self.poison(&format!("文件夹快照写入失败: {e}"));
        }
    }

    /// 各 TOML 源文件的 mtime 快照（后台对账比对用）。缓存不可用时返回 None，调用方跳过本轮
    pub fn load_source_mtimes(&self) -> Option<HashMap<String, i64>> {
        if !self.enabled() {
            return None;
        }
        let guard = self.conn.lock().unwrap();
        let conn = guard.as_ref()?;
        let result = (|| -> rusqlite::Result<HashMap<String, i64>> {
            let mut stmt = conn.prepare("SELECT hash, source_mtime FROM items")?;
            let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;
            let mut out = HashMap::new();
            for row in rows {
                let (hash, mtime) = row?;
                out.insert(hash, mtime);
            }
            Ok(out)
        })();
        drop(guard);
        match result {
            Ok(map) => Some(map),
            Err(e) => {
                self.poison(&format!("source_mtime 快照读取失败: {e}"));
                None
            }
        }
    }

    /// 清空全部镜像并复位注水标记（重建用；测试可经 MetadataStore 触发全量重建）
    #[allow(dead_code)]
    pub fn clear(&self) {
        if !self.enabled() {
            return;
        }
        let result = self.with_conn(|conn| {
            let tx = conn.transaction()?;
            tx.execute("DELETE FROM paths", [])?;
            tx.execute("DELETE FROM tags", [])?;
            tx.execute("DELETE FROM categories", [])?;
            tx.execute("DELETE FROM items", [])?;
            tx.commit()?;
            Ok(())
        });
        if let Err(e) = result {
            self.poison(&format!("缓存清空失败: {e}"));
        }
        self.write_meta(HYDRATED_KEY, "0");
        self.hydrated.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    fn init_schema(&self) {
        if let Err(e) = self.with_conn(|conn| {
            conn.execute(
                "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
                [],
            )?;
            let version: Option<String> = conn
                .query_row("SELECT value FROM meta WHERE key='schema_version'", [], |row| row.get(0))
                .optional()?;
            if version.as_deref() != Some(SCHEMA_VERSION) {
                // 版本不符（含首次建库）：重建全部表；注水标记保持未置位，由 MetadataStore 从 TOML 全量重建
                for table in ["items", "paths", "tags", "categories"] {
                    conn.execute(&format!("DROP TABLE IF EXISTS {table}"), [])?;
                }
                conn.execute(
                    "CREATE TABLE items (hash TEXT PRIMARY KEY, url TEXT, star INTEGER NOT NULL DEFAULT 0, \
                     annotation TEXT, source_mtime INTEGER NOT NULL DEFAULT 0, width INTEGER NOT NULL DEFAULT 0, \
                     height INTEGER NOT NULL DEFAULT 0, palette TEXT, palette_version INTEGER NOT NULL DEFAULT 0)",
                    [],
                )?;
                conn.execute(
                    "CREATE TABLE paths (hash TEXT NOT NULL, path TEXT NOT NULL, size INTEGER NOT NULL, mtime INTEGER NOT NULL, PRIMARY KEY (hash, path))",
                    [],
                )?;
                conn.execute(
                    "CREATE TABLE tags (hash TEXT NOT NULL, tag TEXT NOT NULL, PRIMARY KEY (hash, tag))",
                    [],
                )?;
                conn.execute(
                    "CREATE TABLE categories (hash TEXT NOT NULL, category TEXT NOT NULL, PRIMARY KEY (hash, category))",
                    [],
                )?;
                conn.execute("INSERT INTO meta (key, value) VALUES ('schema_version', ?1) ON CONFLICT(key) DO UPDATE SET value=?1", [SCHEMA_VERSION])?;
                tracing::info!("元数据缓存 schema 已重建 v{SCHEMA_VERSION}");
            }
            // folders 快照表后加（旧缓存无此表）：幂等补建，免于整库重建
            conn.execute(
                "CREATE TABLE IF NOT EXISTS folders (path TEXT PRIMARY KEY, mtime INTEGER NOT NULL, entries INTEGER NOT NULL)",
                [],
            )?;
            Ok(())
        }) {
            tracing::error!("元数据缓存 schema 初始化失败，退化为纯 TOML 模式: {e}");
            self.poison("schema 初始化失败");
            return;
        }

        // 注水标记
        let hydrated = self.read_meta("hydrated").map(|v| v == "1").unwrap_or(false);
        self.hydrated.store(hydrated, std::sync::atomic::Ordering::SeqCst);
    }

    fn with_conn<T>(&self, f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
        let mut conn = self.conn.lock().unwrap();
        let conn = conn.as_mut().ok_or_else(|| rusqlite::Error::InvalidQuery)?;
        f(conn)
    }

    fn poison(&self, what: &str) {
        tracing::error!("{what}，缓存已熔断（退化为纯 TOML 模式）");
        *self.poisoned.lock().unwrap() = true;
    }

    fn read_meta(&self, key: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        let conn = conn.as_ref()?;
        conn.query_row("SELECT value FROM meta WHERE key=?1", [key], |row| row.get(0))
            .optional()
            .ok()
            .flatten()
    }

    fn write_meta(&self, key: &str, value: &str) {
        let conn = self.conn.lock().unwrap();
        if let Some(conn) = conn.as_ref() {
            let _ = conn.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=?2",
                rusqlite::params![key, value],
            );
        }
    }
}

/// 调色板的镜像存储格式（与 TOML 同款 entry；null/损坏 = 未提炼，worker 重新提炼补齐）
fn parse_palette_json(json: &str) -> Option<Vec<PaletteEntry>> {
    serde_json::from_str(json).ok()
}

fn insert_item(tx: &rusqlite::Transaction, hash: &str, meta: &ItemMetadata, source_mtime: i64) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO items (hash, url, star, annotation, source_mtime, width, height, palette, palette_version) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            hash,
            meta.url,
            meta.star,
            meta.annotation,
            source_mtime,
            meta.width,
            meta.height,
            meta.palette.as_ref().map(|p| serde_json::to_string(p).unwrap_or_default()),
            meta.palette_version,
        ],
    )?;
    insert_child_rows(tx, hash, meta)
}

fn insert_child_rows(tx: &rusqlite::Transaction, hash: &str, meta: &ItemMetadata) -> rusqlite::Result<()> {
    for p in &meta.paths {
        tx.execute(
            "INSERT INTO paths (hash, path, size, mtime) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![hash, p.path, p.size, p.modification_time],
        )?;
    }
    for tag in &meta.tags {
        tx.execute("INSERT OR IGNORE INTO tags (hash, tag) VALUES (?1, ?2)", rusqlite::params![hash, tag])?;
    }
    for category in &meta.categories {
        tx.execute(
            "INSERT OR IGNORE INTO categories (hash, category) VALUES (?1, ?2)",
            rusqlite::params![hash, category],
        )?;
    }
    Ok(())
}

fn delete_child_rows(tx: &rusqlite::Transaction, hash: &str) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM paths WHERE hash=?1", [hash])?;
    tx.execute("DELETE FROM tags WHERE hash=?1", [hash])?;
    tx.execute("DELETE FROM categories WHERE hash=?1", [hash])?;
    Ok(())
}

fn load_child_rows(
    conn: &Connection,
    sql: &str,
    items: &mut HashMap<String, (ItemMetadata, i64)>,
    add: impl Fn(&mut ItemMetadata, String),
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
    for row in rows {
        let (hash, value) = row?;
        if let Some((meta, _)) = items.get_mut(&hash) {
            add(meta, value);
        }
    }
    Ok(())
}


//! `.hawk/metadata.db`：素材元数据的唯一权威 SQLite 存储与启动注水来源。
//!
//! 不再承担库外 TOML 镜像角色，也没有写失败熔断/纯文本回退：读写错误直接上传调用方。
//! schema v2 在 v1 上增量加入 phash/phash_version，保留已有 SQLite 元数据；不读取 TOML。

use crate::core::metadata::{ItemMetadata, PaletteEntry, PerceptualHash};
use rusqlite::{Connection, OptionalExtension};
use std::collections::HashMap;
use std::sync::Mutex;

const SCHEMA_VERSION: &str = "2";

pub struct MetadataDb {
    /// 打开或 schema 初始化失败时保持 None，后续读写返回错误；启动状态据此进入错误页
    conn: Mutex<Option<Connection>>,
}

impl MetadataDb {
    pub fn open(path: &str) -> MetadataDb {
        let result = (|| -> rusqlite::Result<Connection> {
            let conn = Connection::open(path)?;
            conn.pragma_update(None, "journal_mode", "DELETE")?;
            conn.pragma_update(None, "busy_timeout", 5000)?;
            conn.pragma_update(None, "synchronous", "NORMAL")?;
            Ok(conn)
        })();
        let db = match result {
            Ok(conn) => MetadataDb {
                conn: Mutex::new(Some(conn)),
            },
            Err(e) => {
                tracing::error!("元数据数据库打开失败 {path}: {e}");
                MetadataDb {
                    conn: Mutex::new(None),
                }
            }
        };
        if let Err(e) = db.init_schema() {
            tracing::error!("元数据数据库 schema 初始化失败 {path}: {e}");
            db.conn.lock().unwrap().take();
        }
        db
    }

    fn init_schema(&self) -> rusqlite::Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
                [],
            )?;
            let version: Option<String> = conn
                .query_row("SELECT value FROM meta WHERE key='schema_version'", [], |row| {
                    row.get(0)
                })
                .optional()?;
            if version
                .as_deref()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0)
                > SCHEMA_VERSION.parse::<u64>().unwrap()
            {
                return Err(rusqlite::Error::InvalidQuery);
            }

            // v1 表继续保留 source_mtime 列（不再读取/写入业务含义），避免丢弃已有 SQLite 数据。
            conn.execute(
                "CREATE TABLE IF NOT EXISTS items (hash TEXT PRIMARY KEY, url TEXT, star INTEGER NOT NULL DEFAULT 0, \
                 annotation TEXT, source_mtime INTEGER NOT NULL DEFAULT 0, width INTEGER NOT NULL DEFAULT 0, \
                 height INTEGER NOT NULL DEFAULT 0, palette TEXT, phash TEXT, phash_version INTEGER NOT NULL DEFAULT 0)",
                [],
            )?;
            conn.execute(
                "CREATE TABLE IF NOT EXISTS paths (hash TEXT NOT NULL, path TEXT NOT NULL, size INTEGER NOT NULL, mtime INTEGER NOT NULL, PRIMARY KEY (hash, path))",
                [],
            )?;
            conn.execute(
                "CREATE TABLE IF NOT EXISTS tags (hash TEXT NOT NULL, tag TEXT NOT NULL, PRIMARY KEY (hash, tag))",
                [],
            )?;
            conn.execute(
                "CREATE TABLE IF NOT EXISTS categories (hash TEXT NOT NULL, category TEXT NOT NULL, PRIMARY KEY (hash, category))",
                [],
            )?;
            conn.execute(
                "CREATE TABLE IF NOT EXISTS folders (path TEXT PRIMARY KEY, mtime INTEGER NOT NULL, entries INTEGER NOT NULL)",
                [],
            )?;
            // v1 → v2 只做增量加列；已有标签、评分、备注、调色板和位置原样保留。
            ensure_column(conn, "phash", "TEXT")?;
            ensure_column(conn, "phash_version", "INTEGER NOT NULL DEFAULT 0")?;
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', ?1) \
                 ON CONFLICT(key) DO UPDATE SET value=?1",
                [SCHEMA_VERSION],
            )?;
            Ok(())
        })
    }

    /// 启动全量注水：数据库是唯一来源，不做 TOML/缓存回退。
    pub fn load_all(&self) -> rusqlite::Result<Vec<(String, ItemMetadata)>> {
        self.with_conn(|conn| {
            let mut items: HashMap<String, ItemMetadata> = HashMap::new();
            {
                let mut stmt = conn.prepare(
                    "SELECT hash, url, star, annotation, width, height, palette, phash, phash_version FROM items",
                )?;
                let rows = stmt.query_map([], |row| {
                    let palette_json: Option<String> = row.get(6)?;
                    let phash_version = row.get::<_, i64>(8)? as u32;
                    let phash_value = row.get::<_, Option<String>>(7)?;
                    let phash = (phash_version > 0).then(|| PerceptualHash {
                        version: phash_version,
                        value: phash_value.and_then(|hex| u64::from_str_radix(&hex, 16).ok()),
                    });
                    Ok((
                        row.get::<_, String>(0)?,
                        ItemMetadata {
                            url: row.get(1)?,
                            star: row.get::<_, i64>(2)? as i32,
                            annotation: row.get(3)?,
                            width: row.get::<_, i64>(4)? as i32,
                            height: row.get::<_, i64>(5)? as i32,
                            palette: palette_json.as_deref().and_then(parse_palette_json),
                            phash,
                            ..ItemMetadata::default()
                        },
                    ))
                })?;
                for row in rows {
                    let (hash, meta) = row?;
                    items.insert(hash, meta);
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
                    if let Some(meta) = items.get_mut(&hash) {
                        meta.paths.push(crate::core::metadata::PathEntry {
                            path,
                            size,
                            modification_time: mtime,
                        });
                    }
                }
            }
            load_child_rows(conn, "SELECT hash, tag FROM tags", &mut items, |meta, value| {
                meta.tags.push(value)
            })?;
            load_child_rows(
                conn,
                "SELECT hash, category FROM categories",
                &mut items,
                |meta, value| meta.categories.push(value),
            )?;
            Ok(items.into_iter().collect())
        })
    }

    pub fn save(&self, hash: &str, meta: &ItemMetadata) -> Result<(), String> {
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            upsert_item(&tx, hash, meta)?;
            tx.commit()?;
            Ok(())
        })
        .map_err(|e| format!("元数据数据库写入失败 {hash}: {e}"))
    }

    pub fn save_batch(&self, entries: &[(String, ItemMetadata)]) -> Result<(), String> {
        if entries.is_empty() {
            return Ok(());
        }
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            for (hash, meta) in entries {
                upsert_item(&tx, hash, meta)?;
            }
            tx.commit()?;
            Ok(())
        })
        .map_err(|e| format!("元数据数据库批量写入失败: {e}"))
    }

    pub fn delete(&self, hash: &str) -> Result<(), String> {
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            tx.execute("DELETE FROM items WHERE hash=?1", [hash])?;
            delete_child_rows(&tx, hash)?;
            tx.commit()?;
            Ok(())
        })
        .map_err(|e| format!("元数据数据库删除失败 {hash}: {e}"))
    }

    pub fn load_folder_snapshots(&self) -> HashMap<String, (i64, i64)> {
        let result = self.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT path, mtime, entries FROM folders")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?;
            let mut out = HashMap::new();
            for row in rows {
                let (path, mtime, entries) = row?;
                out.insert(path, (mtime, entries));
            }
            Ok(out)
        });
        match result {
            Ok(map) => map,
            Err(e) => {
                tracing::error!("文件夹快照读取失败，按首轮全量扫描处理: {e}");
                HashMap::new()
            }
        }
    }

    pub fn replace_folder_snapshots(
        &self,
        snapshots: &HashMap<String, (i64, i64)>,
    ) -> Result<(), String> {
        self.with_conn(|conn| {
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
        })
        .map_err(|e| format!("文件夹快照写入失败: {e}"))
    }

    fn with_conn<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> {
        let mut conn = self.conn.lock().unwrap();
        let conn = conn.as_mut().ok_or(rusqlite::Error::InvalidQuery)?;
        f(conn)
    }
}

fn ensure_column(conn: &Connection, name: &str, definition: &str) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(items)")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    let exists = rows.into_iter().flatten().any(|column| column == name);
    drop(stmt);
    if !exists {
        conn.execute(
            &format!("ALTER TABLE items ADD COLUMN {name} {definition}"),
            [],
        )?;
    }
    Ok(())
}

fn parse_palette_json(json: &str) -> Option<Vec<PaletteEntry>> {
    serde_json::from_str(json).ok()
}

fn upsert_item(
    tx: &rusqlite::Transaction,
    hash: &str,
    meta: &ItemMetadata,
) -> rusqlite::Result<()> {
    let phash_version = meta.phash.as_ref().map_or(0, |p| p.version as i64);
    let phash_value = meta
        .phash
        .as_ref()
        .and_then(|p| p.value.map(|value| format!("{value:016x}")));
    tx.execute(
        "INSERT INTO items (hash, url, star, annotation, source_mtime, width, height, palette, phash, phash_version) \
         VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, ?8, ?9) \
         ON CONFLICT(hash) DO UPDATE SET url=?2, star=?3, annotation=?4, source_mtime=0, \
         width=?5, height=?6, palette=?7, phash=?8, phash_version=?9",
        rusqlite::params![
            hash,
            meta.url,
            meta.star,
            meta.annotation,
            meta.width,
            meta.height,
            meta.palette.as_ref().map(|p| serde_json::to_string(p).unwrap_or_default()),
            phash_value,
            phash_version,
        ],
    )?;
    delete_child_rows(tx, hash)?;
    insert_child_rows(tx, hash, meta)
}

fn insert_child_rows(
    tx: &rusqlite::Transaction,
    hash: &str,
    meta: &ItemMetadata,
) -> rusqlite::Result<()> {
    for path in &meta.paths {
        tx.execute(
            "INSERT INTO paths (hash, path, size, mtime) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![hash, path.path, path.size, path.modification_time],
        )?;
    }
    for tag in &meta.tags {
        tx.execute(
            "INSERT OR IGNORE INTO tags (hash, tag) VALUES (?1, ?2)",
            rusqlite::params![hash, tag],
        )?;
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
    items: &mut HashMap<String, ItemMetadata>,
    add: impl Fn(&mut ItemMetadata, String),
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (hash, value) = row?;
        if let Some(meta) = items.get_mut(&hash) {
            add(meta, value);
        }
    }
    Ok(())
}

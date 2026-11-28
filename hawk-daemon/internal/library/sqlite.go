package library

import (
	"database/sql"
	"encoding/json"
	"fmt"
	"sort"
	"strings"

	_ "modernc.org/sqlite"
)

const sqliteSchemaVersion = "3"

// MetadataDB 是 .hawk/metadata.db 的唯一读写入口。
type MetadataDB struct {
	db *sql.DB
}

// OpenMetadataDB 打开数据库并建表。失败时返回错误，由启动流程进入错误页。
func OpenMetadataDB(file string) (*MetadataDB, error) {
	db, err := sql.Open("sqlite", file)
	if err != nil {
		return nil, fmt.Errorf("元数据数据库打开失败 %s: %w", file, err)
	}
	db.SetMaxOpenConns(1)
	for _, pragma := range []string{
		"PRAGMA journal_mode=DELETE",
		"PRAGMA busy_timeout=5000",
		"PRAGMA synchronous=NORMAL",
	} {
		if _, err := db.Exec(pragma); err != nil {
			_ = db.Close()
			return nil, fmt.Errorf("元数据数据库初始化失败 %s: %w", file, err)
		}
	}
	m := &MetadataDB{db: db}
	if err := m.initSchema(); err != nil {
		_ = db.Close()
		return nil, fmt.Errorf("元数据数据库 schema 初始化失败 %s: %w", file, err)
	}
	return m, nil
}

func (m *MetadataDB) Close() error { return m.db.Close() }

func (m *MetadataDB) initSchema() error {
	statements := []string{
		"CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
		`CREATE TABLE IF NOT EXISTS items (hash TEXT PRIMARY KEY, url TEXT, star INTEGER NOT NULL DEFAULT 0,
		 annotation TEXT, source_mtime INTEGER NOT NULL DEFAULT 0, width INTEGER NOT NULL DEFAULT 0,
		 height INTEGER NOT NULL DEFAULT 0, palette TEXT)`,
		"CREATE TABLE IF NOT EXISTS paths (hash TEXT NOT NULL, path TEXT NOT NULL, size INTEGER NOT NULL, mtime INTEGER NOT NULL, PRIMARY KEY (hash, path))",
		"CREATE TABLE IF NOT EXISTS tags (hash TEXT NOT NULL, tag TEXT NOT NULL, PRIMARY KEY (hash, tag))",
		"CREATE TABLE IF NOT EXISTS categories (hash TEXT NOT NULL, category TEXT NOT NULL, PRIMARY KEY (hash, category))",
		"CREATE TABLE IF NOT EXISTS folders (path TEXT PRIMARY KEY, mtime INTEGER NOT NULL, entries INTEGER NOT NULL)",
	}
	for _, statement := range statements {
		if _, err := m.db.Exec(statement); err != nil {
			return err
		}
	}
	var version string
	err := m.db.QueryRow("SELECT value FROM meta WHERE key='schema_version'").Scan(&version)
	if err != nil && err != sql.ErrNoRows {
		return err
	}
	if version != "" && version > sqliteSchemaVersion {
		return fmt.Errorf("元数据 schema 版本 %s 高于当前支持版本 %s", version, sqliteSchemaVersion)
	}
	_, err = m.db.Exec(
		"INSERT INTO meta (key, value) VALUES ('schema_version', ?1) ON CONFLICT(key) DO UPDATE SET value=?1",
		sqliteSchemaVersion,
	)
	return err
}

// LoadAll 一次读取全部素材，作为内存索引的注水来源。
func (m *MetadataDB) LoadAll() ([]*Item, error) {
	items := make(map[string]*Item)
	rows, err := m.db.Query("SELECT hash, url, star, annotation, width, height, palette FROM items")
	if err != nil {
		return nil, err
	}
	for rows.Next() {
		var (
			url, annotation sql.NullString
			hash            string
			star, width, h  int64
			palette         sql.NullString
		)
		if err := rows.Scan(&hash, &url, &star, &annotation, &width, &h, &palette); err != nil {
			rows.Close()
			return nil, err
		}
		item := newItem(hash)
		item.URL = url.String
		item.Star = int(star)
		item.Annotation = annotation.String
		item.Width = int(width)
		item.Height = int(h)
		if palette.Valid {
			item.PaletteSet = true
			if palette.String != "" {
				_ = json.Unmarshal([]byte(palette.String), &item.Palette)
			}
			if item.Palette == nil {
				item.Palette = []PaletteEntry{}
			}
			item.rebuildPaletteLab()
		}
		items[hash] = item
	}
	if err := rows.Err(); err != nil {
		rows.Close()
		return nil, err
	}
	rows.Close()

	if err := m.loadLocations(items); err != nil {
		return nil, err
	}
	if err := m.loadTags(items); err != nil {
		return nil, err
	}
	if err := m.loadCategories(items); err != nil {
		return nil, err
	}

	out := make([]*Item, 0, len(items))
	for _, item := range items {
		// 无位置的元数据不进入索引（item 的存续由位置决定）。
		if len(item.Locations) == 0 {
			continue
		}
		out = append(out, item)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].ID < out[j].ID })
	return out, nil
}

func (m *MetadataDB) loadLocations(items map[string]*Item) error {
	rows, err := m.db.Query("SELECT hash, path, size, mtime FROM paths")
	if err != nil {
		return err
	}
	defer rows.Close()
	for rows.Next() {
		var hash, path string
		var size, mtime int64
		if err := rows.Scan(&hash, &path, &size, &mtime); err != nil {
			return err
		}
		if item, ok := items[hash]; ok {
			item.Locations = append(item.Locations, PathEntry{Path: path, Size: size, ModificationTime: mtime})
		}
	}
	return rows.Err()
}

func (m *MetadataDB) loadTags(items map[string]*Item) error {
	rows, err := m.db.Query("SELECT hash, tag FROM tags")
	if err != nil {
		return err
	}
	defer rows.Close()
	for rows.Next() {
		var hash, tag string
		if err := rows.Scan(&hash, &tag); err != nil {
			return err
		}
		if item, ok := items[hash]; ok {
			item.Tags = append(item.Tags, tag)
		}
	}
	return rows.Err()
}

func (m *MetadataDB) loadCategories(items map[string]*Item) error {
	rows, err := m.db.Query("SELECT hash, category FROM categories")
	if err != nil {
		return err
	}
	defer rows.Close()
	for rows.Next() {
		var hash, category string
		if err := rows.Scan(&hash, &category); err != nil {
			return err
		}
		if item, ok := items[hash]; ok {
			item.Categories = append(item.Categories, category)
		}
	}
	return rows.Err()
}

// SaveBatch 在单个事务中写入全部条目。
func (m *MetadataDB) SaveBatch(items []*Item) error {
	if len(items) == 0 {
		return nil
	}
	tx, err := m.db.Begin()
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback() }()

	for _, item := range items {
		var palette any
		if item.PaletteSet {
			data, err := json.Marshal(item.Palette)
			if err != nil {
				return err
			}
			palette = string(data)
		}
		if _, err := tx.Exec(
			`INSERT INTO items (hash, url, star, annotation, source_mtime, width, height, palette)
			 VALUES (?1, NULLIF(?2, ''), ?3, NULLIF(?4, ''), 0, ?5, ?6, ?7)
			 ON CONFLICT(hash) DO UPDATE SET url=NULLIF(?2, ''), star=?3, annotation=NULLIF(?4, ''),
			 source_mtime=0, width=?5, height=?6, palette=?7`,
			item.ID, item.URL, item.Star, item.Annotation, item.Width, item.Height, palette,
		); err != nil {
			return err
		}
		if _, err := tx.Exec("DELETE FROM paths WHERE hash=?1", item.ID); err != nil {
			return err
		}
		if _, err := tx.Exec("DELETE FROM tags WHERE hash=?1", item.ID); err != nil {
			return err
		}
		if _, err := tx.Exec("DELETE FROM categories WHERE hash=?1", item.ID); err != nil {
			return err
		}
		for _, loc := range item.Locations {
			if _, err := tx.Exec(
				"INSERT INTO paths (hash, path, size, mtime) VALUES (?1, ?2, ?3, ?4)",
				item.ID, loc.Path, loc.Size, loc.ModificationTime,
			); err != nil {
				return err
			}
		}
		for _, tag := range item.Tags {
			if _, err := tx.Exec("INSERT OR IGNORE INTO tags (hash, tag) VALUES (?1, ?2)", item.ID, tag); err != nil {
				return err
			}
		}
		for _, category := range item.Categories {
			if _, err := tx.Exec("INSERT OR IGNORE INTO categories (hash, category) VALUES (?1, ?2)", item.ID, category); err != nil {
				return err
			}
		}
	}
	return tx.Commit()
}

// Save 写入单条。
func (m *MetadataDB) Save(item *Item) error { return m.SaveBatch([]*Item{item}) }

// Delete 删除一条素材及其子表。
func (m *MetadataDB) Delete(id string) error {
	tx, err := m.db.Begin()
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback() }()
	for _, statement := range []string{
		"DELETE FROM items WHERE hash=?1",
		"DELETE FROM paths WHERE hash=?1",
		"DELETE FROM tags WHERE hash=?1",
		"DELETE FROM categories WHERE hash=?1",
	} {
		if _, err := tx.Exec(statement, id); err != nil {
			return err
		}
	}
	return tx.Commit()
}

// LoadFolderSnapshots 读取目录快照；读取失败按空表处理。
func (m *MetadataDB) LoadFolderSnapshots() map[string][2]int64 {
	out := make(map[string][2]int64)
	rows, err := m.db.Query("SELECT path, mtime, entries FROM folders")
	if err != nil {
		return out
	}
	defer rows.Close()
	for rows.Next() {
		var path string
		var mtime, entries int64
		if err := rows.Scan(&path, &mtime, &entries); err != nil {
			return out
		}
		out[path] = [2]int64{mtime, entries}
	}
	return out
}

// ReplaceFolderSnapshots 整体替换目录快照。
func (m *MetadataDB) ReplaceFolderSnapshots(snapshots map[string][2]int64) error {
	tx, err := m.db.Begin()
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback() }()
	if _, err := tx.Exec("DELETE FROM folders"); err != nil {
		return err
	}
	for path, values := range snapshots {
		if _, err := tx.Exec("INSERT INTO folders (path, mtime, entries) VALUES (?1, ?2, ?3)", path, values[0], values[1]); err != nil {
			return err
		}
	}
	return tx.Commit()
}

// NormalizeNameList 去除空项、去重并保留顺序。
func NormalizeNameList(names []string) []string {
	seen := make(map[string]struct{}, len(names))
	out := make([]string, 0, len(names))
	for _, name := range names {
		name = strings.TrimSpace(name)
		if name == "" {
			continue
		}
		if _, ok := seen[name]; ok {
			continue
		}
		seen[name] = struct{}{}
		out = append(out, name)
	}
	return out
}

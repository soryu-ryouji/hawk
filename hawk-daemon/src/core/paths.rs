//! 素材库路径工具：负责 .hawk/ 内部路径、库内相对路径换算与安全校验。
//! 索引与 API 层统一使用正斜杠相对路径（相对素材库根目录），如 "posters/2024/cat.jpg"。
//! 缩略图/元数据缓存是内容寻址的派生缓存，位于库外系统缓存目录，避免库在 iCloud/Dropbox
//! 等同步盘时 .hawk/ 膨胀拖累同步。
//! 库内路径布局（含缓存目录命名：库文件夹名_<根路径 SHA-256 前16位>）。

use sha2::{Digest, Sha256};

pub const HAWK_DIR_NAME: &str = ".hawk";
pub const TRASH_DIR_NAME: &str = "trash";
/// 回收站位置的路径前缀（相对库根目录）
pub const TRASH_PREFIX: &str = ".hawk/trash/";

#[derive(Clone)]
pub struct LibraryPaths {
    pub root: String,
    pub hawk_dir: String,
    pub metadata_dir: String,
    /// 缩略图派生缓存目录（库外系统缓存目录或测试覆盖）
    pub thumbnails_dir: String,
    /// SQLite 元数据缓存文件（库外系统缓存目录或测试覆盖）
    pub index_db_file: String,
    pub trash_dir: String,
    pub config_file: String,
    pub categories_file: String,
    pub tags_file: String,
    pub view_file: String,
    /// 全局列表隐藏项注册表（.hawk/global_filter.toml，参与同步）
    pub global_filter_file: String,
    /// 缓存目录（缩略图与 index.db 所在）：默认 <系统缓存>/hawk/cache/<库名>_<哈希>，
    /// 可经 --cache-parent 指定全局父目录（桌面端设置面板配置）
    pub cache_dir: String,
}

impl LibraryPaths {
    pub const HAWK_DIR_NAME: &'static str = ".hawk";
    pub const TRASH_DIR_NAME: &'static str = "trash";
}

impl LibraryPaths {
    /// cache_parent：缓存父目录覆盖（--cache-parent / 测试指向临时目录）；
    /// None 时用系统缓存目录下的 hawk/cache。库缓存子目录 <库名>_<路径哈希16位> 在其下拼接
    pub fn new(root: &str, cache_parent: Option<String>) -> LibraryPaths {
        let root = full_path(root);
        let hawk_dir = join_path(&root, HAWK_DIR_NAME);
        let metadata_dir = join_path(&hawk_dir, "metadata");
        let parent = cache_parent.unwrap_or_else(default_cache_parent_hawk);
        let cache_dir = join_path(&parent, &cache_dir_name(&root));
        let thumbnails_dir = join_path(&cache_dir, "thumbnails");
        let trash_dir = join_path(&hawk_dir, TRASH_DIR_NAME);
        let config_file = join_path(&hawk_dir, "config.toml");
        let categories_file = join_path(&hawk_dir, "categories.toml");
        let tags_file = join_path(&hawk_dir, "tags.toml");
        let view_file = join_path(&hawk_dir, "view.toml");
        let global_filter_file = join_path(&hawk_dir, "global_filter.toml");
        let index_db_file = join_path(&cache_dir, "index.db");
        LibraryPaths {
            root,
            hawk_dir,
            metadata_dir,
            index_db_file,
            thumbnails_dir,
            trash_dir,
            config_file,
            categories_file,
            tags_file,
            view_file,
            global_filter_file,
            cache_dir,
        }
    }

    /// 缓存目录与库根不得互相包含（缓存放进库内会被扫描/监听污染索引；库嵌进缓存目录同理）。
    /// 返回 None 表示位置合法；Some(原因) 供启动期拒绝
    pub fn cache_location_error(&self) -> Option<String> {
        if self.cache_dir == self.root || self.cache_dir.starts_with(&format!("{}/", self.root)) {
            return Some("缓存目录不能位于素材库内".to_string());
        }
        if self.root.starts_with(&format!("{}/", self.cache_dir)) {
            return Some("素材库不能位于缓存目录内".to_string());
        }
        None
    }

    /// 创建 .hawk/ 目录结构，并生成排除 trash 的 .gitignore（缺失的排除项会补上）
    pub fn ensure_layout(&self) {
        let _ = std::fs::create_dir_all(&self.metadata_dir);
        let _ = std::fs::create_dir_all(&self.thumbnails_dir);
        let _ = std::fs::create_dir_all(&self.trash_dir);

        let gitignore = join_path(&self.hawk_dir, ".gitignore");
        let existing = std::fs::read_to_string(&gitignore).unwrap_or_default();
        let lines: Vec<&str> = existing.lines().collect();
        if !lines.iter().any(|l| *l == "trash/") {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&gitignore) {
                let _ = writeln!(f, "trash/");
            }
        }
    }

    /// 绝对路径 → 库内相对路径（正斜杠）。库根目录返回 ""，不在库内时返回 None
    pub fn to_relative(&self, abs: &str) -> Option<String> {
        let abs = normalize_separators(abs);
        let rel = strip_prefix_components(&self.root, &abs);
        let rel = rel?;
        if rel.is_empty() {
            return Some(String::new());
        }
        let rel = rel.trim_start_matches('/');
        if rel.is_empty() {
            return Some(String::new());
        }
        if rel.split('/').any(|s| s == "..") {
            return None;
        }
        Some(rel.to_string())
    }

    /// 库内相对路径 → 绝对路径。含越界成分（..、绝对路径）时返回 None
    pub fn to_absolute(&self, rel: &str) -> Option<String> {
        if rel.trim().is_empty() || is_absolute_path(rel) {
            return None;
        }
        let segments: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
        if segments.is_empty() || segments.iter().any(|s| *s == "." || *s == "..") {
            return None;
        }
        let abs = full_path(&join_path(&self.root, &segments.join("/")));
        if abs == self.root || abs.starts_with(&(self.root.clone() + "/")) || abs.starts_with(&(self.root.clone() + "\\")) {
            Some(abs)
        } else {
            None
        }
    }

    /// 是否属于 .hawk/ 内部（回收站除外，回收站参与索引）
    pub fn is_internal(rel: &str) -> bool {
        if rel.starts_with(TRASH_PREFIX) {
            return false;
        }
        rel == HAWK_DIR_NAME || rel.starts_with(&format!("{HAWK_DIR_NAME}/"))
    }

    pub fn is_in_trash(rel: &str) -> bool {
        rel.starts_with(TRASH_PREFIX)
    }

    /// 回收站位置 → 原库内路径（去掉 .hawk/trash/ 前缀）
    pub fn trash_to_library_path(rel: &str) -> &str {
        rel.strip_prefix(TRASH_PREFIX).unwrap_or(rel)
    }

    /// 库内路径 → 回收站位置
    pub fn library_to_trash_path(rel: &str) -> String {
        format!("{TRASH_PREFIX}{rel}")
    }

    /// 校验 API 传入的库内相对路径：非空、不指向 .hawk 内部、不含越界成分
    pub fn is_valid_library_path(rel: Option<&str>) -> bool {
        let rel = match rel {
            Some(r) if !r.trim().is_empty() && !r.starts_with('/') && !r.contains('\\') => r,
            _ => return false,
        };
        let segments: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
        if segments.iter().any(|s| *s == "." || *s == "..") {
            return false;
        }
        !Self::is_internal(rel)
    }

    /// 取相对路径的父目录部分，根目录返回 ""
    pub fn dir_of(rel: &str) -> &str {
        match rel.rfind('/') {
            Some(i) => &rel[..i],
            None => "",
        }
    }

    /// 文件名（不含扩展名）
    pub fn name_of(rel: &str) -> &str {
        let file_name = &rel[rel.rfind('/').map(|i| i + 1).unwrap_or(0)..];
        match file_name.rfind('.') {
            Some(i) if i > 0 => &file_name[..i],
            _ => file_name,
        }
    }

    /// 扩展名，小写，不含点
    pub fn ext_of(rel: &str) -> String {
        let file_name = &rel[rel.rfind('/').map(|i| i + 1).unwrap_or(0)..];
        match file_name.rfind('.') {
            Some(i) if i > 0 => file_name[i + 1..].to_lowercase(),
            _ => String::new(),
        }
    }
}

/// Unix 毫秒时间戳
pub fn unix_ms(system_time: std::time::SystemTime) -> i64 {
    match system_time.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        Err(_) => 0,
    }
}

/// 文件的修改时间（Unix 毫秒）；读取失败返回 0
pub fn file_mtime_ms(path: &str) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(unix_ms)
        .unwrap_or(0)
}

// ---------- 纯路径工具 ----------

/// 规范化分隔符为正斜杠后的字符串比较前缀剥离；逐组件比较，大小写敏感
fn strip_prefix_components(prefix: &str, path: &str) -> Option<String> {
    let prefix = normalize_separators(prefix);
    let path = normalize_separators(path);
    if path == prefix {
        return Some(String::new());
    }
    let p = prefix.trim_end_matches('/');
    if let Some(rest) = path.strip_prefix(p) {
        let rest = rest.trim_start_matches('/');
        if rest.split('/').any(|s| s == "..") {
            return None;
        }
        // 仅当 path 确实以 prefix 的完整组件结尾时才算前缀匹配
        if path.len() > p.len() && path.as_bytes()[p.len()] == b'/' {
            return Some(rest.to_string());
        }
    }
    None
}

fn normalize_separators(p: &str) -> String {
    if std::path::MAIN_SEPARATOR == '\\' {
        p.replace('\\', "/")
    } else {
        p.to_string()
    }
}

fn is_absolute_path(p: &str) -> bool {
    let p = p.replace('\\', "/");
    p.starts_with('/') || p.chars().nth(1) == Some(':')
}

/// 简易 GetFullPath：绝对化 + 文本化归约 . 与 .. 组件（不解析符号链接，避免 Windows \\?\ 前缀）
pub fn full_path(p: &str) -> String {
    let p = p.replace('\\', "/");
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    let joined = out.join("/");
    if p.starts_with('/') {
        format!("/{joined}")
    } else {
        joined
    }
}

pub fn join_path(base: &str, child: &str) -> String {
    if base.ends_with('/') {
        format!("{base}{child}")
    } else {
        format!("{base}/{child}")
    }
}

/// 库外系统缓存目录父级（按平台）
fn default_cache_parent() -> String {
    #[cfg(target_os = "windows")]
    {
        std::env::var("LOCALAPPDATA").unwrap_or_else(|_| {
            full_path(&format!(
                "{}/AppData/Local",
                std::env::var("USERPROFILE").unwrap_or_default()
            ))
        })
    }
    #[cfg(target_os = "macos")]
    {
        format!("{}/Library/Application Support", std::env::var("HOME").unwrap_or_default())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
            format!("{}/.local/share", std::env::var("HOME").unwrap_or_default())
        })
    }
}

/// 默认缓存父目录（<系统缓存>/hawk/cache）：默认行为与历史版本一致，既有用户缓存路径不变
fn default_cache_parent_hawk() -> String {
    join_path(&default_cache_parent(), "hawk/cache")
}

/// 缓存子目录名：库文件夹名_路径哈希前16位（小写十六进制）
fn cache_dir_name(root: &str) -> String {
    format!("{}_{}", library_label(root), library_key(root))
}

/// 库文件夹名（缓存目录的可识别前缀）；非法字符清洗、末尾点/空格去除、截断 32 字符，空名兜底 library
fn library_label(root: &str) -> String {
    let trimmed = root.trim_end_matches('/');
    let name = trimmed.rsplit('/').next().unwrap_or(trimmed);
    if name.trim().is_empty() {
        return "library".to_string();
    }
    let cleaned: String = name
        .chars()
        .map(|c| if is_invalid_file_name_char(c) { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim_end_matches(['.', ' ']).to_string();
    if cleaned.is_empty() {
        return "library".to_string();
    }
    cleaned.chars().take(32).collect()
}

fn is_invalid_file_name_char(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') || (c.is_control())
}

/// 库标识：根路径的 SHA-256 前 16 位（小写十六进制），保证多库/同名库缓存目录唯一
pub fn library_key(root: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(root.as_bytes());
    let digest = hasher.finalize();
    let mut s = String::with_capacity(16);
    for b in &digest[..8] {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_parent_override_and_location_check() {
        // 默认：系统缓存目录下的 hawk/cache/<库名>_<哈希>（与历史版本一致）
        let paths = LibraryPaths::new("/data/library", None);
        assert!(paths.cache_dir.contains("hawk/cache/library_"));
        assert!(paths.cache_location_error().is_none());

        // 父目录覆盖：库子目录在其下拼接
        let paths = LibraryPaths::new("/data/library", Some("/fast/cache".to_string()));
        assert!(paths.cache_dir.starts_with("/fast/cache/library_"));
        assert_eq!(paths.thumbnails_dir, format!("{}/thumbnails", paths.cache_dir));
        assert_eq!(paths.index_db_file, format!("{}/index.db", paths.cache_dir));
        assert!(paths.cache_location_error().is_none());

        // 互斥：缓存父目录落在库内 → 拼接后的库缓存子目录也在库内，拒绝
        let bad = LibraryPaths::new("/data/library", Some("/data/library/cache".to_string()));
        assert!(bad.cache_location_error().is_some());
        // 库根直接作父目录同样拒绝（拼接后仍在库内）
        let bad = LibraryPaths::new("/data/library", Some("/data/library".to_string()));
        assert!(bad.cache_location_error().is_some());
        // 缓存在库的同级、库在缓存的同级均合法（斜杠边界前缀比较）
        let ok = LibraryPaths::new("/data/cache/library", Some("/data/cache".to_string()));
        assert!(ok.cache_location_error().is_none());
    }

    #[test]
    fn relative_roundtrip() {
        let paths = LibraryPaths::new("/data/library", Some("/tmp/cache".to_string()));
        assert_eq!(paths.to_relative("/data/library"), Some(String::new()));
        assert_eq!(
            paths.to_relative("/data/library/posters/2024/cat.jpg"),
            Some("posters/2024/cat.jpg".to_string())
        );
        assert_eq!(paths.to_relative("/data/other/cat.jpg"), None);
        assert_eq!(
            paths.to_absolute("posters/2024/cat.jpg"),
            Some("/data/library/posters/2024/cat.jpg".to_string())
        );
        assert_eq!(paths.to_absolute("../escape.jpg"), None);
        assert_eq!(paths.to_absolute("/abs/cat.jpg"), None);
        assert_eq!(paths.to_absolute("a/./b.jpg"), None);
    }

    #[test]
    fn trash_helpers() {
        assert!(LibraryPaths::is_in_trash(".hawk/trash/posters/cat.jpg"));
        assert!(!LibraryPaths::is_internal(".hawk/trash/posters/cat.jpg"));
        assert!(LibraryPaths::is_internal(".hawk/metadata/x.toml"));
        assert_eq!(
            LibraryPaths::trash_to_library_path(".hawk/trash/posters/cat.jpg"),
            "posters/cat.jpg"
        );
        assert_eq!(
            LibraryPaths::library_to_trash_path("posters/cat.jpg"),
            ".hawk/trash/posters/cat.jpg"
        );
    }

    #[test]
    fn name_and_ext() {
        assert_eq!(LibraryPaths::name_of("posters/sunset-photo.jpg"), "sunset-photo");
        assert_eq!(LibraryPaths::ext_of("posters/sunset-photo.JPG"), "jpg");
        assert_eq!(LibraryPaths::ext_of("noext"), "");
        assert_eq!(LibraryPaths::name_of(".hidden"), ".hidden");
        assert_eq!(LibraryPaths::dir_of("posters/2024/cat.jpg"), "posters/2024");
        assert_eq!(LibraryPaths::dir_of("cat.jpg"), "");
    }

    #[test]
    fn library_key_is_sha256_prefix() {
        // 对根路径字符串做 SHA-256，取前 16 位小写 hex
        assert_eq!(library_key("/data/library").len(), 16);
    }
}

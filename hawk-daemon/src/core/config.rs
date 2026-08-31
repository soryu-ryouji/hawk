//! 项目配置（.hawk/config.toml）。由文件监听触发 Reload，索引流水线在配置变更后全量比对。
//! 库配置：ignore 模式无 "/" 时匹配任意深度同名项；Matcher 默认序数忽略大小写。

use crate::core::paths::LibraryPaths;
use std::sync::RwLock;

#[derive(Clone, Default)]
pub struct WebSettings {
    pub enabled: bool,
    pub port: u16,
    pub token: Option<String>,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub name: Option<String>,
    pub ignore: Vec<String>,
    pub web: WebSettings,
}

pub struct LibraryConfig {
    paths: LibraryPaths,
    current: RwLock<Snapshot>,
    matcher: RwLock<IgnoreMatcher>,
}

impl LibraryConfig {
    pub fn new(paths: LibraryPaths) -> LibraryConfig {
        ensure_default(&paths);
        let snapshot = load(&paths);
        let matcher = IgnoreMatcher::build(&snapshot.ignore);
        LibraryConfig {
            paths,
            current: RwLock::new(snapshot),
            matcher: RwLock::new(matcher),
        }
    }

    pub fn current(&self) -> Snapshot {
        self.current.read().unwrap().clone()
    }

    pub fn reload(&self) {
        let snapshot = load(&self.paths);
        let matcher = IgnoreMatcher::build(&snapshot.ignore);
        *self.current.write().unwrap() = snapshot;
        *self.matcher.write().unwrap() = matcher;
    }

    /// 相对路径是否被 ignore 规则命中（仅用于库内文件，回收站不参与）
    pub fn is_ignored(&self, rel: &str) -> bool {
        self.matcher.read().unwrap().is_ignored(rel)
    }

    /// 启动期静态读取 [web] 段；运行时读取走 current().web
    pub fn peek_web(library_root: &str) -> WebSettings {
        let paths = LibraryPaths::new(library_root, None);
        if std::path::Path::new(&paths.config_file).is_file() {
            parse_web(&paths.config_file)
        } else {
            WebSettings::default()
        }
    }
}

/// 库首次打开时生成带注释的默认 config.toml（已存在则不覆盖）
fn ensure_default(paths: &LibraryPaths) {
    if std::path::Path::new(&paths.config_file).is_file() {
        return;
    }
    if let Some(parent) = std::path::Path::new(&paths.config_file).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&paths.config_file, DEFAULT_CONFIG_TEXT);
}

const DEFAULT_CONFIG_TEXT: &str = r#"# hawk 项目配置（.hawk/config.toml，按素材库隔离、随库同步）
# name / ignore 保存即热更（文件监听 Reload）
# [web] 段的端口/绑定/token 保存后由桌面端重启服务生效

# 素材库显示名（缺省为库目录名）
# name = "我的素材库"

# 索引时忽略的路径（不含 "/" 的模式匹配任意深度同名项）
ignore = []

# 局域网 web 查看（只读；桌面端设置面板读写）
[web]
enabled = false
port = 27372
token = ""
"#;

fn load(paths: &LibraryPaths) -> Snapshot {
    let mut snapshot = Snapshot::default();
    if !std::path::Path::new(&paths.config_file).is_file() {
        return snapshot;
    }
    let text = match std::fs::read_to_string(&paths.config_file) {
        Ok(t) => t,
        Err(_) => return snapshot,
    };
    let table: toml::Value = match toml::from_str(&text) {
        Ok(t) => t,
        Err(_) => return snapshot,
    };
    if let Some(name) = table.get("name").and_then(|v| v.as_str()) {
        snapshot.name = Some(name.to_string());
    }
    if let Some(ignore) = table.get("ignore").and_then(|v| v.as_array()) {
        snapshot.ignore = ignore.iter().filter_map(|v| v.as_str().map(String::from)).collect();
    }
    snapshot.web = table.get("web").map(parse_web_value).unwrap_or_default();
    snapshot
}

/// 解析 [web] 段；缺段/非法值回退默认
pub fn parse_web(config_file: &str) -> WebSettings {
    let text = match std::fs::read_to_string(config_file) {
        Ok(t) => t,
        Err(_) => return WebSettings::default(),
    };
    let table: toml::Value = match toml::from_str(&text) {
        Ok(t) => t,
        Err(_) => return WebSettings::default(),
    };
    table.get("web").map(parse_web_value).unwrap_or_default()
}

fn parse_web_value(value: &toml::Value) -> WebSettings {
    let mut web = WebSettings::default();
    if let Some(table) = value.as_table() {
        web.enabled = table.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
        if let Some(port) = table.get("port").and_then(|v| v.as_integer()) {
            if (1..=65535).contains(&port) {
                web.port = port as u16;
            }
        }
        if let Some(token) = table.get("token").and_then(|v| v.as_str()) {
            let token = token.trim();
            if !token.is_empty() {
                web.token = Some(token.to_string());
            }
        }
    }
    web
}

// ---------- ignore 匹配器 ----------

pub struct IgnoreMatcher {
    /// 预展开的分段模式（小写）：无 "/" 的模式 → [**/p, **/p/**]；含 "/" → [p, p/**]
    patterns: Vec<Vec<String>>,
}

impl IgnoreMatcher {
    pub fn build(patterns: &[String]) -> IgnoreMatcher {
        let mut expanded = Vec::new();
        for raw in patterns {
            let raw = raw.trim();
            if raw.is_empty() {
                continue;
            }
            let raw = raw.to_lowercase();
            if raw.contains('/') {
                expanded.push(split_segments(&raw));
                let mut with_children = raw.clone();
                with_children.push_str("/**");
                expanded.push(split_segments(&with_children));
            } else {
                expanded.push(split_segments(&format!("**/{raw}")));
                expanded.push(split_segments(&format!("**/{raw}/**")));
            }
        }
        IgnoreMatcher { patterns: expanded }
    }

    pub fn is_ignored(&self, rel: &str) -> bool {
        let rel = rel.to_lowercase();
        let segments = split_segments(&rel);
        self.patterns.iter().any(|p| glob_segments(p, &segments))
    }
}

fn split_segments(path: &str) -> Vec<String> {
    path.split('/').filter(|s| !s.is_empty()).map(String::from).collect()
}

/// 分段 glob：`**` 跨目录，段内 `*`/`?` 不跨 '/'。大小写需由调用方归一
fn glob_segments(pattern: &[String], path: &[String]) -> bool {
    // 经典双指针回溯
    let (mut p, mut s) = (0usize, 0usize);
    let mut star_p: Option<usize> = None;
    let mut star_s = 0usize;
    while s < path.len() {
        if p < pattern.len() && (pattern[p] == "**" || segment_match(&pattern[p], &path[s])) {
            if pattern[p] == "**" {
                star_p = Some(p);
                star_s = s;
                p += 1;
                continue;
            }
            p += 1;
            s += 1;
        } else if let Some(sp) = star_p {
            p = sp + 1;
            star_s += 1;
            s = star_s;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == "**" {
        p += 1;
    }
    p == pattern.len()
}

/// 单段通配：* 任意字符序列，? 单字符
fn segment_match(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star_pi: Option<usize> = None;
    let mut star_ti = 0usize;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star_pi = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(patterns: &[&str]) -> IgnoreMatcher {
        IgnoreMatcher::build(&patterns.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn bare_name_matches_any_depth() {
        let matcher = m(&["node_modules"]);
        assert!(matcher.is_ignored("node_modules"));
        assert!(matcher.is_ignored("a/b/node_modules"));
        assert!(matcher.is_ignored("a/b/node_modules/x/y.js"));
        assert!(!matcher.is_ignored("a/node_modules.txt"));
    }

    #[test]
    fn wildcard_bare_pattern() {
        let matcher = m(&["*.tmp"]);
        assert!(matcher.is_ignored("a/b/x.tmp"));
        assert!(!matcher.is_ignored("a/b/x.png"));
    }

    #[test]
    fn slash_pattern_matches_subtree() {
        let matcher = m(&["posters/2024"]);
        assert!(matcher.is_ignored("posters/2024"));
        assert!(matcher.is_ignored("posters/2024/cat.jpg"));
        assert!(!matcher.is_ignored("posters/2025/cat.jpg"));
    }

    #[test]
    fn case_insensitive() {
        let matcher = m(&["Node_Modules"]);
        assert!(matcher.is_ignored("a/node_modules/x.js"));
    }
}

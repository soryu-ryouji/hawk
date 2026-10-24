//! 项目配置（.hawk/config.toml）。由文件监听触发 Reload，索引流水线在配置变更后全量比对。
//! 库配置：ignore 模式无 "/" 时匹配任意深度同名项；Matcher 默认序数忽略大小写。
//! [web] 段保存即热生效（LAN 监听 supervisor 运行期重绑，见 api/lan.rs）；
//! 每个局域网访问密钥独立携带读/写权限，鉴权中间件每请求经 current().web 查找并判定，热生效。

use crate::core::paths::LibraryPaths;
use std::collections::HashSet;
use std::sync::RwLock;

/// 局域网访问密钥：明文 token 与配置一起存于库内；
/// 列表 API 仅 admin 可读，渲染层可复制给浏览器/移动端使用。
#[derive(Clone, Default, PartialEq)]
pub struct WebAccessKey {
    pub id: String,
    pub label: String,
    pub token: String,
    /// true = 该密钥允许写操作；false = 只读
    pub write: bool,
    pub created_at_ms: u64,
    /// 配对设备元数据：用于目标端管理列表展示对端信息（网站密钥为空）。
    pub peer_id: Option<String>,
    pub library_name: Option<String>,
    pub url: Option<String>,
}

#[derive(Clone, PartialEq)]
pub struct WebSettings {
    pub enabled: bool,
    /// 局域网网站访问（浏览器/网页版）密钥
    pub keys: Vec<WebAccessKey>,
    pub port: u16,
    /// hawk-app 之间直连共享使用的密钥；非空即允许局域网 app 配对，不依赖 enabled
    pub peer_keys: Vec<WebAccessKey>,
    /// hawk-app 直连 API 端口（与网站端口分离）
    pub peer_port: u16,
}

impl Default for WebSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 27372,
            keys: Vec::new(),
            peer_keys: Vec::new(),
            peer_port: 27375,
        }
    }
}

/// 周期兜底扫描设置（库级，随库同步）：监听漏事件时的最终一致保证
#[derive(Clone, PartialEq)]
pub struct ScanSettings {
    /// 是否开启周期兜底重扫
    pub periodic: bool,
    /// 重扫间隔（秒），下限 60
    pub interval_seconds: u64,
}

impl Default for ScanSettings {
    fn default() -> Self {
        ScanSettings {
            periodic: true,
            interval_seconds: 900,
        }
    }
}

/// reload 前后差异：调用方据此决定后续动作（ignore / 扩展名白名单变化 → 重扫，web 变化 → LAN 重绑）
#[derive(Default)]
pub struct ConfigChange {
    pub ignore_changed: bool,
    /// 扩展名白名单变化：需强制重扫以收敛既有条目（新增的后缀入库、移除的后缀出库）
    pub extensions_changed: bool,
    /// 周期兜底扫描设置变化（消费循环的定时检查自读配置，仅用于日志/事件语义）
    pub scan_changed: bool,
    pub web_changed: bool,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub name: Option<String>,
    pub ignore: Vec<String>,
    /// 可见扩展名白名单（空 = 不过滤，全部入库）
    pub extensions: Vec<String>,
    /// 周期兜底扫描设置
    pub scan: ScanSettings,
    pub web: WebSettings,
}

pub struct LibraryConfig {
    paths: LibraryPaths,
    current: RwLock<Snapshot>,
    matcher: RwLock<IgnoreMatcher>,
    extensions: RwLock<ExtensionFilter>,
    /// 最近一次加载/重载的错误：解析失败时保留上次有效配置继续运行（正常为 None）
    last_error: RwLock<Option<String>>,
}

impl LibraryConfig {
    pub fn new(paths: LibraryPaths) -> LibraryConfig {
        ensure_default(&paths);
        // 启动期解析失败没有「上次有效配置」可回退：用默认值继续，并记录错误（app/status 暴露）
        let (snapshot, error) = match load(&paths) {
            Ok(snapshot) => (snapshot, None),
            Err(e) => {
                tracing::warn!("配置加载失败，按默认配置继续: {e}");
                (Snapshot::default(), Some(e))
            }
        };
        let matcher = IgnoreMatcher::build(&snapshot.ignore);
        let extensions = ExtensionFilter::build(&snapshot.extensions);
        LibraryConfig {
            paths,
            current: RwLock::new(snapshot),
            matcher: RwLock::new(matcher),
            extensions: RwLock::new(extensions),
            last_error: RwLock::new(error),
        }
    }

    /// 最近一次配置加载/重载的错误（解析失败时保留上次有效配置继续运行）；正常为 None
    pub fn config_error(&self) -> Option<String> {
        self.last_error.read().unwrap().clone()
    }

    pub fn current(&self) -> Snapshot {
        self.current.read().unwrap().clone()
    }

    /// 重读配置文件并重建 matcher/扩展名过滤，返回前后差异。
    /// 解析失败时保留上次有效配置（返回空差异），错误经 config_error 暴露——
    /// 用户手滑写错一个字符不该让 ignore/白名单/局域网配置静默失效
    pub fn reload(&self) -> ConfigChange {
        let snapshot = match load(&self.paths) {
            Ok(snapshot) => snapshot,
            Err(e) => {
                tracing::warn!("配置重载失败，保留上次有效配置: {e}");
                *self.last_error.write().unwrap() = Some(e);
                return ConfigChange::default();
            }
        };
        *self.last_error.write().unwrap() = None;
        let matcher = IgnoreMatcher::build(&snapshot.ignore);
        let extensions = ExtensionFilter::build(&snapshot.extensions);
        let change = {
            let mut cur = self.current.write().unwrap();
            let change = ConfigChange {
                ignore_changed: cur.ignore != snapshot.ignore,
                extensions_changed: cur.extensions != snapshot.extensions,
                scan_changed: cur.scan != snapshot.scan,
                web_changed: cur.web != snapshot.web,
            };
            *cur = snapshot;
            change
        };
        *self.matcher.write().unwrap() = matcher;
        *self.extensions.write().unwrap() = extensions;
        change
    }

    /// 写回 name 键并热更：toml_edit 就地改/删键，保留文件其余段与注释。
    /// 原子写（临时文件 + rename，update_web 同款）；None/空白清除自定义名（回退库目录名）
    pub fn update_name(&self, name: Option<&str>) -> Result<(), String> {
        let file = &self.paths.config_file;
        let text =
            std::fs::read_to_string(file).map_err(|e| format!("读取配置失败 {file}: {e}"))?;
        let mut doc = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| format!("配置解析失败 {file}: {e}"))?;
        match name.map(str::trim).filter(|s| !s.is_empty()) {
            Some(n) => doc["name"] = toml_edit::value(n),
            None => {
                doc.as_table_mut().remove("name");
            }
        }
        let tmp = format!("{file}.tmp");
        std::fs::write(&tmp, doc.to_string()).map_err(|e| format!("配置写入失败 {tmp}: {e}"))?;
        if let Err(e) = std::fs::rename(&tmp, file) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("配置替换失败 {file}: {e}"));
        }
        // name 变化无后续动作（library/info 每次读 current()），reload 仅刷新快照
        self.reload();
        Ok(())
    }

    /// 相对路径是否被 ignore 规则命中（仅用于库内文件，回收站不参与）
    pub fn is_ignored(&self, rel: &str) -> bool {
        self.matcher.read().unwrap().is_ignored(rel)
    }

    /// 扩展名是否在白名单内（白名单为空 = 全部放行；无扩展名的文件在白名单非空时排除）。
    /// 回收站不参与过滤（由调用方按 in_trash 区分），保证已回收的素材仍可见可恢复
    pub fn is_extension_included(&self, rel: &str) -> bool {
        self.extensions.read().unwrap().allows(rel)
    }

    /// 周期兜底扫描设置（消费循环定时检查时读取；热更即生效）
    pub fn scan_settings(&self) -> ScanSettings {
        self.current.read().unwrap().scan.clone()
    }

    /// 写回 [scan] 段并热更（toml_edit 保注释；原子写），返回前后差异
    pub fn update_scan(&self, scan: &ScanSettings) -> Result<ConfigChange, String> {
        let file = &self.paths.config_file;
        let text =
            std::fs::read_to_string(file).map_err(|e| format!("读取配置失败 {file}: {e}"))?;
        let mut doc = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| format!("配置解析失败 {file}: {e}"))?;
        doc["scan"]["periodic"] = toml_edit::value(scan.periodic);
        doc["scan"]["interval"] = toml_edit::value(scan.interval_seconds.max(60) as i64);
        let tmp = format!("{file}.tmp");
        std::fs::write(&tmp, doc.to_string()).map_err(|e| format!("配置写入失败 {tmp}: {e}"))?;
        if let Err(e) = std::fs::rename(&tmp, file) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("配置替换失败 {file}: {e}"));
        }
        Ok(self.reload())
    }

    /// 文件是否应进入索引：ignore 未命中且扩展名在白名单内。
    /// 目录只用 is_ignored（目录无扩展名语义）
    pub fn is_file_included(&self, rel: &str) -> bool {
        !self.is_ignored(rel) && self.is_extension_included(rel)
    }

    /// 写回 [web] 段并热更：toml_edit 就地改键值，保留文件其余段与注释。
    /// 原子写（临时文件 + rename），成功后 reload 返回前后差异（调用方据此 wake LAN supervisor）
    pub fn update_web(&self, web: &WebSettings) -> Result<ConfigChange, String> {
        let file = &self.paths.config_file;
        let text =
            std::fs::read_to_string(file).map_err(|e| format!("读取配置失败 {file}: {e}"))?;
        let mut doc = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| format!("配置解析失败 {file}: {e}"))?;
        let table = doc["web"]
            .as_table_mut()
            .ok_or_else(|| format!("配置格式错误 {file}：[web] 必须是普通表"))?;
        // 已存在的键只换值（键上的注释装饰保留），缺失的键自动创建。
        table["enabled"] = toml_edit::value(web.enabled);
        table["port"] = toml_edit::value(i64::from(web.port));
        table["peer_port"] = toml_edit::value(i64::from(web.peer_port));

        set_web_keys(table, "keys", &web.keys);
        set_web_keys(table, "peer_keys", &web.peer_keys);
        let output = doc.to_string();
        // 写盘前先做一次完整 TOML 解析，避免留下不可解析文本。
        toml::from_str::<toml::Value>(&output)
            .map_err(|e| format!("配置序列化校验失败 {file}: {e}"))?;
        let tmp = format!("{file}.tmp");
        std::fs::write(&tmp, output).map_err(|e| format!("配置写入失败 {tmp}: {e}"))?;
        if let Err(e) = std::fs::rename(&tmp, file) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("配置替换失败 {file}: {e}"));
        }
        let change = self.reload();
        if let Some(error) = self.config_error() {
            return Err(error);
        }
        // 防止序列化层再次静默丢字段：落盘并 reload 后必须与调用方提交的 WebSettings 一致。
        if self.current().web != *web {
            return Err("配置写回后校验失败：web 段未完整持久化".to_string());
        }
        Ok(change)
    }
}

fn set_web_keys(table: &mut toml_edit::Table, field: &str, keys: &[WebAccessKey]) {
    if keys.is_empty() {
        table.remove(field);
    } else {
        table[field] = keys_to_toml(keys);
    }
}

fn keys_to_toml(keys: &[WebAccessKey]) -> toml_edit::Item {
    let mut tables = toml_edit::ArrayOfTables::new();
    for key in keys {
        let mut table = toml_edit::Table::new();
        table["id"] = toml_edit::value(key.id.as_str());
        table["label"] = toml_edit::value(key.label.as_str());
        table["token"] = toml_edit::value(key.token.as_str());
        table["write"] = toml_edit::value(key.write);
        table["created_at_ms"] = toml_edit::value(key.created_at_ms as i64);
        if let Some(peer_id) = &key.peer_id {
            table["peer_id"] = toml_edit::value(peer_id.as_str());
        }
        if let Some(library_name) = &key.library_name {
            table["library_name"] = toml_edit::value(library_name.as_str());
        }
        if let Some(url) = &key.url {
            table["url"] = toml_edit::value(url.as_str());
        }
        tables.push(table);
    }
    toml_edit::Item::ArrayOfTables(tables)
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
# [web] 段的端口/token 保存即热生效（局域网监听运行期重绑，无需重启）

# 素材库显示名（缺省为库目录名）
# name = "我的素材库"

# 索引时忽略的路径（不含 "/" 的模式匹配任意深度同名项）
ignore = []

# 可见扩展名白名单：只索引这些后缀的文件（空数组 = 不过滤，全部入库）。
# 改动即热生效并触发重扫，既有条目随之收敛（白名单外的素材从索引移除，文件本身不动）。
# extensions = ["jpg", "jpeg", "png", "gif", "webp", "bmp", "tiff", "avif"]

# 周期兜底重扫：监听可能静默丢事件（尤其 macOS FSEvents），开启后每 interval 秒强制遍历一次
# （只 stat 不读文件内容，按 size/mtime 复用哈希）。关闭后仍由实时监听捕获增删改，
# 仅「监听漏掉」的部分需要手动右键「重新扫描」收敛
[scan]
periodic = true
interval = 900

# 局域网网站访问（桌面端设置面板读写）。
# 访问密钥由设置面板创建，每把密钥独立读/写权限；密钥明文随库配置保存，请勿随库公开分享。
[web]
enabled = false
port = 27372
# [[web.keys]]
# id = "..."
# label = "我的浏览器"
# token = "..."
# write = false
# created_at_ms = 0

"#;

/// 读取并解析 config.toml；文件不存在视为新库（默认快照，非错误），
/// 读取/解析失败返回 Err（调用方保留上次有效配置）
fn load(paths: &LibraryPaths) -> Result<Snapshot, String> {
    let mut snapshot = Snapshot::default();
    let file = &paths.config_file;
    if !std::path::Path::new(file).is_file() {
        return Ok(snapshot);
    }
    let text = std::fs::read_to_string(file).map_err(|e| format!("读取配置失败 {file}: {e}"))?;
    let table: toml::Value =
        toml::from_str(&text).map_err(|e| format!("配置解析失败 {file}: {e}"))?;
    if let Some(name) = table.get("name").and_then(|v| v.as_str()) {
        snapshot.name = Some(name.to_string());
    }
    if let Some(ignore) = table.get("ignore").and_then(|v| v.as_array()) {
        snapshot.ignore = ignore
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
    }
    if let Some(extensions) = table.get("extensions").and_then(|v| v.as_array()) {
        snapshot.extensions = extensions
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
    }
    if let Some(scan) = table.get("scan").and_then(|v| v.as_table()) {
        if let Some(periodic) = scan.get("periodic").and_then(|v| v.as_bool()) {
            snapshot.scan.periodic = periodic;
        }
        if let Some(interval) = scan.get("interval").and_then(|v| v.as_integer()) {
            snapshot.scan.interval_seconds = interval.max(60) as u64;
        }
    }
    snapshot.web = table.get("web").map(parse_web_value).unwrap_or_default();
    Ok(snapshot)
}

fn parse_web_keys(value: Option<&toml::Value>) -> Vec<WebAccessKey> {
    let Some(keys) = value.and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    keys.iter()
        .filter_map(|value| {
            let key = value.as_table()?;
            let id = key.get("id").and_then(|v| v.as_str()).unwrap_or("").trim();
            let token = key
                .get("token")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if id.is_empty() || token.is_empty() {
                return None;
            }
            Some(WebAccessKey {
                id: id.to_string(),
                label: key
                    .get("label")
                    .and_then(|v| v.as_str())
                    .unwrap_or("访问密钥")
                    .trim()
                    .to_string(),
                token: token.to_string(),
                write: key.get("write").and_then(|v| v.as_bool()).unwrap_or(false),
                created_at_ms: key
                    .get("created_at_ms")
                    .and_then(|v| v.as_integer())
                    .unwrap_or(0)
                    .max(0) as u64,
                peer_id: key
                    .get("peer_id")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string),
                library_name: key
                    .get("library_name")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string),
                url: key
                    .get("url")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string),
            })
        })
        .collect()
}

fn parse_web_value(value: &toml::Value) -> WebSettings {
    let mut web = WebSettings::default();
    if let Some(table) = value.as_table() {
        web.enabled = table
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if let Some(port) = table.get("port").and_then(|v| v.as_integer()) {
            if (1..=65535).contains(&port) {
                web.port = port as u16;
            }
        }
        if let Some(port) = table.get("peer_port").and_then(|v| v.as_integer()) {
            if (1..=65535).contains(&port) {
                web.peer_port = port as u16;
            }
        }
        web.keys = parse_web_keys(table.get("keys"));
        web.peer_keys = parse_web_keys(table.get("peer_keys"));
    }
    web
}

// ---------- 扩展名白名单 ----------

/// 可见扩展名白名单（空 = 不过滤）。构建时归一：trim、去前导点、小写、去空项
pub struct ExtensionFilter {
    allowed: HashSet<String>,
}

impl ExtensionFilter {
    pub fn build(list: &[String]) -> ExtensionFilter {
        let allowed = list
            .iter()
            .map(|raw| raw.trim().trim_start_matches('.').to_lowercase())
            .filter(|e| !e.is_empty())
            .collect();
        ExtensionFilter { allowed }
    }

    /// 路径的扩展名是否放行（ext_of 已小写；无扩展名文件在白名单非空时排除）
    pub fn allows(&self, rel: &str) -> bool {
        if self.allowed.is_empty() {
            return true;
        }
        let ext = LibraryPaths::ext_of(rel);
        !ext.is_empty() && self.allowed.contains(&ext)
    }
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
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
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

    #[test]
    fn extension_filter_allows_all_when_empty() {
        let f = ExtensionFilter::build(&[]);
        assert!(f.allows("a/b/x.png"));
        assert!(f.allows("noext"));
    }

    #[test]
    fn extension_filter_normalizes_and_matches() {
        let f = ExtensionFilter::build(&[".PNG".to_string(), " jpg ".to_string()]);
        assert!(f.allows("a/b/x.png"));
        assert!(f.allows("x.JPG"));
        assert!(!f.allows("x.gif"));
        assert!(!f.allows("noext"));
        assert!(!f.allows("a.b/c"));
    }

    #[test]
    fn extensions_change_detected_and_applied() {
        let dir = std::env::temp_dir().join(format!("hawk-cfg-ext-test-{}", std::process::id()));
        let root = dir.join("lib");
        std::fs::create_dir_all(&root).unwrap();
        let paths = LibraryPaths::new(root.to_str().unwrap(), None);
        let cfg = LibraryConfig::new(paths.clone());
        assert!(cfg.is_file_included("a.png"), "默认无白名单");

        std::fs::write(&paths.config_file, "extensions = [\"png\"]\n").unwrap();
        let change = cfg.reload();
        assert!(change.extensions_changed);
        assert!(!change.ignore_changed);
        assert!(cfg.is_file_included("a.png"));
        assert!(!cfg.is_file_included("a.txt"));
        assert!(!cfg.is_file_included("noext"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_settings_parse_and_update() {
        let dir = std::env::temp_dir().join(format!("hawk-cfg-scan-test-{}", std::process::id()));
        let root = dir.join("lib");
        std::fs::create_dir_all(&root).unwrap();
        let paths = LibraryPaths::new(root.to_str().unwrap(), None);
        let cfg = LibraryConfig::new(paths.clone());
        // 默认模板：periodic = true / 900s
        assert!(cfg.scan_settings().periodic);
        assert_eq!(cfg.scan_settings().interval_seconds, 900);

        // 写回：关闭 + 下限夹紧（10 → 60）
        let change = cfg
            .update_scan(&ScanSettings {
                periodic: false,
                interval_seconds: 10,
            })
            .unwrap();
        assert!(change.scan_changed);
        assert!(!cfg.scan_settings().periodic);
        assert_eq!(cfg.scan_settings().interval_seconds, 60);
        let text = std::fs::read_to_string(&paths.config_file).unwrap();
        assert!(text.contains("periodic = false"));
        assert!(text.contains("[web]"), "其余段保留");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_error_keeps_previous_config() {
        let dir = std::env::temp_dir().join(format!("hawk-cfg-err-test-{}", std::process::id()));
        let root = dir.join("lib");
        std::fs::create_dir_all(&root).unwrap();
        let paths = LibraryPaths::new(root.to_str().unwrap(), None);
        std::fs::create_dir_all(root.join(".hawk")).unwrap();
        std::fs::write(
            &paths.config_file,
            "extensions = [\"png\"]\nname = \"有效\"\n",
        )
        .unwrap();
        let cfg = LibraryConfig::new(paths.clone());
        assert_eq!(cfg.config_error(), None);
        assert!(cfg.is_file_included("a.png") && !cfg.is_file_included("a.txt"));

        // 语法错误：保留上次有效配置，记录错误
        std::fs::write(&paths.config_file, "extensions = [\"png\"\n").unwrap();
        let change = cfg.reload();
        assert!(!change.extensions_changed && !change.ignore_changed && !change.web_changed);
        assert!(cfg.config_error().is_some());
        assert!(
            cfg.is_file_included("a.png") && !cfg.is_file_included("a.txt"),
            "旧配置仍生效"
        );
        assert_eq!(cfg.current().name.as_deref(), Some("有效"));

        // 修好后错误清除、新配置生效
        std::fs::write(&paths.config_file, "extensions = [\"txt\"]\n").unwrap();
        assert!(cfg.reload().extensions_changed);
        assert_eq!(cfg.config_error(), None);
        assert!(!cfg.is_file_included("a.png") && cfg.is_file_included("a.txt"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_name_writes_and_clears() {
        let dir = std::env::temp_dir().join(format!("hawk-cfg-test-{}", std::process::id()));
        let root = dir.join("lib");
        std::fs::create_dir_all(root.join(".hawk")).unwrap();
        let paths = LibraryPaths::new(root.to_str().unwrap(), None);
        let cfg = LibraryConfig::new(paths);

        // 改名生效且保留其余段与注释
        cfg.update_name(Some("  我的库  ")).unwrap();
        let text = std::fs::read_to_string(root.join(".hawk/config.toml")).unwrap();
        assert!(text.contains("name = \"我的库\""));
        assert!(text.contains("[web]"));
        assert!(text.contains("# 索引时忽略的路径"));
        assert_eq!(cfg.current().name.as_deref(), Some("我的库"));

        // 空白清除自定义名（回退目录名）
        cfg.update_name(Some(" ")).unwrap();
        let text = std::fs::read_to_string(root.join(".hawk/config.toml")).unwrap();
        assert!(!text.lines().any(|l| l.starts_with("name")));
        assert_eq!(cfg.current().name, None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_web_persists_keys() {
        let dir = std::env::temp_dir().join(format!("hawk-cfg-web-keys-{}", std::process::id()));
        let root = dir.join("lib");
        std::fs::create_dir_all(root.join(".hawk")).unwrap();
        let paths = LibraryPaths::new(root.to_str().unwrap(), None);
        std::fs::write(
            &paths.config_file,
            "[web]\nenabled = false\nport = 27372\npeer_port = 27375\n",
        )
        .unwrap();
        let cfg = LibraryConfig::new(paths.clone());

        let key = |id: &str, token: &str, write: bool| WebAccessKey {
            id: id.to_string(),
            label: format!("label-{id}"),
            token: token.to_string(),
            write,
            created_at_ms: 42,
            ..Default::default()
        };
        let web = WebSettings {
            enabled: true,
            port: 28101,
            keys: vec![key("web_a", "token_a", false)],
            peer_keys: vec![key("peer_a", "token_b", true)],
            peer_port: 28102,
        };
        let change = cfg.update_web(&web).unwrap();
        assert!(change.web_changed);
        let saved = cfg.current().web;
        assert!(saved.enabled);
        assert_eq!(saved.port, web.port);
        assert_eq!(saved.peer_port, web.peer_port);
        assert!(saved.keys == web.keys);
        assert!(saved.peer_keys == web.peer_keys);

        let text = std::fs::read_to_string(&paths.config_file).unwrap();
        assert!(text.contains("[[web.keys]]"), "{text}");
        assert!(text.contains("[[web.peer_keys]]"), "{text}");

        let cleared = WebSettings {
            keys: vec![],
            peer_keys: vec![],
            ..web
        };
        cfg.update_web(&cleared).unwrap();
        let saved = cfg.current().web;
        assert!(saved.enabled);
        assert_eq!(saved.port, cleared.port);
        assert_eq!(saved.peer_port, cleared.peer_port);
        assert!(saved.keys.is_empty());
        assert!(saved.peer_keys.is_empty());
        let text = std::fs::read_to_string(&paths.config_file).unwrap();
        assert!(!text.contains("[[web.keys]]"), "{text}");
        assert!(!text.contains("[[web.peer_keys]]"), "{text}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}

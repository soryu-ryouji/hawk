//! 文件夹/分类/标签锁（.hawk/locks.toml，参与同步）：被锁的维度需要密码解锁后其内容才可见。
//! 与 global_filter（客户端约定式的视图偏好）不同，锁是安全边界，由服务端强制：
//!
//! - 列表查询：未解锁的锁由 API 层并入 exclude_* 参数（复用现有排除逻辑），主动筛选
//!   （folders/categories/tags 参数）命中未解锁的锁时直接 403 LOCKED
//! - 内容直连（thumbnail/file）与事件广播：按 `LockGuard::item_visible` 判定（位置 OR + 分类标签 AND）
//!
//! 解锁模型：`POST lock/unlock` 校验密码后发放随机票据（daemon 内存 `票据 → 锁条目`，重启失效），
//! 各客户端独立持有（解锁状态不随分享的页面链接扩散），请求经 `X-Hawk-Unlock` 头或
//! `?unlock=` 查询参数（img/SSE 通道）附带。前端丢弃票据即「锁定回去」。
//!
//! 密码以 Argon2id（PHC 字符串自含 salt 与参数）存储；验证失败全局节流（连续 5 次冷却 60s）
//! 防在线爆破。锁不是加密：文件系统层面无保护，仅防止「通过 hawk 查看」。
//! 级联跟随（文件夹移动/删除、分类/标签改名删除）由索引流水线在对应 Job 内调用（同 global_filter）。

use crate::core::events::EventBus;
use crate::core::metadata::toml_string;
use crate::core::paths::LibraryPaths;
use crate::core::registry_file::{atomic_write, sort_entries};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 锁集变更事件：负载为完整快照（LocksSnapshot，仅名称不含哈希），客户端据此重拉并重查列表
pub const LOCKS_CHANGED: &str = "locks.changed";

/// 解锁失败节流：连续失败次数上限与冷却时长（全局计数，防在线爆破）
const THROTTLE_MAX_FAILURES: u32 = 5;
const THROTTLE_COOLDOWN: Duration = Duration::from_secs(60);

/// 锁的维度
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LockDim {
    Folder,
    Category,
    Tag,
}

impl LockDim {
    /// API 字符串（folder/category/tag）→ 维度
    pub fn parse(s: &str) -> Option<LockDim> {
        match s {
            "folder" => Some(LockDim::Folder),
            "category" => Some(LockDim::Category),
            "tag" => Some(LockDim::Tag),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            LockDim::Folder => "folder",
            LockDim::Category => "category",
            LockDim::Tag => "tag",
        }
    }
}

/// 单个锁条目：name 为文件夹库内相对路径或分类/标签名；password 为 Argon2id PHC 字符串
#[derive(Clone)]
struct LockEntry {
    name: String,
    password: String,
}

/// 锁快照（API 响应/事件负载：仅名称，不含密码哈希）
#[derive(Clone, Default, serde::Serialize, utoipa::ToSchema)]
pub struct LocksSnapshot {
    pub folders: Vec<String>,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
}

/// 单条解锁结果
pub enum UnlockOutcome {
    /// 校验通过，返回新票据
    Granted(String),
    /// 锁不存在
    NotFound,
    /// 密码错误
    WrongPassword,
    /// 节流冷却中（剩余冷却时长）
    Throttled(Duration),
}

/// 锁定/改密结果（set/remove 共用）
pub enum ManageOutcome {
    Ok,
    NotFound,
    /// 需要旧密码校验但提供的旧密码错误
    WrongOldPassword,
    Throttled(Duration),
}

/// 请求级锁守卫：当前请求「未解锁」的锁集合（快照视图，每请求构建一次）。
/// 全空时一切判定走快路径（恒可见）
#[derive(Default, Clone)]
pub struct LockGuard {
    folders: HashSet<String>,
    categories: HashSet<String>,
    tags: HashSet<String>,
}

impl LockGuard {
    pub fn is_empty(&self) -> bool {
        self.folders.is_empty() && self.categories.is_empty() && self.tags.is_empty()
    }

    /// 文件夹视图（folders 参数）是否位于未解锁的锁定文件夹内：目标自身或任一祖先被锁
    pub fn folder_view_locked(&self, folder: &str) -> bool {
        if self.folders.is_empty() {
            return false;
        }
        // 库根（""）不可能位于锁定文件夹内
        let mut cur = folder;
        loop {
            if self.folders.contains(cur) {
                return true;
            }
            match cur.rfind('/') {
                Some(i) => cur = &cur[..i],
                None => return false,
            }
        }
    }

    /// 分类/标签是否被未解锁的锁覆盖（主动筛选参数的 403 判定用）
    pub fn category_locked(&self, name: &str) -> bool {
        self.categories.contains(name)
    }

    pub fn tag_locked(&self, name: &str) -> bool {
        self.tags.contains(name)
    }

    /// 位置级判定：该位置的祖先链不命中未解锁的文件夹锁，且 item 的分类/标签不命中未解锁锁
    pub fn entry_visible(&self, loc_path: &str, categories: &[String], tags: &[String]) -> bool {
        self.location_free(loc_path) && self.taxonomy_visible(categories, tags)
    }

    /// item 级判定（内容直连/事件广播）：存在任一「无锁或已解锁」位置（同内容多路径 OR 语义），
    /// 且分类/标签命中的锁全部已解锁
    pub fn item_visible(
        &self,
        mut loc_paths: impl Iterator<Item = String>,
        categories: &[String],
        tags: &[String],
    ) -> bool {
        if self.is_empty() {
            return true;
        }
        if !self.taxonomy_visible(categories, tags) {
            return false;
        }
        loc_paths.any(|p| self.location_free(&p))
    }

    /// 位置自由：祖先链（含所在目录自身）不命中未解锁的文件夹锁。
    /// 按位置的库内实际路径判定（回收站位置为 .hawk/trash/ 前缀，锁条目已随迁移记录
    /// 同前缀，故回收站内保持锁定；与列表排除同一口径）
    fn location_free(&self, loc_path: &str) -> bool {
        if self.folders.is_empty() {
            return true;
        }
        let mut cur = LibraryPaths::dir_of(loc_path);
        loop {
            if self.folders.contains(cur) {
                return false;
            }
            match cur.rfind('/') {
                Some(i) => cur = &cur[..i],
                None => return true,
            }
        }
    }

    /// 分类/标签（item 属性，与位置无关）：任一命中未解锁的锁即不可见
    fn taxonomy_visible(&self, categories: &[String], tags: &[String]) -> bool {
        if !self.categories.is_empty() && categories.iter().any(|c| self.categories.contains(c)) {
            return false;
        }
        if !self.tags.is_empty() && tags.iter().any(|t| self.tags.contains(t)) {
            return false;
        }
        true
    }
}

pub struct Locks {
    file: String,
    folders: std::sync::RwLock<Vec<LockEntry>>,
    categories: std::sync::RwLock<Vec<LockEntry>>,
    tags: std::sync::RwLock<Vec<LockEntry>>,
    /// 解锁票据：secret → 锁条目（dim, name）。daemon 内存，重启失效
    tickets: std::sync::RwLock<HashMap<String, (LockDim, String)>>,
    /// 验证失败节流（全局）
    throttle: Mutex<(u32, Option<Instant>)>,
}

impl Locks {
    pub fn new(paths: &LibraryPaths) -> Locks {
        let (folders, categories, tags) = load(&paths.locks_file);
        Locks {
            file: paths.locks_file.clone(),
            folders: std::sync::RwLock::new(folders),
            categories: std::sync::RwLock::new(categories),
            tags: std::sync::RwLock::new(tags),
            tickets: std::sync::RwLock::new(HashMap::new()),
            throttle: Mutex::new((0, None)),
        }
    }

    /// 名称快照（小写排序，与各注册表一致）
    pub fn snapshot(&self) -> LocksSnapshot {
        let mut folders: Vec<String> = self
            .folders
            .read()
            .unwrap()
            .iter()
            .map(|e| e.name.clone())
            .collect();
        let mut categories: Vec<String> = self
            .categories
            .read()
            .unwrap()
            .iter()
            .map(|e| e.name.clone())
            .collect();
        let mut tags: Vec<String> = self
            .tags
            .read()
            .unwrap()
            .iter()
            .map(|e| e.name.clone())
            .collect();
        sort_entries(&mut folders);
        sort_entries(&mut categories);
        sort_entries(&mut tags);
        LocksSnapshot {
            folders,
            categories,
            tags,
        }
    }

    /// 是否存在某维度的锁（快速判定，供 folder/list 标记等）
    pub fn contains(&self, dim: LockDim, name: &str) -> bool {
        match dim {
            LockDim::Folder => self.folders.read().unwrap().iter().any(|e| e.name == name),
            LockDim::Category => self
                .categories
                .read()
                .unwrap()
                .iter()
                .any(|e| e.name == name),
            LockDim::Tag => self.tags.read().unwrap().iter().any(|e| e.name == name),
        }
    }

    /// 由请求携带的票据构建锁守卫：全部锁 −（票据覆盖且仍存在的锁）= 未解锁锁
    pub fn guard(&self, tickets: &[String]) -> LockGuard {
        let mut unlocked: HashSet<(LockDim, String)> = HashSet::new();
        {
            let table = self.tickets.read().unwrap();
            for t in tickets {
                if let Some(key) = table.get(t) {
                    unlocked.insert(key.clone());
                }
            }
        }
        let mut guard = LockGuard::default();
        for e in self.folders.read().unwrap().iter() {
            if !unlocked.contains(&(LockDim::Folder, e.name.clone())) {
                guard.folders.insert(e.name.clone());
            }
        }
        for e in self.categories.read().unwrap().iter() {
            if !unlocked.contains(&(LockDim::Category, e.name.clone())) {
                guard.categories.insert(e.name.clone());
            }
        }
        for e in self.tags.read().unwrap().iter() {
            if !unlocked.contains(&(LockDim::Tag, e.name.clone())) {
                guard.tags.insert(e.name.clone());
            }
        }
        guard
    }

    /// 零解锁守卫（事件广播的保守判定）
    pub fn zero_guard(&self) -> LockGuard {
        self.guard(&[])
    }

    /// 解锁：校验密码，通过则发放票据
    pub fn unlock(&self, dim: LockDim, name: &str, password: &str) -> UnlockOutcome {
        if let Some(wait) = self.check_throttle() {
            return UnlockOutcome::Throttled(wait);
        }
        let entries = self.list(dim);
        let Some(entry) = entries.iter().find(|e| e.name == name) else {
            // 不存在的锁不计失败（无密码可试）
            return UnlockOutcome::NotFound;
        };
        if !verify_password(&entry.password, password) {
            self.record_failure();
            return UnlockOutcome::WrongPassword;
        }
        self.clear_failures();
        let ticket = issue_ticket();
        self.tickets
            .write()
            .unwrap()
            .insert(ticket.clone(), (dim, name.to_string()));
        UnlockOutcome::Granted(ticket)
    }

    /// 设锁/改密：已存在时需旧密码（WrongOldPassword），新设时直接写入
    pub fn set(
        &self,
        dim: LockDim,
        name: &str,
        password: &str,
        old_password: Option<&str>,
    ) -> ManageOutcome {
        if let Some(wait) = self.check_throttle() {
            return ManageOutcome::Throttled(wait);
        }
        let mut entries = self.list(dim);
        match entries.iter().position(|e| e.name == name) {
            Some(i) => {
                let Some(old) = old_password else {
                    return ManageOutcome::WrongOldPassword; // 已锁：改密必须带旧密码
                };
                if !verify_password(&entries[i].password, old) {
                    self.record_failure();
                    return ManageOutcome::WrongOldPassword;
                }
                self.clear_failures();
                entries[i].password = hash_password(password);
            }
            None => {
                entries.push(LockEntry {
                    name: name.to_string(),
                    password: hash_password(password),
                });
            }
        }
        self.save_list(dim, entries);
        ManageOutcome::Ok
    }

    /// 解除锁：需要密码
    pub fn remove(&self, dim: LockDim, name: &str, password: &str) -> ManageOutcome {
        if let Some(wait) = self.check_throttle() {
            return ManageOutcome::Throttled(wait);
        }
        let mut entries = self.list(dim);
        let Some(i) = entries.iter().position(|e| e.name == name) else {
            return ManageOutcome::NotFound;
        };
        if !verify_password(&entries[i].password, password) {
            self.record_failure();
            return ManageOutcome::WrongOldPassword;
        }
        self.clear_failures();
        entries.remove(i);
        self.save_list(dim, entries);
        ManageOutcome::Ok
    }

    // ---------- 级联簿记（索引流水线调用；含移入/移出回收站的路径迁移） ----------

    /// 文件夹移动/重命名：前缀范围内条目整体迁移（保留密码哈希）；目标已存在时合并（去重）。
    /// 含移入回收站（trash 前缀）与恢复的往返迁移
    pub fn rename_folder_prefix(&self, old_dir: &str, new_dir: &str) -> bool {
        let mut entries = self.folders.write().unwrap();
        let prefix = format!("{old_dir}/");
        let hits: Vec<LockEntry> = entries
            .iter()
            .filter(|e| e.name == old_dir || e.name.starts_with(&prefix))
            .cloned()
            .collect();
        if hits.is_empty() {
            return false;
        }
        for hit in hits {
            let moved_name = format!("{new_dir}{}", &hit.name[old_dir.len()..]);
            // 删除源条目；目标同名时合并（去重，保留既有密码）
            entries.retain(|e| e.name != hit.name);
            if !entries.iter().any(|e| e.name == moved_name) {
                entries.push(LockEntry {
                    name: moved_name,
                    password: hit.password,
                });
            }
        }
        drop(entries);
        self.persist();
        true
    }

    /// 文件夹删除（含清空回收站）：前缀范围内条目一并移除
    pub fn delete_folder_prefix(&self, dir: &str) -> bool {
        let mut entries = self.folders.write().unwrap();
        let prefix = format!("{dir}/");
        let before = entries.len();
        entries.retain(|e| e.name != dir && !e.name.starts_with(&prefix));
        let changed = entries.len() != before;
        drop(entries);
        if changed {
            self.persist();
        }
        changed
    }

    /// 分类/标签重命名跟随；目标已锁时合并（保留目标密码）
    pub fn rename_taxonomy(&self, dim: LockDim, old_name: &str, new_name: &str) -> bool {
        let mut entries = self.list(dim);
        let Some(i) = entries.iter().position(|e| e.name == old_name) else {
            return false;
        };
        if let Some(j) = entries.iter().position(|e| e.name == new_name) {
            if j != i {
                // 目标已锁：合并到目标（目标的密码生效），删除旧条目
                entries.remove(i);
            } else {
                return false;
            }
        } else {
            entries[i].name = new_name.to_string();
        }
        self.save_list(dim, entries);
        true
    }

    /// 分类/标签删除跟随
    pub fn delete_taxonomy(&self, dim: LockDim, name: &str) -> bool {
        let mut entries = self.list(dim);
        let before = entries.len();
        entries.retain(|e| e.name != name);
        let changed = entries.len() != before;
        if changed {
            self.save_list(dim, entries);
        }
        changed
    }

    /// 外部修改（网盘同步落地等）后重载；返回是否发生变更（调用方据此广播）
    pub fn reload(&self) -> bool {
        let (folders, categories, tags) = load(&self.file);
        let mut changed = false;
        {
            let mut cur = self.folders.write().unwrap();
            if names(&cur) != names(&folders) {
                *cur = folders;
                changed = true;
            }
        }
        {
            let mut cur = self.categories.write().unwrap();
            if names(&cur) != names(&categories) {
                *cur = categories;
                changed = true;
            }
        }
        {
            let mut cur = self.tags.write().unwrap();
            if names(&cur) != names(&tags) {
                *cur = tags;
                changed = true;
            }
        }
        changed
    }

    // ---------- 内部 ----------

    fn list(&self, dim: LockDim) -> Vec<LockEntry> {
        match dim {
            LockDim::Folder => self.folders.read().unwrap().clone(),
            LockDim::Category => self.categories.read().unwrap().clone(),
            LockDim::Tag => self.tags.read().unwrap().clone(),
        }
    }

    fn save_list(&self, dim: LockDim, entries: Vec<LockEntry>) {
        match dim {
            LockDim::Folder => *self.folders.write().unwrap() = entries,
            LockDim::Category => *self.categories.write().unwrap() = entries,
            LockDim::Tag => *self.tags.write().unwrap() = entries,
        }
        self.persist();
    }

    /// 三段全量落盘（调用方须先完成对应段的写锁更新）
    fn persist(&self) {
        let body = format_entries(
            &self.folders.read().unwrap(),
            &self.categories.read().unwrap(),
            &self.tags.read().unwrap(),
        );
        atomic_write(&self.file, &body);
    }

    /// 节流闸门：冷却中返回剩余时长
    fn check_throttle(&self) -> Option<Duration> {
        let state = self.throttle.lock().unwrap();
        if let Some(until) = state.1 {
            let now = Instant::now();
            if now < until {
                return Some(until - now);
            }
        }
        None
    }

    fn record_failure(&self) {
        let mut state = self.throttle.lock().unwrap();
        state.0 += 1;
        if state.0 >= THROTTLE_MAX_FAILURES {
            state.1 = Some(Instant::now() + THROTTLE_COOLDOWN);
        }
    }

    fn clear_failures(&self) {
        *self.throttle.lock().unwrap() = (0, None);
    }
}

fn names(entries: &[LockEntry]) -> Vec<String> {
    entries.iter().map(|e| e.name.clone()).collect()
}

/// 广播锁集变更（负载为名称快照，不含哈希）
pub fn publish_changed(bus: &EventBus, snapshot: &LocksSnapshot) {
    bus.publish(
        LOCKS_CHANGED,
        serde_json::to_value(snapshot).expect("锁集快照序列化失败"),
    );
}

// ---------- 密码哈希（Argon2id，默认参数：19MiB / t=2 / p=1） ----------

fn hash_password(password: &str) -> String {
    use argon2::password_hash::{PasswordHasher, SaltString};
    // 盐与票据的随机源：项目已有 getrandom（盐 16 字节，PHC 标准长度）
    let mut salt_bytes = [0u8; 16];
    getrandom::fill(&mut salt_bytes).expect("系统随机源不可用");
    let salt = SaltString::encode_b64(&salt_bytes).expect("盐编码失败");
    argon2::Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("Argon2id 哈希计算失败")
        .to_string()
}

fn verify_password(phc: &str, password: &str) -> bool {
    use argon2::password_hash::PasswordVerifier;
    let Ok(parsed) = argon2::PasswordHash::new(phc) else {
        return false; // 存储损坏按验证失败处理
    };
    argon2::Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// 随机票据：32 字节 hex（daemon 内存级凭证，重启失效）
fn issue_ticket() -> String {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("系统随机源不可用");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------- 文件读写：手拼 TOML（array of tables），原子写 ----------

fn format_entries(folders: &[LockEntry], categories: &[LockEntry], tags: &[LockEntry]) -> String {
    let mut out = String::new();
    for e in folders {
        out.push_str(&format!(
            "[[folders]]\npath = {}\npassword = {}\n\n",
            toml_string(&e.name),
            toml_string(&e.password)
        ));
    }
    for e in categories {
        out.push_str(&format!(
            "[[categories]]\nname = {}\npassword = {}\n\n",
            toml_string(&e.name),
            toml_string(&e.password)
        ));
    }
    for e in tags {
        out.push_str(&format!(
            "[[tags]]\nname = {}\npassword = {}\n\n",
            toml_string(&e.name),
            toml_string(&e.password)
        ));
    }
    out
}

fn load(file: &str) -> (Vec<LockEntry>, Vec<LockEntry>, Vec<LockEntry>) {
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(_) => return (Vec::new(), Vec::new(), Vec::new()),
    };
    let value: toml::Value = match toml::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("锁注册表解析失败，按空表处理: {file}: {e}");
            return (Vec::new(), Vec::new(), Vec::new());
        }
    };
    (
        parse_entries(&value, "folders", "path"),
        parse_entries(&value, "categories", "name"),
        parse_entries(&value, "tags", "name"),
    )
}

/// 解析 array of tables：name_key 为该维度取名字段（folders 用 path，其余用 name）；
/// 名字去空去重，password 缺失或非法（空串）按损坏条目跳过
fn parse_entries(value: &toml::Value, table: &str, name_key: &str) -> Vec<LockEntry> {
    let mut out: Vec<LockEntry> = value
        .get(table)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let name = v.get(name_key)?.as_str()?.trim().to_string();
                    let password = v.get("password")?.as_str()?.to_string();
                    if name.is_empty() || password.is_empty() {
                        return None;
                    }
                    Some(LockEntry { name, password })
                })
                .collect()
        })
        .unwrap_or_default();
    // 同名条目保留首个（网盘合并冲突的常见形态）
    let mut seen = HashSet::new();
    out.retain(|e| seen.insert(e.name.clone()));
    out.sort_by_key(|e| e.name.to_lowercase());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> (std::path::PathBuf, Locks) {
        let dir =
            std::env::temp_dir().join(format!("hawk-locks-test-{name}-{}", std::process::id()));
        let root = dir.join("lib");
        std::fs::create_dir_all(root.join(".hawk")).unwrap();
        let paths = LibraryPaths::new(root.to_str().unwrap(), None);
        (dir, Locks::new(&paths))
    }

    #[test]
    fn set_unlock_and_persist() {
        let (dir, locks) = fixture("set");
        assert!(matches!(
            locks.set(LockDim::Folder, "a/b", "pw123", None),
            ManageOutcome::Ok
        ));
        assert!(matches!(
            locks.set(LockDim::Category, "私密", "pw2", None),
            ManageOutcome::Ok
        ));

        // 零解锁守卫：a/b 子树与「私密」分类均不可见
        let zero = locks.zero_guard();
        assert!(!zero.item_visible(["a/b/x.jpg".to_string()].into_iter(), &[], &[]));
        assert!(!zero.item_visible(
            ["free/x.jpg".to_string()].into_iter(),
            &["私密".to_string()],
            &[]
        ));
        assert!(zero.item_visible(["free/x.jpg".to_string()].into_iter(), &[], &[]));

        // 解锁错误密码
        assert!(matches!(
            locks.unlock(LockDim::Folder, "a/b", "wrong"),
            UnlockOutcome::WrongPassword
        ));
        // 解锁正确密码 → 票据
        let UnlockOutcome::Granted(ticket) = locks.unlock(LockDim::Folder, "a/b", "pw123") else {
            panic!("应发放票据");
        };
        let guard = locks.guard(std::slice::from_ref(&ticket));
        assert!(guard.item_visible(["a/b/x.jpg".to_string()].into_iter(), &[], &[]));
        // 「私密」分类仍锁
        assert!(!guard.item_visible(
            ["a/b/x.jpg".to_string()].into_iter(),
            &["私密".to_string()],
            &[]
        ));

        // 票据对其他客户端无效（独立持有）
        assert!(!locks
            .guard(&[])
            .item_visible(["a/b/x.jpg".to_string()].into_iter(), &[], &[]));

        // 落盘后可重载（新实例恢复；票据随之失效——内存态）
        let paths = LibraryPaths::new(dir.join("lib").to_str().unwrap(), None);
        let reloaded = Locks::new(&paths);
        assert_eq!(reloaded.snapshot().folders, vec!["a/b".to_string()]);
        assert!(!reloaded.guard(&[ticket]).item_visible(
            ["a/b/x.jpg".to_string()].into_iter(),
            &[],
            &[]
        ));

        // 解除锁需密码
        assert!(matches!(
            locks.remove(LockDim::Folder, "a/b", "wrong"),
            ManageOutcome::WrongOldPassword
        ));
        assert!(matches!(
            locks.remove(LockDim::Folder, "a/b", "pw123"),
            ManageOutcome::Ok
        ));
        assert!(locks.snapshot().folders.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_prefix_cascade() {
        let (dir, locks) = fixture("prefix");
        locks.set(LockDim::Folder, "posters", "pw", None);
        locks.set(LockDim::Folder, "posters/2024", "pw", None);
        locks.set(LockDim::Folder, "other", "pw", None);

        assert!(locks.rename_folder_prefix("posters", ".hawk/trash/posters"));
        let snap = locks.snapshot();
        assert!(snap.folders.contains(&".hawk/trash/posters".to_string()));
        assert!(snap
            .folders
            .contains(&".hawk/trash/posters/2024".to_string()));

        // 迁移后锁住 trash 前缀（回收站保持锁定）
        assert!(!locks.zero_guard().item_visible(
            [".hawk/trash/posters/2024/a.jpg".to_string()].into_iter(),
            &[],
            &[]
        ));

        assert!(locks.rename_folder_prefix(".hawk/trash/posters", "posters"));
        assert!(locks
            .snapshot()
            .folders
            .contains(&"posters/2024".to_string()));

        assert!(locks.delete_folder_prefix("posters"));
        assert_eq!(locks.snapshot().folders, vec!["other".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn taxonomy_cascade() {
        let (dir, locks) = fixture("taxon");
        locks.set(LockDim::Category, "a", "pw", None);
        locks.set(LockDim::Tag, "x", "pw", None);

        // 改名跟随（目标已锁时合并到目标）
        locks.set(LockDim::Category, "b", "pwb", None);
        assert!(locks.rename_taxonomy(LockDim::Category, "a", "b"));
        assert_eq!(locks.snapshot().categories, vec!["b".to_string()]);

        assert!(locks.rename_taxonomy(LockDim::Tag, "x", "y"));
        assert_eq!(locks.snapshot().tags, vec!["y".to_string()]);

        assert!(locks.delete_taxonomy(LockDim::Category, "b"));
        assert!(locks.delete_taxonomy(LockDim::Tag, "y"));
        assert!(locks.snapshot().categories.is_empty() && locks.snapshot().tags.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn guard_view_and_entry() {
        let (dir, locks) = fixture("guard");
        locks.set(LockDim::Folder, "a/b", "pw", None);
        locks.set(LockDim::Tag, "nsfw", "pw", None);
        let zero = locks.zero_guard();

        // 视图判定：a/b 自身与其子孙视图被锁；兄弟/父视图不锁
        assert!(zero.folder_view_locked("a/b"));
        assert!(zero.folder_view_locked("a/b/c"));
        assert!(!zero.folder_view_locked("a"));
        assert!(!zero.folder_view_locked("b"));

        // entry 级：位置与分类/标签都自由才可见
        assert!(zero.entry_visible("a/x.jpg", &[], &[]));
        assert!(!zero.entry_visible("a/b/x.jpg", &[], &[]));
        assert!(!zero.entry_visible("a/x.jpg", &[], &["nsfw".to_string()]));
        assert!(!zero.entry_visible("a/b/x.jpg", &["nsfw".to_string()], &["nsfw".to_string()]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn throttle_blocks_after_failures() {
        let (dir, locks) = fixture("throttle");
        locks.set(LockDim::Folder, "f", "pw", None);
        for _ in 0..THROTTLE_MAX_FAILURES {
            assert!(matches!(
                locks.unlock(LockDim::Folder, "f", "bad"),
                UnlockOutcome::WrongPassword
            ));
        }
        // 冷却中：正确密码也被拒
        assert!(matches!(
            locks.unlock(LockDim::Folder, "f", "pw"),
            UnlockOutcome::Throttled(_)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

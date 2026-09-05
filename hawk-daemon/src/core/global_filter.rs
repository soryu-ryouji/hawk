//! 全局列表隐藏项注册表（.hawk/global_filter.toml，参与同步）：被标记的文件夹/分类/标签，
//! 其下素材在「全部素材/根目录/未分类/未标签」等全局视图中被查询层排除（exclude 参数由
//! 客户端按本表附带）；进入该维度自身视图仍可见。回收站视图不参与排除。
//!
//! 纯注册表（与索引/元数据无耦合，同 view_prefs）：API 端点直接读写，不经过索引流水线；
//! 级联跟随（文件夹移动/删除、分类/标签改名删除）由索引流水线在对应 Job 内调用。
//! 文件夹条目即库内相对路径（子树整体隐藏）；隐藏文件夹移入回收站时条目跟随路径迁移，
//! 恢复时随之回归，清空回收站时一并清除（与排序偏好同款簿记）。

use crate::core::events::EventBus;
use crate::core::paths::LibraryPaths;
use crate::core::registry_file::{atomic_write, format_string_list, parse_string_list, sort_entries};
use std::sync::RwLock;

/// 隐藏集变更事件：负载为完整快照（GlobalFilterSnapshot），客户端据此重拉/就地替换并重查列表
pub const GLOBAL_FILTER_CHANGED: &str = "global_filter.changed";

#[derive(Clone, Default, serde::Serialize, utoipa::ToSchema)]
pub struct GlobalFilterSnapshot {
    pub folders: Vec<String>,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
}

pub struct GlobalFilter {
    file: String,
    folders: RwLock<Vec<String>>,
    categories: RwLock<Vec<String>>,
    tags: RwLock<Vec<String>>,
}

impl GlobalFilter {
    pub fn new(paths: &LibraryPaths) -> GlobalFilter {
        let snapshot = load(&paths.global_filter_file);
        GlobalFilter {
            file: paths.global_filter_file.clone(),
            folders: RwLock::new(snapshot.folders),
            categories: RwLock::new(snapshot.categories),
            tags: RwLock::new(snapshot.tags),
        }
    }

    pub fn snapshot(&self) -> GlobalFilterSnapshot {
        GlobalFilterSnapshot {
            folders: self.folders.read().unwrap().clone(),
            categories: self.categories.read().unwrap().clone(),
            tags: self.tags.read().unwrap().clone(),
        }
    }

    /// 标记/取消隐藏；返回是否发生变更（调用方据此广播事件）
    pub fn set_folder_hidden(&self, path: &str, hidden: bool) -> bool {
        set_locked(&self.file, &self.folders, path, hidden, || self.snapshot())
    }

    pub fn set_category_hidden(&self, name: &str, hidden: bool) -> bool {
        set_locked(&self.file, &self.categories, name, hidden, || self.snapshot())
    }

    pub fn set_tag_hidden(&self, name: &str, hidden: bool) -> bool {
        set_locked(&self.file, &self.tags, name, hidden, || self.snapshot())
    }

    /// 文件夹移动/重命名：前缀范围内条目跟随迁移（含移入回收站，恢复时随之回归）。
    /// 目标已存在时合并（去重）；返回是否发生变更
    pub fn rename_folder_prefix(&self, old_dir: &str, new_dir: &str) -> bool {
        let mut folders = self.folders.write().unwrap();
        let prefix = format!("{old_dir}/");
        let mut changed = false;
        let hits: Vec<String> = folders
            .iter()
            .filter(|f| *f == old_dir || f.starts_with(&prefix))
            .cloned()
            .collect();
        for hit in hits {
            let suffix = &hit[old_dir.len()..];
            let moved = format!("{new_dir}{suffix}");
            folders.retain(|f| f != &hit);
            if !folders.iter().any(|f| f == &moved) {
                folders.push(moved);
            }
            changed = true;
        }
        if changed {
            sort_entries(&mut folders);
            save(&self.file, &folders, &self.categories.read().unwrap(), &self.tags.read().unwrap());
        }
        changed
    }

    /// 文件夹删除（含清空回收站对回收站前缀的清除）：前缀范围内条目一并移除
    pub fn delete_folder_prefix(&self, dir: &str) -> bool {
        let mut folders = self.folders.write().unwrap();
        let prefix = format!("{dir}/");
        let before = folders.len();
        folders.retain(|f| f != dir && !f.starts_with(&prefix));
        let changed = folders.len() != before;
        if changed {
            save(&self.file, &folders, &self.categories.read().unwrap(), &self.tags.read().unwrap());
        }
        changed
    }

    /// 分类重命名跟随；目标已隐藏时合并（去重）
    pub fn rename_category(&self, old_name: &str, new_name: &str) -> bool {
        rename_locked(&self.file, &self.categories, old_name, new_name, || self.snapshot())
    }

    pub fn delete_category(&self, name: &str) -> bool {
        delete_locked(&self.file, &self.categories, name, || self.snapshot())
    }

    pub fn rename_tag(&self, old_name: &str, new_name: &str) -> bool {
        rename_locked(&self.file, &self.tags, old_name, new_name, || self.snapshot())
    }

    pub fn delete_tag(&self, name: &str) -> bool {
        delete_locked(&self.file, &self.tags, name, || self.snapshot())
    }

    /// 外部修改（网盘同步落地等）后重载；返回是否发生变更（调用方据此广播）
    pub fn reload(&self) -> bool {
        let loaded = load(&self.file);
        let mut changed = false;
        {
            let mut folders = self.folders.write().unwrap();
            if *folders != loaded.folders {
                *folders = loaded.folders;
                changed = true;
            }
        }
        {
            let mut categories = self.categories.write().unwrap();
            if *categories != loaded.categories {
                *categories = loaded.categories;
                changed = true;
            }
        }
        {
            let mut tags = self.tags.write().unwrap();
            if *tags != loaded.tags {
                *tags = loaded.tags;
                changed = true;
            }
        }
        changed
    }
}

/// 广播隐藏集变更（负载为完整快照：客户端无需二次拉取）
pub fn publish_changed(bus: &EventBus, snapshot: &GlobalFilterSnapshot) {
    bus.publish(
        GLOBAL_FILTER_CHANGED,
        serde_json::to_value(snapshot).expect("隐藏集快照序列化失败"),
    );
}

// ---------- 内部：单列表增删改的公共骨架（写锁内改完即落盘） ----------

/// set/add/remove 的公共实现；snap 用于在持锁改完后取一致快照落盘
fn set_locked(
    file: &str,
    list: &RwLock<Vec<String>>,
    name: &str,
    present: bool,
    snap: impl FnOnce() -> GlobalFilterSnapshot,
) -> bool {
    let mut entries = list.write().unwrap();
    let exists = entries.iter().any(|e| e == name);
    if present == exists {
        return false;
    }
    if present {
        entries.push(name.to_string());
        sort_entries(&mut entries);
    } else {
        entries.retain(|e| e != name);
    }
    drop(entries);
    let snapshot = snap();
    save(file, &snapshot.folders, &snapshot.categories, &snapshot.tags);
    true
}

fn rename_locked(
    file: &str,
    list: &RwLock<Vec<String>>,
    old_name: &str,
    new_name: &str,
    snap: impl FnOnce() -> GlobalFilterSnapshot,
) -> bool {
    let mut entries = list.write().unwrap();
    if !entries.iter().any(|e| e == old_name) {
        return false;
    }
    entries.retain(|e| e != old_name);
    if !entries.iter().any(|e| e == new_name) {
        entries.push(new_name.to_string());
    }
    sort_entries(&mut entries);
    drop(entries);
    let snapshot = snap();
    save(file, &snapshot.folders, &snapshot.categories, &snapshot.tags);
    true
}

fn delete_locked(
    file: &str,
    list: &RwLock<Vec<String>>,
    name: &str,
    snap: impl FnOnce() -> GlobalFilterSnapshot,
) -> bool {
    let mut entries = list.write().unwrap();
    if !entries.iter().any(|e| e == name) {
        return false;
    }
    entries.retain(|e| e != name);
    drop(entries);
    let snapshot = snap();
    save(file, &snapshot.folders, &snapshot.categories, &snapshot.tags);
    true
}

// ---------- 文件读写：固定 schema（三个字符串数组键），原子写 ----------

fn load(file: &str) -> GlobalFilterSnapshot {
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(_) => return GlobalFilterSnapshot::default(),
    };
    let value: toml::Value = match toml::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("隐藏项注册表解析失败，按空表处理: {file}: {e}");
            return GlobalFilterSnapshot::default();
        }
    };
    GlobalFilterSnapshot {
        folders: parse_string_list(&value, "folders"),
        categories: parse_string_list(&value, "categories"),
        tags: parse_string_list(&value, "tags"),
    }
}

fn save(file: &str, folders: &[String], categories: &[String], tags: &[String]) {
    let body = format!(
        "{}\n{}\n{}\n",
        format_string_list("folders", folders),
        format_string_list("categories", categories),
        format_string_list("tags", tags),
    );
    atomic_write(file, &body);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> (std::path::PathBuf, GlobalFilter) {
        let dir = std::env::temp_dir().join(format!("hawk-gfilter-test-{name}-{}", std::process::id()));
        let root = dir.join("lib");
        std::fs::create_dir_all(root.join(".hawk")).unwrap();
        let paths = LibraryPaths::new(root.to_str().unwrap(), None);
        (dir, GlobalFilter::new(&paths))
    }

    #[test]
    fn set_and_persist() {
        let (dir, filter) = fixture("set");
        assert!(filter.set_folder_hidden("a/b", true));
        assert!(filter.set_category_hidden("素材堆", true));
        assert!(filter.set_tag_hidden("tmp", true));
        // 重复设置无变更
        assert!(!filter.set_folder_hidden("a/b", true));
        let snap = filter.snapshot();
        assert!(snap.folders.contains(&"a/b".to_string()));
        assert!(snap.categories.contains(&"素材堆".to_string()));
        assert!(snap.tags.contains(&"tmp".to_string()));

        // 落盘后可重载（新实例从文件恢复）
        let paths = LibraryPaths::new(dir.join("lib").to_str().unwrap(), None);
        let reloaded = GlobalFilter::new(&paths);
        let snap = reloaded.snapshot();
        assert_eq!(snap.folders, vec!["a/b".to_string()]);
        assert_eq!(snap.categories, vec!["素材堆".to_string()]);
        assert_eq!(snap.tags, vec!["tmp".to_string()]);

        assert!(reloaded.set_folder_hidden("a/b", false));
        assert!(reloaded.snapshot().folders.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_prefix_cascade() {
        let (dir, filter) = fixture("prefix");
        filter.set_folder_hidden("posters", true);
        filter.set_folder_hidden("posters/2024", true);
        filter.set_folder_hidden("other", true);

        // 移动（含移入回收站）：前缀范围内条目跟随
        assert!(filter.rename_folder_prefix("posters", ".hawk/trash/posters"));
        let snap = filter.snapshot();
        assert!(snap.folders.contains(&".hawk/trash/posters".to_string()));
        assert!(snap.folders.contains(&".hawk/trash/posters/2024".to_string()));
        assert!(snap.folders.contains(&"other".to_string()));

        // 恢复时回归
        assert!(filter.rename_folder_prefix(".hawk/trash/posters", "posters"));
        assert!(filter.snapshot().folders.contains(&"posters/2024".to_string()));

        // 删除：前缀范围内清除，其余保留
        assert!(filter.delete_folder_prefix("posters"));
        let snap = filter.snapshot();
        assert_eq!(snap.folders, vec!["other".to_string()]);

        // 无命中无变更
        assert!(!filter.delete_folder_prefix("posters"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn taxonomy_cascade() {
        let (dir, filter) = fixture("taxon");
        filter.set_category_hidden("a", true);
        filter.set_tag_hidden("x", true);

        // 重命名跟随；目标已存在时合并
        filter.set_category_hidden("b", true);
        assert!(filter.rename_category("a", "b"));
        let snap = filter.snapshot();
        assert_eq!(snap.categories, vec!["b".to_string()]);

        assert!(filter.rename_tag("x", "y"));
        assert!(filter.snapshot().tags.contains(&"y".to_string()));

        assert!(filter.delete_category("b"));
        assert!(filter.delete_tag("y"));
        let snap = filter.snapshot();
        assert!(snap.categories.is_empty() && snap.tags.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

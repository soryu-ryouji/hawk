//! 视图偏好注册表（.hawk/view.toml，参与同步）：记住文件夹/分类/标签视图各自的排序方式。
//! 扁平 map（scope 键 folder:<路径>/category:<名>/tag:<名>），不理解继承语义（前端沿父链解析）。
//! 视图偏好（含 RenamePrefix/DeletePrefix 跟随目录移动/删除）。

use crate::core::metadata::toml_string;
use crate::core::paths::LibraryPaths;
use std::collections::HashMap;
use std::sync::Mutex;

const ORDER_BY_WHITELIST: [&str; 4] = ["modification_time", "name", "size", "star"];
const ORDER_WHITELIST: [&str; 2] = ["asc", "desc"];

#[derive(Clone, Debug, PartialEq)]
pub struct ViewSort {
    pub order_by: String,
    pub order: String,
}

#[derive(Default)]
pub struct ViewPreferences {
    file: String,
    entries: Mutex<HashMap<String, ViewSort>>,
}

impl ViewPreferences {
    pub fn new(paths: &LibraryPaths) -> ViewPreferences {
        let prefs = ViewPreferences {
            file: paths.view_file.clone(),
            entries: Mutex::new(HashMap::new()),
        };
        prefs.reload();
        prefs
    }

    pub fn snapshot(&self) -> HashMap<String, ViewSort> {
        self.entries.lock().unwrap().clone()
    }

    /// 覆盖写一条偏好；scope 与 sort 须已过 try_parse_scope/校验
    pub fn set(&self, scope: &str, sort: ViewSort) {
        let mut entries = self.entries.lock().unwrap();
        entries.insert(scope.to_string(), sort);
        save_locked(&self.file, &entries);
    }

    /// 删除一条偏好（回到继承/默认）。不存在则无动作
    pub fn delete(&self, scope: &str) {
        let mut entries = self.entries.lock().unwrap();
        if entries.remove(scope).is_some() {
            save_locked(&self.file, &entries);
        }
    }

    /// 文件夹移动/重命名：前缀范围内的 folder: 键跟随迁移
    pub fn rename_prefix(&self, old_dir: &str, new_dir: &str) {
        let mut entries = self.entries.lock().unwrap();
        let prefix = format!("folder:{old_dir}");
        let hits: Vec<String> = entries
            .keys()
            .filter(|k| *k == &prefix || k.starts_with(&format!("{prefix}/")))
            .cloned()
            .collect();
        if hits.is_empty() {
            return;
        }
        for key in hits {
            if let Some(sort) = entries.remove(&key) {
                let suffix = &key[prefix.len()..];
                entries.insert(format!("folder:{new_dir}{suffix}"), sort);
            }
        }
        save_locked(&self.file, &entries);
    }

    /// 文件夹删除：前缀范围内的 folder: 键一并清除
    pub fn delete_prefix(&self, dir: &str) {
        let mut entries = self.entries.lock().unwrap();
        let prefix = format!("folder:{dir}");
        let hits: Vec<String> = entries
            .keys()
            .filter(|k| *k == &prefix || k.starts_with(&format!("{prefix}/")))
            .cloned()
            .collect();
        if hits.is_empty() {
            return;
        }
        for key in hits {
            entries.remove(&key);
        }
        save_locked(&self.file, &entries);
    }

    /// 外部修改（含网盘同步落地）后重载；解析失败的条目跳过
    pub fn reload(&self) {
        let mut entries = self.entries.lock().unwrap();
        entries.clear();
        let text = match std::fs::read_to_string(&self.file) {
            Ok(t) => t,
            Err(_) => return,
        };
        let table: toml::Value = match toml::from_str(&text) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("视图偏好解析失败，按空表处理: {}: {e}", self.file);
                return;
            }
        };
        let Some(table) = table.as_table() else { return };
        for (key, value) in table {
            let Some(section) = value.as_table() else { continue };
            let order_by = section.get("order_by").and_then(|v| v.as_str());
            let order = section.get("order").and_then(|v| v.as_str());
            match (order_by, order) {
                (Some(ob), Some(o)) => match try_parse_scope(key).and_then(|scope| try_normalize_sort(ob, o).map(|s| (scope, s))) {
                    Some((scope, sort)) => {
                        entries.insert(scope, sort);
                    }
                    None => tracing::warn!("视图偏好条目无效，已跳过: {key}"),
                },
                _ => tracing::warn!("视图偏好条目无效，已跳过: {key}"),
            }
        }
    }
}

/// 校验并规范化 scope 键。folder 路径须为合法库内路径（"" 为库根）；
/// category/tag 名称复用各自注册表的名称规则
pub fn try_parse_scope(raw: &str) -> Option<String> {
    if let Some(path) = raw.strip_prefix("folder:") {
        if !path.is_empty() && !LibraryPaths::is_valid_library_path(Some(path)) {
            return None;
        }
        return Some(format!("folder:{path}"));
    }
    if let Some(name) = raw.strip_prefix("category:") {
        return crate::core::taxonomy::normalize_category_name(Some(name)).map(|n| format!("category:{n}"));
    }
    if let Some(name) = raw.strip_prefix("tag:") {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        return Some(format!("tag:{name}"));
    }
    None
}

/// 校验并规范化排序值（order_by/order 白名单，小写）
pub fn try_normalize_sort(order_by: &str, order: &str) -> Option<ViewSort> {
    let ob = order_by.trim().to_lowercase();
    let o = order.trim().to_lowercase();
    if !ORDER_BY_WHITELIST.contains(&ob.as_str()) || !ORDER_WHITELIST.contains(&o.as_str()) {
        return None;
    }
    Some(ViewSort { order_by: ob, order: o })
}

fn save_locked(file: &str, entries: &HashMap<String, ViewSort>) {
    let mut sb = String::new();
    let mut keys: Vec<&String> = entries.keys().collect();
    keys.sort();
    for key in keys {
        let sort = &entries[key];
        sb.push('[');
        sb.push_str(&toml_string(key));
        sb.push_str("]\n");
        sb.push_str(&format!("order_by = {}\n", toml_string(&sort.order_by)));
        sb.push_str(&format!("order = {}\n", toml_string(&sort.order)));
        sb.push('\n');
    }
    let tmp = format!("{file}.tmp");
    if std::fs::write(&tmp, sb).is_ok() && std::fs::rename(&tmp, file).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_parsing() {
        assert_eq!(try_parse_scope("folder:"), Some("folder:".to_string()));
        assert_eq!(try_parse_scope("folder:posters/2024"), Some("folder:posters/2024".to_string()));
        assert_eq!(try_parse_scope("category: 海报 "), Some("category:海报".to_string()));
        assert_eq!(try_parse_scope("tag:nature"), Some("tag:nature".to_string()));
        assert_eq!(try_parse_scope("folder:.hawk/x"), None);
        assert_eq!(try_parse_scope("folder:a/../b"), None);
        assert_eq!(try_parse_scope("category:a/b"), None);
        assert_eq!(try_parse_scope("other:x"), None);
    }

    #[test]
    fn sort_validation() {
        assert!(try_normalize_sort("modification_time", "desc").is_some());
        assert!(try_normalize_sort("Name", "ASC").is_some());
        assert!(try_normalize_sort("evil", "desc").is_none());
        assert!(try_normalize_sort("name", "up").is_none());
    }
}

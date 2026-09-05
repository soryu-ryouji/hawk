//! 注册表文件持久化公共件：原子写（临时文件 + rename）与字符串列表键的解析/格式化。
//! taxonomy（categories.toml/tags.toml 单键列表）、global_filter（三键列表）、
//! view_prefs（map 型）共用，各注册表只保留自己的 schema 与读写时机。
//! 写入失败保持静默（尽力而为，不阻断业务）——与原各实现语义一致。

use crate::core::metadata::toml_string;

/// 原子写：临时文件 + rename；rename 失败清理临时文件
pub fn atomic_write(file: &str, body: &str) {
    let tmp = format!("{file}.tmp");
    if std::fs::write(&tmp, body).is_ok() && std::fs::rename(&tmp, file).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// 从 toml 值读字符串数组键：trim、去空、去重、小写排序；键缺失/类型不符按空表
pub fn parse_string_list(value: &toml::Value, key: &str) -> Vec<String> {
    let mut out: Vec<String> = value
        .get(key)
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    out = out.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    out.dedup();
    sort_entries(&mut out);
    out
}

/// 格式化为单行键值：`key = ["a", "b"]`（字符串经 toml_string 转义）
pub fn format_string_list(key: &str, entries: &[String]) -> String {
    format!(
        "{} = [{}]",
        key,
        entries.iter().map(|e| toml_string(e)).collect::<Vec<_>>().join(", ")
    )
}

/// 列表条目序的单一来源：小写字典序（大小写不敏感）
pub fn sort_entries(entries: &mut Vec<String>) {
    entries.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_normalizes() {
        let value: toml::Value = toml::from_str(r#"tags = [" b ", "a", "", "a"]"#).unwrap();
        assert_eq!(parse_string_list(&value, "tags"), vec!["a".to_string(), "b".to_string()]);
        assert!(parse_string_list(&value, "missing").is_empty());
        let bad: toml::Value = toml::from_str(r#"tags = "notalist""#).unwrap();
        assert!(parse_string_list(&bad, "tags").is_empty());
    }

    #[test]
    fn format_escapes() {
        assert_eq!(format_string_list("tags", &["a".to_string()]), r#"tags = ["a"]"#);
        assert_eq!(
            format_string_list("tags", &["带\"引号".to_string()]),
            r#"tags = ["带\"引号"]"#
        );
    }

    #[test]
    fn atomic_write_roundtrip() {
        let dir = std::env::temp_dir().join(format!("hawk-regfile-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("x.toml");
        atomic_write(file.to_str().unwrap(), "a = [\"b\"]\n");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "a = [\"b\"]\n");
        assert!(!dir.join("x.toml.tmp").exists(), "临时文件应已 rename");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

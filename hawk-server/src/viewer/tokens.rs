//! 浏览器令牌（bt）哈希的持久化：viewer_tokens.json（sha256 hex 集合，原子写）。

use sha2::Digest;
use std::collections::HashSet;
use std::path::Path;

pub fn hash(token: &str) -> String {
    sha2::Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn load(file: &Path) -> HashSet<String> {
    std::fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<String>>(&bytes).ok())
        .map(|v| v.into_iter().collect())
        .unwrap_or_default()
}

pub fn save(file: &Path, hashes: &HashSet<String>) -> Result<(), String> {
    let mut list: Vec<&String> = hashes.iter().collect();
    list.sort();
    let bytes = serde_json::to_vec_pretty(&list).map_err(|e| format!("令牌序列化失败: {e}"))?;
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("令牌写入失败 {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, file).map_err(|e| format!("令牌替换失败 {}: {e}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_and_hex() {
        let h = hash("hb_test");
        assert_eq!(h.len(), 64);
        assert_eq!(h, hash("hb_test"));
        assert_ne!(h, hash("hb_other"));
    }
}

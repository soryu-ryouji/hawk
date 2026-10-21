//! 浏览器令牌（bt）哈希与访问级别的持久化：viewer_tokens.json（原子写）。
//! 只存 sha256 哈希，不存明文；访问级别随令牌一起保存，服务端重启后仍生效。

use crate::state::Access;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::HashMap;
use std::path::Path;

pub fn hash(token: &str) -> String {
    sha2::Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[derive(Serialize, Deserialize)]
struct TokenEntry {
    hash: String,
    access: Access,
}

pub fn load(file: &Path) -> HashMap<String, Access> {
    std::fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<TokenEntry>>(&bytes).ok())
        .map(|v| {
            v.into_iter()
                .map(|entry| (entry.hash, entry.access))
                .collect()
        })
        .unwrap_or_default()
}

pub fn save(file: &Path, tokens: &HashMap<String, Access>) -> Result<(), String> {
    let mut list: Vec<TokenEntry> = tokens
        .iter()
        .map(|(hash, access)| TokenEntry {
            hash: hash.clone(),
            access: *access,
        })
        .collect();
    list.sort_by(|a, b| a.hash.cmp(&b.hash));
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

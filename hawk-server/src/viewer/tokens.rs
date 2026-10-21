//! 浏览器令牌（bt）的持久化：viewer_tokens.json（原子写）。
//! 只存 sha256 哈希，不存明文；每条 bt 绑定访问级别、目标设备与查看者凭证
//! （hawk-app key 的 sha256，建会话时透传给目标 daemon 本地执法），重启后仍生效。

use crate::state::Access;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

pub fn hash(token: &str) -> String {
    crate::keystore::sha256_hex(token)
}

/// bt 承载的授权：访问级别 + 绑定的目标设备 + 查看者凭证（sha256(key)）
#[derive(Clone, Serialize, Deserialize)]
pub struct Grant {
    pub access: Access,
    pub device_id: String,
    /// hawk-app key 的验证子：会话时随 session.request 下发给目标 daemon 本地校验
    pub credential: String,
}

#[derive(Serialize, Deserialize)]
struct TokenEntry {
    hash: String,
    #[serde(flatten)]
    grant: Grant,
}

pub fn load(file: &Path) -> HashMap<String, Grant> {
    std::fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<TokenEntry>>(&bytes).ok())
        .map(|v| {
            v.into_iter()
                .map(|entry| (entry.hash, entry.grant))
                .collect()
        })
        .unwrap_or_default()
}

pub fn save(file: &Path, tokens: &HashMap<String, Grant>) -> Result<(), String> {
    let mut list: Vec<TokenEntry> = tokens
        .iter()
        .map(|(hash, grant)| TokenEntry {
            hash: hash.clone(),
            grant: grant.clone(),
        })
        .collect();
    list.sort_by(|a, b| a.hash.cmp(&b.hash));
    let bytes = serde_json::to_vec_pretty(&list).map_err(|e| format!("令牌序列化失败: {e}"))?;
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("令牌写入失败 {}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
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

    #[test]
    fn grant_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("viewer_tokens.json");
        let mut tokens = HashMap::new();
        tokens.insert(
            hash("hb_1"),
            Grant {
                access: Access::Write,
                device_id: "d_1".to_string(),
                credential: "cred".to_string(),
            },
        );
        save(&file, &tokens).unwrap();
        let loaded = load(&file);
        let grant = loaded.get(&hash("hb_1")).unwrap();
        assert_eq!(grant.access, Access::Write);
        assert_eq!(grant.device_id, "d_1");
        assert_eq!(grant.credential, "cred");
    }
}

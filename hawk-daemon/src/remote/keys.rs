//! 访问密钥（hawk-app key）的本地签发与保管：`<identity_dir>/keys.json`。
//! key 明文只在签发时经 API 返回一次；本地只存 sha256 验证子与前缀预览。
//! 查看者会话的 session.request 携带验证子，daemon 据此本地执法（权限以本地映射为准，
//! server 声称的 access 只是参考）；每次变更与每次信令接入后全量推送给 server。

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// 自定义 key 最小长度：纯读 key 4 字符；携带写权限 12 字符（弱写 key 被撞开的代价比读大得多）
const MIN_CUSTOM_LEN_READ: usize = 4;
const MIN_CUSTOM_LEN_WRITE: usize = 12;

#[derive(Clone, Serialize, Deserialize)]
pub struct KeyEntry {
    pub id: String,
    pub label: String,
    /// sha256(key) hex：上送 server 的验证子（server 存 HMAC(pepper, s)），查看者会话执法依据
    pub hash: String,
    /// key 前 4 字符（列表识别用；自定义 key 前缀可能泄露弱前缀，可接受——完整 key 不可恢复）
    pub prefix: String,
    pub read: bool,
    pub write: bool,
    pub created_at_ms: u64,
}

pub struct KeyStore {
    file: PathBuf,
    entries: Mutex<Vec<KeyEntry>>,
}

impl KeyStore {
    /// 打开（必要时创建）keys.json；解析失败按空集合继续（用户可在设置里重建）
    pub fn open(identity_dir: &str) -> Arc<KeyStore> {
        let file = Path::new(identity_dir).join("keys.json");
        let entries = match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice::<Vec<KeyEntry>>(&bytes).unwrap_or_else(|e| {
                tracing::warn!("访问密钥文件解析失败，按空集合继续 {}: {e}", file.display());
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        Arc::new(KeyStore {
            file,
            entries: Mutex::new(entries),
        })
    }

    pub fn list(&self) -> Vec<KeyEntry> {
        self.entries.lock().unwrap().clone()
    }

    /// 签发：custom 为 None 时程序生成（hk_ + 四组十六进制，128 位）；
    /// 返回 (条目, 明文 key)——明文只在此刻可见
    pub fn create(
        &self,
        label: &str,
        read: bool,
        write: bool,
        custom: Option<&str>,
    ) -> Result<(KeyEntry, String), String> {
        if !read && !write {
            return Err("读/写权限至少勾选一项".to_string());
        }
        let key = match custom.map(str::trim).filter(|c| !c.is_empty()) {
            Some(custom) => {
                let min = if write {
                    MIN_CUSTOM_LEN_WRITE
                } else {
                    MIN_CUSTOM_LEN_READ
                };
                if custom.chars().count() < min {
                    return Err(format!(
                        "自定义密钥至少 {min} 个字符{}",
                        if write { "（携带写权限）" } else { "" }
                    ));
                }
                custom.to_string()
            }
            None => generate_key(),
        };
        let hash = sha256_hex(&key);
        if self.entries.lock().unwrap().iter().any(|e| e.hash == hash) {
            return Err("该密钥与现有密钥重复".to_string());
        }
        let entry = KeyEntry {
            id: format!("k_{:016x}", random_u64()),
            label: label.trim().to_string(),
            hash,
            prefix: key.chars().take(4).collect(),
            read,
            write,
            created_at_ms: now_ms(),
        };
        let mut entries = self.entries.lock().unwrap();
        entries.push(entry.clone());
        self.save_locked(&entries)?;
        Ok((entry, key))
    }

    /// 更新标签/权限（权限收窄立即生效：server 映射与本地映射同步更新）
    pub fn patch(
        &self,
        id: &str,
        label: Option<&str>,
        read: Option<bool>,
        write: Option<bool>,
    ) -> Result<Option<KeyEntry>, String> {
        let mut entries = self.entries.lock().unwrap();
        let Some(entry) = entries.iter_mut().find(|e| e.id == id) else {
            return Ok(None);
        };
        if let Some(label) = label {
            entry.label = label.trim().to_string();
        }
        if let Some(read) = read {
            entry.read = read;
        }
        if let Some(write) = write {
            entry.write = write;
        }
        if !entry.read && !entry.write {
            return Err("读/写权限至少保留一项".to_string());
        }
        let updated = entry.clone();
        self.save_locked(&entries)?;
        Ok(Some(updated))
    }

    /// 吊销（本地删除 + 调用方负责推送 server；进行中的会话由 daemon 会话侧感知）
    pub fn remove(&self, id: &str) -> Result<bool, String> {
        let mut entries = self.entries.lock().unwrap();
        let before = entries.len();
        entries.retain(|e| e.id != id);
        if entries.len() == before {
            return Ok(false);
        }
        self.save_locked(&entries)?;
        Ok(true)
    }

    /// 查看者会话执法：验证子（sha256）→ 权限
    pub fn resolve(&self, hash: &str) -> Option<KeyEntry> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.hash == hash)
            .cloned()
    }

    /// 全量推送本设备的 key 映射到 server（覆盖式；service-key 鉴权）。
    /// 设备未注册/远程未配置时返回 Err，调用方按可重试场景处理
    pub async fn sync_to_server(
        &self,
        http: &reqwest::Client,
        base: &str,
        service_key: &str,
        device_id: &str,
    ) -> Result<(), String> {
        let keys: Vec<serde_json::Value> = self
            .list()
            .iter()
            .map(|e| json!({ "s": e.hash, "read": e.read, "write": e.write }))
            .collect();
        let resp = http
            .put(format!("{base}/api/v1/devices/{device_id}/keys"))
            .bearer_auth(service_key)
            .json(&json!({ "keys": keys }))
            .send()
            .await
            .map_err(|e| format!("访问密钥同步失败: {e}"))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            let status = resp.status();
            let message = resp
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(String::from))
                .unwrap_or_else(|| status.to_string());
            Err(format!("访问密钥同步失败（{status}）: {message}"))
        }
    }

    fn save_locked(&self, entries: &[KeyEntry]) -> Result<(), String> {
        if let Some(parent) = self.file.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("访问密钥目录创建失败 {}: {e}", parent.display()))?;
        }
        let bytes =
            serde_json::to_vec_pretty(entries).map_err(|e| format!("访问密钥序列化失败: {e}"))?;
        let tmp = self.file.with_extension("json.tmp");
        std::fs::write(&tmp, bytes)
            .map_err(|e| format!("访问密钥写入失败 {}: {e}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &self.file)
            .map_err(|e| format!("访问密钥替换失败 {}: {e}", self.file.display()))
    }
}

/// 程序生成的 key：`hk_` + 四组八位十六进制（128 位随机；门令牌保护下不可在线爆破）
fn generate_key() -> String {
    format!(
        "hk_{:08x}-{:08x}-{:08x}-{:08x}",
        random_u64() as u32,
        random_u64() as u32,
        random_u64() as u32,
        random_u64() as u32
    )
}

fn random_u64() -> u64 {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).expect("系统随机源可用");
    u64::from_le_bytes(bytes)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn sha256_hex(text: &str) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_generated_and_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let store = KeyStore::open(dir.path().to_str().unwrap());
        let (entry, key) = store.create("插件", true, false, None).unwrap();
        assert!(key.starts_with("hk_"));
        assert_eq!(key.len(), "hk_".len() + 8 * 4 + 3);
        // 明文不落盘
        let raw = std::fs::read_to_string(dir.path().join("keys.json")).unwrap();
        assert!(!raw.contains(&key));
        // 验证子可解析权限
        let found = store.resolve(&entry.hash).unwrap();
        assert!(found.read && !found.write);
    }

    #[test]
    fn custom_key_length_rules() {
        let dir = tempfile::tempdir().unwrap();
        let store = KeyStore::open(dir.path().to_str().unwrap());
        // 纯读 4 字符即可
        assert!(store.create("a", true, false, Some("1234")).is_ok());
        // 带写权限不足 12 字符拒绝
        assert!(store.create("b", true, true, Some("short-key")).is_err());
        assert!(store.create("b", true, true, Some("123456789012")).is_ok());
        // 重复拒绝
        assert!(store.create("c", true, false, Some("1234")).is_err());
        // 权限全关拒绝
        assert!(store.create("d", false, false, None).is_err());
    }

    #[test]
    fn patch_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        let store = KeyStore::open(dir.path().to_str().unwrap());
        let (entry, _) = store.create("a", true, false, None).unwrap();
        let updated = store
            .patch(&entry.id, None, None, Some(true))
            .unwrap()
            .unwrap();
        assert!(updated.write);
        // 收窄回只读
        let updated = store
            .patch(&entry.id, None, None, Some(false))
            .unwrap()
            .unwrap();
        assert!(!updated.write);
        // 权限全关拒绝
        assert!(store
            .patch(&entry.id, None, Some(false), Some(false))
            .is_err());
        assert!(store.remove(&entry.id).unwrap());
        assert!(!store.remove(&entry.id).unwrap());
        // 重启后仍为空
        let reopened = KeyStore::open(dir.path().to_str().unwrap());
        assert!(reopened.list().is_empty());
    }
}

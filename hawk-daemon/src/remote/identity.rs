//! 设备身份：`remote/identity.json`（device_id + iroh 密钥对）。
//! 库级本地专用目录，不参与同步。endpoint_id（Ed25519 公钥）在服务端登记
//! （iroh 握手按公钥认证，无单独钉扎环节）；轮换身份 = 换钥后重新注册。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize)]
pub struct Identity {
    /// 服务端分配的设备 id（首次注册成功后回填）
    #[serde(default)]
    pub device_id: Option<String>,
    /// iroh SecretKey（hex）
    pub secret_key_hex: String,
    /// iroh EndpointId（公钥 hex）
    pub endpoint_id: String,
}

impl Identity {
    /// 读取设备身份；不存在则生成密钥对并落盘（device_id 留空待注册）
    pub fn load_or_create(remote_dir: &str) -> Result<Identity, String> {
        let file = identity_file(remote_dir);
        match std::fs::read(&file) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(identity) => Ok(identity),
                Err(e) => {
                    // 旧格式（证书时代字段）或损坏：备份后重新生成，不硬崩
                    let legacy = file.with_extension("json.legacy");
                    let _ = std::fs::rename(&file, &legacy);
                    tracing::warn!(
                        "设备身份文件格式过旧或损坏（{e}），已备份到 {} 并重新生成身份",
                        legacy.display()
                    );
                    let identity = Identity::generate();
                    identity.persist(remote_dir)?;
                    Ok(identity)
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let identity = Identity::generate();
                identity.persist(remote_dir)?;
                tracing::info!(
                    "已生成远程访问设备身份（endpoint {}）",
                    identity.endpoint_id
                );
                Ok(identity)
            }
            Err(e) => Err(format!("设备身份文件读取失败 {}: {e}", file.display())),
        }
    }

    /// 原子写回（tmp + rename）；私钥文件 owner 限读写（unix）
    pub fn persist(&self, remote_dir: &str) -> Result<(), String> {
        let file = identity_file(remote_dir);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("设备身份目录创建失败 {}: {e}", parent.display()))?;
        }
        let bytes =
            serde_json::to_vec_pretty(self).map_err(|e| format!("设备身份序列化失败: {e}"))?;
        let tmp = file.with_extension("json.tmp");
        std::fs::write(&tmp, bytes)
            .map_err(|e| format!("设备身份写入失败 {}: {e}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &file)
            .map_err(|e| format!("设备身份替换失败 {}: {e}", file.display()))
    }

    /// iroh SecretKey 还原
    pub fn secret_key(&self) -> Result<iroh::SecretKey, String> {
        let bytes = hex_decode(&self.secret_key_hex)?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| "设备私钥长度非法".to_string())?;
        Ok(iroh::SecretKey::from_bytes(&bytes))
    }

    /// 身份轮换：重新生成密钥对，device_id 保持不变（调用方负责同步服务端并落盘）
    pub fn rotate(&mut self) {
        let secret_key = iroh::SecretKey::generate();
        self.endpoint_id = secret_key.public().to_string();
        self.secret_key_hex = hex_encode(&secret_key.to_bytes());
    }

    fn generate() -> Identity {
        let secret_key = iroh::SecretKey::generate();
        Identity {
            device_id: None,
            endpoint_id: secret_key.public().to_string(),
            secret_key_hex: hex_encode(&secret_key.to_bytes()),
        }
    }
}

fn identity_file(remote_dir: &str) -> PathBuf {
    Path::new(remote_dir).join("identity.json")
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("hex 长度非法".to_string());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| format!("hex 解码失败: {e}")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_identity_file_regenerates() {
        // 证书时代的旧格式：解析失败 → 备份 + 重新生成，不硬崩
        let dir = tempfile::tempdir().unwrap();
        let remote_dir = dir.path().join("remote");
        std::fs::create_dir_all(&remote_dir).unwrap();
        std::fs::write(
            remote_dir.join("identity.json"),
            r#"{"device_id":"d_old","cert_der_b64":"x","key_der_b64":"y","fingerprint":"z"}"#,
        )
        .unwrap();
        let identity = Identity::load_or_create(remote_dir.to_str().unwrap()).unwrap();
        assert!(
            identity.device_id.is_none(),
            "旧格式不保留 device_id（重新注册）"
        );
        assert_eq!(identity.endpoint_id.len(), 64);
        assert!(remote_dir.join("identity.json.legacy").exists());
    }

    #[test]
    fn identity_persists_across_loads() {
        let dir = tempfile::tempdir().unwrap();
        let remote_dir = dir.path().join("remote");
        let remote_dir = remote_dir.to_str().unwrap();

        let first = Identity::load_or_create(remote_dir).unwrap();
        assert!(first.device_id.is_none());
        assert_eq!(first.endpoint_id.len(), 64);

        let mut registered = first.clone();
        registered.device_id = Some("d_test".to_string());
        registered.persist(remote_dir).unwrap();

        let loaded = Identity::load_or_create(remote_dir).unwrap();
        assert_eq!(loaded.device_id.as_deref(), Some("d_test"));
        assert_eq!(loaded.endpoint_id, first.endpoint_id);
        assert_eq!(loaded.secret_key_hex, first.secret_key_hex);
    }

    #[test]
    fn rotate_changes_endpoint_id_keeps_device_id() {
        let dir = tempfile::tempdir().unwrap();
        let remote_dir = dir.path().join("remote");
        let mut identity = Identity::load_or_create(remote_dir.to_str().unwrap()).unwrap();
        identity.device_id = Some("d_keep".to_string());
        let old_id = identity.endpoint_id.clone();
        let old_key = identity.secret_key_hex.clone();

        identity.rotate();
        assert_eq!(identity.device_id.as_deref(), Some("d_keep"));
        assert_ne!(identity.endpoint_id, old_id);
        assert_ne!(identity.secret_key_hex, old_key);
        assert_eq!(identity.endpoint_id.len(), 64);
    }
}

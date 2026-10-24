//! 服务端设备身份（网页版宿主）：data_dir/identity.json（device_id + iroh 密钥对）。
//! 服务端以「网页版」设备身份走既有协议发起会话，A 侧按常规对端裁决（allow_remote + 身份放行）。
//! 身份文件损坏时重新生成，不保留旧文件。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 网页版设备在设备列表中的显示名
pub const VIEWER_DEVICE_NAME: &str = "网页版 (hawk-server)";

#[derive(Clone, Serialize, Deserialize)]
pub struct Identity {
    #[serde(default)]
    pub device_id: Option<String>,
    /// iroh SecretKey（hex；iroh EndpointId 即其公钥，握手按公钥认证）
    pub secret_key_hex: String,
    pub endpoint_id: String,
}

impl Identity {
    pub fn load_or_create(data_dir: &Path) -> Result<Identity, String> {
        let file = identity_file(data_dir);
        match std::fs::read(&file) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(identity) => Ok(identity),
                Err(e) => {
                    tracing::warn!("设备身份文件损坏（{e}），重新生成身份");
                    let identity = Identity::generate();
                    identity.persist(data_dir)?;
                    Ok(identity)
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let identity = Identity::generate();
                identity.persist(data_dir)?;
                tracing::info!("已生成网页版设备身份（endpoint {}）", identity.endpoint_id);
                Ok(identity)
            }
            Err(e) => Err(format!("设备身份文件读取失败 {}: {e}", file.display())),
        }
    }

    pub fn persist(&self, data_dir: &Path) -> Result<(), String> {
        let file = identity_file(data_dir);
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

    fn generate() -> Identity {
        let secret_key = iroh::SecretKey::generate();
        Identity {
            device_id: None,
            endpoint_id: secret_key.public().to_string(),
            secret_key_hex: hex_encode(&secret_key.to_bytes()),
        }
    }
}

fn identity_file(data_dir: &Path) -> PathBuf {
    data_dir.join("identity.json")
}

pub fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("hex 长度非法".to_string());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| format!("hex 解码失败: {e}")))
        .collect()
}

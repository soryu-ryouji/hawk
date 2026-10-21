//! 设备身份：`.hawk/remote/identity.json`（device_id + QUIC 自签证书与指纹）。
//! 库级本地专用目录，不参与同步。证书指纹在服务端登记（TOFU 钉扎的锚点）；
//! 轮换身份 = 删除该文件后重新注册。

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize)]
pub struct Identity {
    /// 服务端分配的设备 id（首次注册成功后回填）
    #[serde(default)]
    pub device_id: Option<String>,
    /// 自签名证书 DER（base64）：数据面 QUIC 身份，指纹 = sha256(cert_der)
    pub cert_der_b64: String,
    /// 证书私钥 DER（base64，PKCS#8）
    pub key_der_b64: String,
    pub fingerprint: String,
}

impl Identity {
    /// 读取设备身份；不存在则生成自签证书并落盘（device_id 留空待注册）
    pub fn load_or_create(remote_dir: &str) -> Result<Identity, String> {
        let file = identity_file(remote_dir);
        match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| format!("设备身份文件解析失败 {}: {e}", file.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let identity = Identity::generate()?;
                identity.persist(remote_dir)?;
                tracing::info!("已生成远程访问设备身份（指纹 {}）", identity.fingerprint);
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

    /// 证书轮换：重新生成自签证书与指纹，device_id 保持不变（调用方负责同步服务端并落盘）
    pub fn rotate_cert(&mut self) -> Result<(), String> {
        let (cert_der_b64, key_der_b64, fingerprint) = generate_cert()?;
        self.cert_der_b64 = cert_der_b64;
        self.key_der_b64 = key_der_b64;
        self.fingerprint = fingerprint;
        Ok(())
    }

    fn generate() -> Result<Identity, String> {
        let (cert_der_b64, key_der_b64, fingerprint) = generate_cert()?;
        Ok(Identity {
            device_id: None,
            cert_der_b64,
            key_der_b64,
            fingerprint,
        })
    }
}

/// 生成自签证书材料：(证书 DER b64, 私钥 DER b64, 指纹 hex)
fn generate_cert() -> Result<(String, String, String), String> {
    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(vec!["hawk-remote".to_string()])
            .map_err(|e| format!("设备证书生成失败: {e}"))?;
    let cert_der = cert.der();
    let fingerprint = hex(sha2::Sha256::digest(cert_der));
    Ok((
        base64::engine::general_purpose::STANDARD.encode(cert_der),
        base64::engine::general_purpose::STANDARD.encode(key_pair.serialize_der()),
        fingerprint,
    ))
}

fn identity_file(remote_dir: &str) -> PathBuf {
    Path::new(remote_dir).join("identity.json")
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_persists_across_loads() {
        let dir = tempfile::tempdir().unwrap();
        let remote_dir = dir.path().join("remote");
        let remote_dir = remote_dir.to_str().unwrap();

        let first = Identity::load_or_create(remote_dir).unwrap();
        assert!(first.device_id.is_none());
        assert_eq!(first.fingerprint.len(), 64);

        // 注册回填 device_id 后落盘
        let mut registered = first.clone();
        registered.device_id = Some("d_test".to_string());
        registered.persist(remote_dir).unwrap();

        let loaded = Identity::load_or_create(remote_dir).unwrap();
        assert_eq!(loaded.device_id.as_deref(), Some("d_test"));
        assert_eq!(loaded.fingerprint, first.fingerprint);
        assert_eq!(loaded.cert_der_b64, first.cert_der_b64);
    }

    #[test]
    fn rotate_cert_changes_fingerprint_keeps_device_id() {
        let dir = tempfile::tempdir().unwrap();
        let remote_dir = dir.path().join("remote");
        let mut identity = Identity::load_or_create(remote_dir.to_str().unwrap()).unwrap();
        identity.device_id = Some("d_keep".to_string());
        let old_fingerprint = identity.fingerprint.clone();
        let old_cert = identity.cert_der_b64.clone();

        identity.rotate_cert().unwrap();
        assert_eq!(identity.device_id.as_deref(), Some("d_keep"));
        assert_ne!(identity.fingerprint, old_fingerprint);
        assert_ne!(identity.cert_der_b64, old_cert);
        assert_eq!(identity.fingerprint.len(), 64);
    }
}

//! 服务端设备身份（网页版宿主）：data_dir/identity.json（device_id + QUIC 自签证书与指纹）。
//! 服务端以「网页版」设备身份走既有协议发起会话，A 侧按常规对端裁决（allow_remote + 指纹放行）。

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::path::{Path, PathBuf};

/// 网页版设备在设备列表中的显示名
pub const VIEWER_DEVICE_NAME: &str = "网页版 (hawk-server)";

#[derive(Clone, Serialize, Deserialize)]
pub struct Identity {
    #[serde(default)]
    pub device_id: Option<String>,
    pub cert_der_b64: String,
    pub key_der_b64: String,
    pub fingerprint: String,
}

impl Identity {
    pub fn load_or_create(data_dir: &Path) -> Result<Identity, String> {
        let file = identity_file(data_dir);
        match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| format!("设备身份文件解析失败 {}: {e}", file.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let identity = Identity::generate()?;
                identity.persist(data_dir)?;
                tracing::info!("已生成网页版设备身份（指纹 {}）", identity.fingerprint);
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

    /// base64 DER 还原为 quinn 证书链与私钥
    pub fn der(
        &self,
    ) -> Result<
        (
            Vec<quinn::rustls::pki_types::CertificateDer<'static>>,
            quinn::rustls::pki_types::PrivateKeyDer<'static>,
        ),
        String,
    > {
        let cert = base64::engine::general_purpose::STANDARD
            .decode(&self.cert_der_b64)
            .map_err(|e| format!("设备证书解码失败: {e}"))?;
        let key = base64::engine::general_purpose::STANDARD
            .decode(&self.key_der_b64)
            .map_err(|e| format!("设备私钥解码失败: {e}"))?;
        Ok((
            vec![quinn::rustls::pki_types::CertificateDer::from(cert)],
            quinn::rustls::pki_types::PrivatePkcs8KeyDer::from(key).into(),
        ))
    }

    fn generate() -> Result<Identity, String> {
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(vec!["hawk-server-viewer".to_string()])
                .map_err(|e| format!("设备证书生成失败: {e}"))?;
        let cert_der = cert.der();
        let fingerprint: String = sha2::Sha256::digest(cert_der)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Ok(Identity {
            device_id: None,
            cert_der_b64: base64::engine::general_purpose::STANDARD.encode(cert_der),
            key_der_b64: base64::engine::general_purpose::STANDARD
                .encode(signing_key.serialize_der()),
            fingerprint,
        })
    }
}

fn identity_file(data_dir: &Path) -> PathBuf {
    data_dir.join("identity.json")
}

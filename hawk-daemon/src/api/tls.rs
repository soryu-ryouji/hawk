//! LAN Web TLS 身份：应用级自签证书、私钥与 SHA-256 指纹。

use axum_server::tls_rustls::RustlsConfig;
use sha2::Digest;
use std::path::{Path, PathBuf};

pub struct LanTls {
    pub config: RustlsConfig,
    pub fingerprint: String,
}

pub async fn load_or_create(identity_dir: &str, names: Vec<String>) -> Result<LanTls, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cert_file = tls_file(identity_dir, "lan-tls-cert.pem");
    let key_file = tls_file(identity_dir, "lan-tls-key.pem");
    if cert_file.is_file() && key_file.is_file() {
        let cert = std::fs::read(&cert_file).map_err(|e| format!("读取 LAN 证书失败: {e}"))?;
        let key = std::fs::read(&key_file).map_err(|e| format!("读取 LAN 私钥失败: {e}"))?;
        let config = RustlsConfig::from_pem(cert.clone(), key)
            .await
            .map_err(|e| format!("加载 LAN TLS 配置失败: {e}"))?;
        return Ok(LanTls {
            config,
            fingerprint: fingerprint(&cert),
        });
    }

    let mut names = names;
    names.extend([
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ]);
    names.sort();
    names.dedup();
    let params =
        rcgen::CertificateParams::new(names).map_err(|e| format!("LAN 证书参数错误: {e}"))?;
    let keypair = rcgen::KeyPair::generate().map_err(|e| format!("LAN 私钥生成失败: {e}"))?;
    let cert = params
        .self_signed(&keypair)
        .map_err(|e| format!("LAN 证书生成失败: {e}"))?;
    let cert_pem = cert.pem();
    let key_pem = keypair.serialize_pem();
    if let Some(parent) = cert_file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建 LAN 身份目录失败: {e}"))?;
    }
    std::fs::write(&cert_file, cert_pem.as_bytes())
        .map_err(|e| format!("写入 LAN 证书失败: {e}"))?;
    std::fs::write(&key_file, key_pem).map_err(|e| format!("写入 LAN 私钥失败: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600));
    }
    let fingerprint = fingerprint(cert_pem.as_bytes());
    let config =
        RustlsConfig::from_pem(cert_pem.into_bytes(), keypair.serialize_pem().into_bytes())
            .await
            .map_err(|e| format!("加载 LAN TLS 配置失败: {e}"))?;
    Ok(LanTls {
        config,
        fingerprint,
    })
}

pub fn fingerprint_if_exists(identity_dir: &str) -> Option<String> {
    let cert = std::fs::read(tls_file(identity_dir, "lan-tls-cert.pem")).ok()?;
    Some(fingerprint(&cert))
}

fn tls_file(identity_dir: &str, name: &str) -> PathBuf {
    Path::new(identity_dir).join(name)
}

fn fingerprint(cert_der: &[u8]) -> String {
    sha2::Sha256::digest(cert_der)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn certificate_is_reused_and_fingerprint_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        let first = load_or_create(&root, vec!["192.168.1.2".to_string()])
            .await
            .unwrap();
        let second = load_or_create(&root, vec!["192.168.1.2".to_string()])
            .await
            .unwrap();
        assert_eq!(first.fingerprint, second.fingerprint);
        assert_eq!(first.fingerprint.len(), 64);
    }
}

//! QUIC 双向指纹钉扎：对端身份 = sha256(自签证书 DER)，经控制面交换（TOFU）。
//! B 侧钉扎 A 的指纹（会话 ready 时服务端下发）；A 侧放行「已接受会话的对端」指纹集合。

use crate::remote::identity::Identity;
use base64::Engine;
use quinn::rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, UnixTime};
use quinn::rustls::{self, SignatureScheme};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified};
use rustls::server::danger::ClientCertVerified;
use sha2::Digest;
use std::collections::HashSet;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// 隧道 ALPN
pub const ALPN: &[u8] = b"hawk-remote/1";

pub fn fingerprint_of(cert_der: &[u8]) -> [u8; 32] {
    sha2::Sha256::digest(cert_der).into()
}

pub fn parse_fingerprint(hex_str: &str) -> Option<[u8; 32]> {
    let bytes: Vec<u8> = (0..hex_str.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex_str.get(i..i + 2)?, 16).ok())
        .collect::<Option<_>>()?;
    bytes.try_into().ok()
}

/// identity.json 的 base64 DER 还原为 quinn 所需的证书链与私钥
fn identity_der(
    identity: &Identity,
) -> Result<
    (
        Vec<CertificateDer<'static>>,
        rustls::pki_types::PrivateKeyDer<'static>,
    ),
    String,
> {
    let cert = base64::engine::general_purpose::STANDARD
        .decode(&identity.cert_der_b64)
        .map_err(|e| format!("设备证书解码失败: {e}"))?;
    let key = base64::engine::general_purpose::STANDARD
        .decode(&identity.key_der_b64)
        .map_err(|e| format!("设备私钥解码失败: {e}"))?;
    Ok((
        vec![CertificateDer::from(cert)],
        PrivatePkcs8KeyDer::from(key).into(),
    ))
}

/// 长会话传输参数：QUIC 默认 idle 超时过短（隧道内 SSE 等长连接会被掐断）
fn transport_config() -> Arc<quinn::TransportConfig> {
    let mut config = quinn::TransportConfig::default();
    config.keep_alive_interval(Some(Duration::from_secs(10)));
    config.max_idle_timeout(Some(
        quinn::IdleTimeout::try_from(Duration::from_secs(60)).expect("idle timeout 合法"),
    ));
    Arc::new(config)
}

/// A 侧服务端配置：强制对端出示证书，且指纹在「已接受会话」放行集合内
pub fn server_config(
    identity: &Identity,
    allowlist: Arc<PeerAllowlist>,
) -> Result<quinn::ServerConfig, String> {
    let (certs, key) = identity_der(identity)?;
    let mut tls = rustls::ServerConfig::builder()
        .with_client_cert_verifier(Arc::new(ClientAllowlistVerifier { allowlist }))
        .with_single_cert(certs, key)
        .map_err(|e| format!("隧道服务端证书配置失败: {e}"))?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls)
        .map_err(|e| format!("隧道 QUIC 配置失败: {e}"))?;
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    config.transport = transport_config();
    Ok(config)
}

/// B 侧客户端配置：钉扎 A 的证书指纹（会话 ready 下发值），同时出示自身证书
pub fn client_config(
    identity: &Identity,
    expected: [u8; 32],
) -> Result<quinn::ClientConfig, String> {
    let (certs, key) = identity_der(identity)?;
    let mut tls = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(ServerPinVerifier { expected }))
        .with_client_auth_cert(certs, key)
        .map_err(|e| format!("隧道客户端证书配置失败: {e}"))?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls)
        .map_err(|e| format!("隧道 QUIC 配置失败: {e}"))?;
    let mut config = quinn::ClientConfig::new(Arc::new(crypto));
    config.transport_config(transport_config());
    Ok(config)
}

// ---------- 放行集合（A 侧：已接受会话的对端指纹） ----------

#[derive(Debug, Default)]
pub struct PeerAllowlist {
    inner: RwLock<HashSet<[u8; 32]>>,
}

impl PeerAllowlist {
    pub fn add(&self, fingerprint: [u8; 32]) {
        self.inner.write().unwrap().insert(fingerprint);
    }

    pub fn remove(&self, fingerprint: &[u8; 32]) {
        self.inner.write().unwrap().remove(fingerprint);
    }

    pub fn clear(&self) {
        self.inner.write().unwrap().clear();
    }

    fn contains(&self, fingerprint: &[u8; 32]) -> bool {
        self.inner.read().unwrap().contains(fingerprint)
    }
}

// ---------- rustls verifier 实现（仅 TLS 1.3；验签走 ring，身份裁决即指纹比对） ----------

fn invalid_cert() -> rustls::Error {
    rustls::Error::InvalidCertificate(rustls::CertificateError::ApplicationVerificationFailure)
}

fn signature_schemes() -> Vec<SignatureScheme> {
    rustls::crypto::ring::default_provider()
        .signature_verification_algorithms
        .supported_schemes()
}

fn verify_tls13(
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &rustls::DigitallySignedStruct,
) -> Result<HandshakeSignatureValid, rustls::Error> {
    let provider = rustls::crypto::ring::default_provider();
    rustls::crypto::verify_tls13_signature(
        message,
        cert,
        dss,
        &provider.signature_verification_algorithms,
    )
}

fn reject_tls12() -> rustls::Error {
    rustls::Error::PeerIncompatible(rustls::PeerIncompatible::Tls12NotOffered)
}

/// B 侧：钉扎 A（服务端）证书指纹
#[derive(Debug)]
struct ServerPinVerifier {
    expected: [u8; 32],
}

impl rustls::client::danger::ServerCertVerifier for ServerPinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if fingerprint_of(end_entity.as_ref()) == self.expected {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(invalid_cert())
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(reject_tls12())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        signature_schemes()
    }
}

/// A 侧：放行集合内的 B（客户端）证书指纹
#[derive(Debug)]
struct ClientAllowlistVerifier {
    allowlist: Arc<PeerAllowlist>,
}

impl rustls::server::danger::ClientCertVerifier for ClientAllowlistVerifier {
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }

    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        if self
            .allowlist
            .contains(&fingerprint_of(end_entity.as_ref()))
        {
            Ok(ClientCertVerified::assertion())
        } else {
            Err(invalid_cert())
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(reject_tls12())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        signature_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_fingerprint_roundtrip() {
        let fp = fingerprint_of(b"cert-bytes");
        let hex_str: String = fp.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(parse_fingerprint(&hex_str), Some(fp));
        assert_eq!(parse_fingerprint("zz"), None);
        assert_eq!(parse_fingerprint("abcd"), None); // 长度不足 32 字节
    }

    #[test]
    fn allowlist_add_remove() {
        let list = PeerAllowlist::default();
        let fp = fingerprint_of(b"a");
        list.add(fp);
        assert!(list.contains(&fp));
        list.remove(&fp);
        assert!(!list.contains(&fp));
    }
}

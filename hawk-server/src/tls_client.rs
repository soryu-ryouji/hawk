//! QUIC 客户端配置（网页版宿主 = 协议里的 B 侧）：钉扎 A 侧证书指纹（TOFU），出示自身证书。

use crate::identity::Identity;
use quinn::rustls::pki_types::{CertificateDer, UnixTime};
use quinn::rustls::{self, SignatureScheme};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified};
use sha2::Digest;
use std::sync::Arc;
use std::time::Duration;

const ALPN: &[u8] = b"hawk-remote/1";

/// 构建钉扎指定指纹的客户端配置（expected = 会话 ready 下发的目标指纹）
pub fn client_config(
    identity: &Identity,
    expected: [u8; 32],
) -> Result<quinn::ClientConfig, String> {
    let (certs, key) = identity.der()?;
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

/// 长会话传输参数（同 daemon 数据面：默认 idle 超时过短会掐断隧道内 SSE）
fn transport_config() -> Arc<quinn::TransportConfig> {
    let mut config = quinn::TransportConfig::default();
    config.keep_alive_interval(Some(Duration::from_secs(10)));
    config.max_idle_timeout(Some(
        quinn::IdleTimeout::try_from(Duration::from_secs(60)).expect("idle timeout 合法"),
    ));
    Arc::new(config)
}

fn invalid_cert() -> rustls::Error {
    rustls::Error::InvalidCertificate(rustls::CertificateError::ApplicationVerificationFailure)
}

fn signature_schemes() -> Vec<SignatureScheme> {
    rustls::crypto::ring::default_provider()
        .signature_verification_algorithms
        .supported_schemes()
}

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
        let digest: [u8; 32] = sha2::Sha256::digest(end_entity.as_ref()).into();
        if digest == self.expected {
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
        Err(rustls::Error::PeerIncompatible(
            rustls::PeerIncompatible::Tls12NotOffered,
        ))
    }

    fn verify_tls13_signature(
        &self,
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

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        signature_schemes()
    }
}

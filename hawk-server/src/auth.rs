//! 控制面认证：tenant_key 换 TenantAdmin Session，设备私钥签名换 Device Session。
//! Session 只存 SHA-256 验证子，明文仅由认证端点返回一次；nonce 短期且单次使用。

use crate::api::ApiError;
use crate::state::AppState;
use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{header, StatusCode};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const TENANT_SESSION_TTL: Duration = Duration::from_secs(2 * 60 * 60);
pub const DEVICE_SESSION_TTL: Duration = Duration::from_secs(24 * 60 * 60);
pub const DEVICE_CHALLENGE_TTL: Duration = Duration::from_secs(5 * 60);
pub const DEVICE_SIGNATURE_DOMAIN: &[u8] = b"hawk-device-auth-v2\0";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Principal {
    TenantAdmin {
        tenant_id: String,
    },
    Device {
        tenant_id: String,
        device_id: String,
    },
}

struct AuthSession {
    principal: Principal,
    expires: Instant,
}

struct Challenge {
    device_id: String,
    expires: Instant,
    expires_at_ms: u64,
}

#[derive(Default)]
pub struct AuthState {
    sessions: Mutex<HashMap<String, AuthSession>>,
    challenges: Mutex<HashMap<String, Challenge>>,
}

impl AuthState {
    pub fn issue_tenant(&self, tenant_id: String) -> String {
        self.issue(Principal::TenantAdmin { tenant_id }, TENANT_SESSION_TTL)
    }

    pub fn issue_device(&self, tenant_id: String, device_id: String) -> String {
        self.issue(
            Principal::Device {
                tenant_id,
                device_id,
            },
            DEVICE_SESSION_TTL,
        )
    }

    fn issue(&self, principal: Principal, ttl: Duration) -> String {
        let token = random_token();
        self.sessions.lock().unwrap().insert(
            crate::keystore::sha256_hex(&token),
            AuthSession {
                principal,
                expires: Instant::now() + ttl,
            },
        );
        token
    }

    pub fn issue_challenge(&self, device_id: String) -> (String, u64) {
        let nonce = random_token();
        let expires = Instant::now() + DEVICE_CHALLENGE_TTL;
        let expires_at_ms = crate::state::now_ms() + DEVICE_CHALLENGE_TTL.as_millis() as u64;
        let mut challenges = self.challenges.lock().unwrap();
        challenges.retain(|_, challenge| challenge.expires > Instant::now());
        challenges.insert(
            nonce.clone(),
            Challenge {
                device_id,
                expires,
                expires_at_ms,
            },
        );
        (nonce, expires_at_ms)
    }

    /// 消费 nonce：无论后续签名是否成功都不能重放。
    pub fn consume_challenge(&self, nonce: &str, device_id: &str) -> Option<u64> {
        let challenge = self.challenges.lock().unwrap().remove(nonce)?;
        if challenge.device_id != device_id || challenge.expires <= Instant::now() {
            return None;
        }
        Some(challenge.expires_at_ms)
    }

    fn resolve(&self, token: &str) -> Option<Principal> {
        let hash = crate::keystore::sha256_hex(token);
        let mut sessions = self.sessions.lock().unwrap();
        let now = Instant::now();
        sessions.retain(|_, session| session.expires > now);
        sessions.get(&hash).map(|session| session.principal.clone())
    }

    pub fn revoke_device(&self, device_id: &str) {
        self.sessions.lock().unwrap().retain(|_, session| {
            !matches!(
                &session.principal,
                Principal::Device { device_id: id, .. } if id == device_id
            )
        });
        self.challenges
            .lock()
            .unwrap()
            .retain(|_, challenge| challenge.device_id != device_id);
    }
}

/// TenantAdmin API 提取器：只接受 audience=tenant_control 的 Session。
pub struct TenantAdmin {
    pub tenant_id: String,
}

impl FromRequestParts<Arc<AppState>> for TenantAdmin {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        match authenticate(parts, state)? {
            Principal::TenantAdmin { tenant_id } if state.keys.tenant(&tenant_id).is_some() => {
                Ok(TenantAdmin { tenant_id })
            }
            _ => Err(unauthorized()),
        }
    }
}

/// Device API 提取器：只接受 audience=device_control 的 Session，并重新校验设备与租户。
pub struct DeviceAuth {
    pub tenant_id: String,
    pub device_id: String,
}

impl FromRequestParts<Arc<AppState>> for DeviceAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        match authenticate(parts, state)? {
            Principal::Device {
                tenant_id,
                device_id,
            } if state.keys.tenant(&tenant_id).is_some()
                && state.device(&device_id).is_some_and(|device| {
                    device.tenant_id == tenant_id && device.device_id == device_id
                }) =>
            {
                Ok(DeviceAuth {
                    tenant_id,
                    device_id,
                })
            }
            _ => Err(unauthorized()),
        }
    }
}

fn authenticate(parts: &Parts, state: &Arc<AppState>) -> Result<Principal, ApiError> {
    if let Some(ip) = peer_ip(parts) {
        if let Some(secs) = state.ratelimit.check_ip(ip) {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                format!("验证失败次数过多，请 {secs} 秒后再试"),
            ));
        }
    }
    let token = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let principal = token.and_then(|token| state.auth.resolve(token));
    if principal.is_none() {
        if let Some(ip) = peer_ip(parts) {
            state.ratelimit.fail_ip(ip);
        }
        return Err(unauthorized());
    }
    Ok(principal.unwrap())
}

fn peer_ip(parts: &Parts) -> Option<std::net::IpAddr> {
    parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip())
}

fn unauthorized() -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, "会话无效或已过期")
}

pub fn device_signature_message(device_id: &str, nonce: &str, expires_at_ms: u64) -> Vec<u8> {
    let mut message = Vec::with_capacity(
        DEVICE_SIGNATURE_DOMAIN.len() + 8 + device_id.len() + 8 + nonce.len() + 8,
    );
    message.extend_from_slice(DEVICE_SIGNATURE_DOMAIN);
    message.extend_from_slice(&(device_id.len() as u64).to_be_bytes());
    message.extend_from_slice(device_id.as_bytes());
    message.extend_from_slice(&(nonce.len() as u64).to_be_bytes());
    message.extend_from_slice(nonce.as_bytes());
    message.extend_from_slice(&expires_at_ms.to_be_bytes());
    message
}

fn random_token() -> String {
    format!(
        "ts_{:016x}{:016x}{:016x}{:016x}",
        rand::random::<u64>(),
        rand::random::<u64>(),
        rand::random::<u64>(),
        rand::random::<u64>()
    )
}

/// 恒定时间比较，避免时序侧信道泄露密钥。
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_is_single_use_and_bound_to_device() {
        let auth = AuthState::default();
        let (nonce, _) = auth.issue_challenge("d_1".into());
        assert_eq!(auth.consume_challenge(&nonce, "d_other"), None);
        let (nonce, expires) = auth.issue_challenge("d_1".into());
        assert_eq!(auth.consume_challenge(&nonce, "d_1"), Some(expires));
        assert_eq!(auth.consume_challenge(&nonce, "d_1"), None);
    }

    #[test]
    fn device_revocation_removes_sessions_and_challenges() {
        let auth = AuthState::default();
        let token = auth.issue_device("t_1".into(), "d_1".into());
        let _ = auth.issue_challenge("d_1".into());
        auth.revoke_device("d_1");
        assert_eq!(auth.resolve(&token), None);
    }

    #[test]
    fn signature_message_is_domain_separated_and_stable() {
        let message = device_signature_message("d_1", "n_1", 42);
        assert!(message.starts_with(DEVICE_SIGNATURE_DOMAIN));
        assert_eq!(message, device_signature_message("d_1", "n_1", 42));
        assert_ne!(message, device_signature_message("d_2", "n_1", 42));
    }
}

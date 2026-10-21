//! 控制面鉴权：Bearer service-key → 租户。验证失败按来源 IP 计入风控（冷却中直接 429）。

use crate::api::ApiError;
use crate::state::AppState;
use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{header, StatusCode};
use std::net::SocketAddr;
use std::sync::Arc;

/// REST 鉴权提取器：`Authorization: Bearer <service_key>` → 解析为所属租户
pub struct ServiceKey {
    /// 该 service-key 所属租户（池）。设备/会话等资源按租户隔离
    pub tenant_id: String,
}

impl FromRequestParts<Arc<AppState>> for ServiceKey {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let peer_ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| addr.ip());
        if let Some(ip) = peer_ip {
            if let Some(secs) = state.ratelimit.check_ip(ip) {
                return Err(ApiError::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    format!("验证失败次数过多，请 {secs} 秒后再试"),
                ));
            }
        }
        let key = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        let tenant = key.and_then(|key| state.keys.resolve_tenant(key));
        match tenant {
            Some(tenant) => Ok(ServiceKey {
                tenant_id: tenant.id,
            }),
            None => {
                if let Some(ip) = peer_ip {
                    state.ratelimit.fail_ip(ip);
                }
                Err(ApiError::new(StatusCode::UNAUTHORIZED, "服务密钥无效"))
            }
        }
    }
}

/// 恒定时间比较，避免时序侧信道泄露密钥
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

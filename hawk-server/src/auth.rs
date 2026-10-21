//! 鉴权：单用户方案下全部端点共用一把固定 access key（启动时经 CLI/env 配置）。

use crate::api::ApiError;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{header, StatusCode};
use std::sync::Arc;

/// REST 鉴权提取器：`Authorization: Bearer <access_key>`
pub struct AccessKey;

impl FromRequestParts<Arc<AppState>> for AccessKey {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let key = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        match key {
            Some(key) if constant_time_eq(key.as_bytes(), state.access_key.as_bytes()) => {
                Ok(AccessKey)
            }
            _ => Err(ApiError::new(StatusCode::UNAUTHORIZED, "访问密钥无效")),
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

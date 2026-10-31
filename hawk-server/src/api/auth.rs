//! 控制面认证端点：tenant_key / 设备签名挑战 → 有期 Session。

use super::{ok, ApiError, JsonBody};
use crate::auth::{device_signature_message, DEVICE_SESSION_TTL, TENANT_SESSION_TTL};
use crate::state::AppState;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::{Json, Router};
use base64::Engine;
use serde::Deserialize;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/tenant/session",
            axum::routing::post(tenant_session),
        )
        .route(
            "/api/v1/device/challenge",
            axum::routing::post(device_challenge),
        )
        .route(
            "/api/v1/device/session",
            axum::routing::post(device_session),
        )
}

#[derive(Deserialize)]
struct TenantSessionBody {
    tenant_key: String,
}

async fn tenant_session(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    JsonBody(body): JsonBody<TenantSessionBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if let Some(secs) = state.ratelimit.check_ip(peer.ip()) {
        return Err(rate_limited(secs));
    }
    let Some(tenant) = state.keys.resolve_tenant(&body.tenant_key) else {
        state.ratelimit.fail_ip(peer.ip());
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "租户密钥无效"));
    };
    let token = state.auth.issue_tenant(tenant.id.clone());
    Ok(ok(json!({
        "session_token": token,
        "tenant_id": tenant.id,
        "expires_in": TENANT_SESSION_TTL.as_secs(),
    })))
}

#[derive(Deserialize)]
struct DeviceChallengeBody {
    device_id: String,
}

async fn device_challenge(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    JsonBody(body): JsonBody<DeviceChallengeBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if let Some(secs) = state.ratelimit.check_ip(peer.ip()) {
        return Err(rate_limited(secs));
    }
    let Some(device) = state.device(&body.device_id) else {
        state.ratelimit.fail_ip(peer.ip());
        return Err(ApiError::not_found("设备不存在"));
    };
    if state.keys.tenant(&device.tenant_id).is_none() {
        state.ratelimit.fail_ip(peer.ip());
        return Err(ApiError::not_found("设备不存在"));
    }
    let (nonce, expires_at_ms) = state.auth.issue_challenge(device.device_id);
    Ok(ok(
        json!({ "nonce": nonce, "expires_at_ms": expires_at_ms }),
    ))
}

#[derive(Deserialize)]
struct DeviceSessionBody {
    device_id: String,
    nonce: String,
    /// base64 编码的 Ed25519 签名。
    signature: String,
}

async fn device_session(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    JsonBody(body): JsonBody<DeviceSessionBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if let Some(secs) = state.ratelimit.check_ip(peer.ip()) {
        return Err(rate_limited(secs));
    }
    let result = verify_device_session(&state, &body);
    if result.is_none() {
        state.ratelimit.fail_ip(peer.ip());
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "设备签名验证失败"));
    }
    let tenant_id = result.unwrap();
    let token = state
        .auth
        .issue_device(tenant_id.clone(), body.device_id.clone());
    Ok(ok(json!({
        "session_token": token,
        "tenant_id": tenant_id,
        "device_id": body.device_id,
        "expires_in": DEVICE_SESSION_TTL.as_secs(),
    })))
}

fn verify_device_session(state: &AppState, body: &DeviceSessionBody) -> Option<String> {
    let device = state.device(&body.device_id)?;
    state.keys.tenant(&device.tenant_id)?;
    let expires_at_ms = state.auth.consume_challenge(&body.nonce, &body.device_id)?;
    let signature = base64::engine::general_purpose::STANDARD
        .decode(body.signature.as_bytes())
        .ok()?;
    let signature = iroh::Signature::try_from(signature.as_slice()).ok()?;
    let endpoint_id = device.endpoint_id.parse::<iroh::EndpointId>().ok()?;
    let message = device_signature_message(&body.device_id, &body.nonce, expires_at_ms);
    endpoint_id.verify(&message, &signature).ok()?;
    Some(device.tenant_id)
}

fn rate_limited(secs: u64) -> ApiError {
    ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        format!("验证失败次数过多，请 {secs} 秒后再试"),
    )
}

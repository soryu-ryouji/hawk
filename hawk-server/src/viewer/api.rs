//! 网页版查看面 API：两级串联验证（service-key → 门令牌 → hawk-app key → bt）；
//! bt 鉴权后的查看面。gate/login 按 IP + 租户走风控。

use super::ViewerManager;
use crate::api::{ok, ApiError, JsonBody};
use crate::state::Access;
use axum::extract::{ConnectInfo, FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::Arc;

pub fn routes() -> Router<Arc<ViewerManager>> {
    Router::new()
        .route("/api/v1/viewer/gate", post(gate))
        .route("/api/v1/viewer/login", post(login))
        .route("/api/v1/viewer/logout", post(logout))
        .route("/api/v1/viewer/devices", get(devices))
        .route(
            "/api/v1/viewer/session",
            post(open_session).get(current_session).delete(disconnect),
        )
}

/// 浏览器令牌鉴权提取器：Bearer 头或 ?token= 查询参数（SPA 的 img/SSE 场景）
pub struct BrowserAuth {
    pub token: String,
    pub access: Access,
    /// bt 绑定的目标设备（登录时由 hawk-app key 映射确定）
    pub device_id: String,
}

impl FromRequestParts<Arc<ViewerManager>> for BrowserAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        manager: &Arc<ViewerManager>,
    ) -> Result<Self, Self::Rejection> {
        let bearer = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        let query = parts.uri.query().and_then(|q| {
            q.split('&')
                .filter_map(|pair| pair.split_once('='))
                .find(|(k, _)| *k == "token")
                .map(|(_, v)| v.to_string())
        });
        let token = bearer.map(String::from).or(query);
        let Some(token) = token else {
            return Err(ApiError::new(StatusCode::UNAUTHORIZED, "未登录"));
        };
        let Some(grant) = manager.validate(&token) else {
            return Err(ApiError::new(StatusCode::UNAUTHORIZED, "令牌无效或已注销"));
        };
        Ok(BrowserAuth {
            token,
            access: grant.access,
            device_id: grant.device_id,
        })
    }
}

#[derive(Deserialize)]
struct GateBody {
    /// service-key（第一级：池门禁）
    service_key: String,
}

/// `POST /api/v1/viewer/gate`：service-key 换门令牌（短时效 + 有限尝试预算）。
/// 失败按来源 IP 计风控；冷却中直接 429
async fn gate(
    State(manager): State<Arc<ViewerManager>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    JsonBody(body): JsonBody<GateBody>,
) -> Result<Json<Value>, ApiError> {
    let limiter = &manager.app_state().ratelimit;
    if let Some(secs) = limiter.check_ip(peer.ip()) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!("验证失败次数过多，请 {secs} 秒后再试"),
        ));
    }
    let Some(gate_token) = manager.gate(&body.service_key) else {
        limiter.fail_ip(peer.ip());
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "服务密钥无效"));
    };
    Ok(ok(serde_json::json!({
        "gate_token": gate_token,
        "expires_in": crate::keystore::GATE_TTL.as_secs(),
    })))
}

#[derive(Deserialize)]
struct LoginBody {
    /// 门令牌（gate 换发）
    gate_token: String,
    /// hawk-app key（第二级：定位到具体素材库，携带读/写权限）
    key: String,
}

/// `POST /api/v1/viewer/login`：门令牌 + hawk-app key 换发浏览器令牌（bt）。
/// 失败按来源 IP 与租户（池）双维度计风控
async fn login(
    State(manager): State<Arc<ViewerManager>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    JsonBody(body): JsonBody<LoginBody>,
) -> Result<Json<Value>, ApiError> {
    let limiter = &manager.app_state().ratelimit;
    if let Some(secs) = limiter.check_ip(peer.ip()) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!("验证失败次数过多，请 {secs} 秒后再试"),
        ));
    }
    let tenant = manager.gate_tenant(&body.gate_token);
    // 池冷却检查（门令牌有效才能确定池；无效门令牌只计 IP）
    if let Some(tenant_id) = &tenant {
        if let Some(secs) = limiter.check_tenant(tenant_id) {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                format!("该服务的访问密钥验证失败过多，请 {secs} 秒后再试"),
            ));
        }
    }
    let Some((token, grant)) = manager.login(&body.gate_token, &body.key) else {
        limiter.fail_ip(peer.ip());
        if let Some(tenant_id) = &tenant {
            limiter.fail_tenant(tenant_id);
        }
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "访问密钥无效"));
    };
    if let Some(tenant_id) = &tenant {
        limiter.success_tenant(tenant_id);
    }
    Ok(ok(serde_json::json!({
        "token": token,
        "access": grant.access.as_str(),
        "device_id": grant.device_id,
    })))
}

/// `POST /api/v1/viewer/logout`：注销当前令牌
async fn logout(State(manager): State<Arc<ViewerManager>>, auth: BrowserAuth) -> Json<Value> {
    manager.logout(&auth.token);
    crate::api::success()
}

/// `GET /api/v1/viewer/devices`：bt 绑定设备的单元素列表（bt 即定位，无池枚举面）
async fn devices(
    State(manager): State<Arc<ViewerManager>>,
    auth: BrowserAuth,
) -> Result<Json<Value>, ApiError> {
    let state = manager.app_state();
    let now = crate::state::now_ms();
    let presence = state.presence.read().unwrap();
    let views: Vec<Value> = state
        .device(&auth.device_id)
        .map(|d| {
            vec![crate::api::devices::device_view_pub(
                &d,
                presence.get(&d.device_id),
                now,
            )]
        })
        .unwrap_or_default();
    Ok(ok(serde_json::json!({ "devices": views })))
}

#[derive(Deserialize)]
struct OpenSessionBody {
    target_device_id: String,
    /// read（默认）/ write（需要写令牌 + 目标设备 allow_write）
    #[serde(default = "default_access")]
    access: String,
}

fn default_access() -> String {
    "read".to_string()
}

/// `POST /api/v1/viewer/session`：打开（或复用）到目标设备的查看会话
async fn open_session(
    State(manager): State<Arc<ViewerManager>>,
    auth: BrowserAuth,
    JsonBody(body): JsonBody<OpenSessionBody>,
) -> Result<Json<Value>, ApiError> {
    let access = match body.access.as_str() {
        "read" => Access::Read,
        "write" => Access::Write,
        _ => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "access 须为 read 或 write",
            ))
        }
    };
    if access == Access::Write {
        if auth.access != Access::Write {
            return Err(ApiError::new(StatusCode::FORBIDDEN, "当前令牌没有写入权限"));
        }
        let Some(target) = manager.app_state().device(&body.target_device_id) else {
            return Err(ApiError::not_found("目标设备不存在"));
        };
        if !target.allow_write {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "目标设备未开启广域网写入",
            ));
        }
    }
    let view = manager
        .open_session(&auth.token, &body.target_device_id, access)
        .await
        .map_err(|message| ApiError::new(StatusCode::BAD_GATEWAY, message))?;
    Ok(ok(view))
}

/// `GET /api/v1/viewer/session`：当前绑定（无则 data 为 null）
async fn current_session(
    State(manager): State<Arc<ViewerManager>>,
    auth: BrowserAuth,
) -> Json<Value> {
    crate::api::ok(manager.current(&auth.token))
}

/// `DELETE /api/v1/viewer/session`：断开当前查看会话（幂等）
async fn disconnect(State(manager): State<Arc<ViewerManager>>, auth: BrowserAuth) -> Json<Value> {
    manager.disconnect(&auth.token).await;
    crate::api::success()
}

//! 网页版查看面 API（浏览器令牌 bt 鉴权；与 access_key 控制面分离）。

use super::ViewerManager;
use crate::api::{ok, ApiError, JsonBody};
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

pub fn routes() -> Router<Arc<ViewerManager>> {
    Router::new()
        .route("/api/v1/viewer/login", post(login))
        .route("/api/v1/viewer/logout", post(logout))
        .route("/api/v1/viewer/devices", get(devices))
        .route(
            "/api/v1/viewer/session",
            post(open_session).get(current_session).delete(disconnect),
        )
}

/// 浏览器令牌鉴权提取器：Bearer 头或 ?token= 查询参数（SPA 的 img/SSE 场景）
pub struct BrowserAuth(pub String);

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
        if manager.validate(&token).is_none() {
            return Err(ApiError::new(StatusCode::UNAUTHORIZED, "令牌无效或已注销"));
        }
        Ok(BrowserAuth(token))
    }
}

#[derive(Deserialize)]
struct LoginBody {
    access_key: String,
}

/// `POST /api/v1/viewer/login`：access_key 换发浏览器令牌
async fn login(
    State(manager): State<Arc<ViewerManager>>,
    JsonBody(body): JsonBody<LoginBody>,
) -> Result<Json<Value>, ApiError> {
    let Some(token) = manager.login(&body.access_key) else {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "访问密钥无效"));
    };
    Ok(ok(serde_json::json!({ "token": token })))
}

/// `POST /api/v1/viewer/logout`：注销当前令牌
async fn logout(State(manager): State<Arc<ViewerManager>>, auth: BrowserAuth) -> Json<Value> {
    manager.logout(&auth.0);
    crate::api::success()
}

/// `GET /api/v1/viewer/devices`：设备列表（同控制面结构）
async fn devices(
    State(manager): State<Arc<ViewerManager>>,
    _auth: BrowserAuth,
) -> Result<Json<Value>, ApiError> {
    let state = manager.app_state();
    let now = crate::state::now_ms();
    let device_list = state.store.lock().unwrap().list();
    let presence = state.presence.read().unwrap();
    let views: Vec<Value> = device_list
        .iter()
        .map(|d| crate::api::devices::device_view_pub(d, presence.get(&d.device_id), now))
        .collect();
    Ok(ok(serde_json::json!({ "devices": views })))
}

#[derive(Deserialize)]
struct OpenSessionBody {
    target_device_id: String,
}

/// `POST /api/v1/viewer/session`：打开（或复用）到目标设备的查看会话
async fn open_session(
    State(manager): State<Arc<ViewerManager>>,
    auth: BrowserAuth,
    JsonBody(body): JsonBody<OpenSessionBody>,
) -> Result<Json<Value>, ApiError> {
    let view = manager
        .open_session(&auth.0, &body.target_device_id)
        .await
        .map_err(|message| ApiError::new(StatusCode::BAD_GATEWAY, message))?;
    Ok(ok(view))
}

/// `GET /api/v1/viewer/session`：当前绑定（无则 data 为 null）
async fn current_session(
    State(manager): State<Arc<ViewerManager>>,
    auth: BrowserAuth,
) -> Json<Value> {
    crate::api::ok(manager.current(&auth.0))
}

/// `DELETE /api/v1/viewer/session`：断开当前查看会话（幂等）
async fn disconnect(State(manager): State<Arc<ViewerManager>>, auth: BrowserAuth) -> Json<Value> {
    manager.disconnect(&auth.0).await;
    crate::api::success()
}

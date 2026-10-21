//! 网页版查看面 API（浏览器令牌 bt 鉴权；与 access_key 控制面分离）。

use super::ViewerManager;
use crate::api::{ok, ApiError, JsonBody};
use crate::state::Access;
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
pub struct BrowserAuth {
    pub token: String,
    pub access: Access,
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
        let Some(access) = manager.validate(&token) else {
            return Err(ApiError::new(StatusCode::UNAUTHORIZED, "令牌无效或已注销"));
        };
        Ok(BrowserAuth { token, access })
    }
}

#[derive(Deserialize)]
struct LoginBody {
    /// access_key（只读）或 write_key（写作用域）
    key: String,
}

/// `POST /api/v1/viewer/login`：访问密钥/写入密钥换发浏览器令牌
async fn login(
    State(manager): State<Arc<ViewerManager>>,
    JsonBody(body): JsonBody<LoginBody>,
) -> Result<Json<Value>, ApiError> {
    let Some((token, access)) = manager.login(&body.key) else {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "密钥无效"));
    };
    Ok(ok(
        serde_json::json!({ "token": token, "access": access.as_str() }),
    ))
}

/// `POST /api/v1/viewer/logout`：注销当前令牌
async fn logout(State(manager): State<Arc<ViewerManager>>, auth: BrowserAuth) -> Json<Value> {
    manager.logout(&auth.token);
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

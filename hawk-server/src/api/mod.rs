//! REST/WSS 路由与响应信封。协议约定：错误信封不带业务错误码，语义由 HTTP 状态码承载，
//! message 供客户端直接展示（见 docs/backend/remote-protocol.md）。

pub mod devices;
pub mod relay;
pub mod sessions;
pub mod ws;

use crate::state::AppState;
use crate::viewer::ViewerManager;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> ApiError {
        ApiError {
            status,
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::NOT_FOUND, message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({ "status": "error", "error": { "message": self.message } });
        (self.status, Json(body)).into_response()
    }
}

/// 成功信封 `{ "status": "success", "data": ... }`
pub fn ok<T: serde::Serialize>(data: T) -> Json<Value> {
    Json(json!({ "status": "success", "data": data }))
}

/// 无 data 的成功响应 `{ "status": "success" }`
pub fn success() -> Json<Value> {
    Json(json!({ "status": "success" }))
}

/// JSON 请求体抽取：解析失败统一 400
pub struct JsonBody<T>(pub T);

impl<T, S> axum::extract::FromRequest<S> for JsonBody<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: axum::extract::Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(JsonBody(value)),
            Err(rejection) => Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("请求体解析失败: {rejection}"),
            )),
        }
    }
}

pub fn router(state: Arc<AppState>, viewer: Arc<ViewerManager>) -> Router {
    let control = Router::new()
        .route(
            "/api/v1/devices",
            post(devices::register).get(devices::list),
        )
        .route("/api/v1/devices/heartbeat", post(devices::heartbeat))
        .route(
            "/api/v1/devices/{device_id}",
            axum::routing::patch(devices::update).delete(devices::remove),
        )
        .route("/api/v1/sessions", post(sessions::create))
        .route("/api/v1/sessions/{session_id}", get(sessions::get))
        .route(
            "/api/v1/sessions/{session_id}/accept",
            post(sessions::accept),
        )
        .route(
            "/api/v1/sessions/{session_id}/reject",
            post(sessions::reject),
        )
        .route(
            "/api/v1/sessions/{session_id}/cancel",
            post(sessions::cancel),
        )
        .route("/api/v1/ws", get(ws::control_channel))
        .route("/api/v1/relay", get(relay::relay_channel))
        .with_state(state);

    Router::new()
        .route("/", get(crate::viewer::page::picker))
        .merge(control)
        .merge(crate::viewer::api::routes().with_state(viewer.clone()))
        // 回退：其余路径（查看面 API、/app 与 /assets 等查看器静态资源）全部经隧道转发给目标 daemon。
        // 网页版前端产物由 daemon 侧托管（daemon 启动时带 --web-dist），服务端不再需要单独部署一份
        .merge(
            Router::new()
                .fallback(crate::viewer::proxy::fallback)
                .with_state(viewer),
        )
}

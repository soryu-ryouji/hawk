//! app 端点：health / startup / status / info / token。
//! App 端点：启动握手、积压快照、token 发现的 Host 环回约束。

use crate::api::envelope::{ApiError, Envelope};
use crate::api::{AccessLevel, SharedState};
use axum::extract::{Extension, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

pub fn routes() -> Router<SharedState> {
    Router::new()
        .route("/api/v1/app/startup", get(startup))
        .route("/api/v1/app/status", get(status))
        .route("/api/v1/app/info", get(info))
        .route("/api/v1/app/token", get(token))
}

/// 就绪探活：无需 token。初始索引完成前返回 503
pub async fn health(State(state): State<SharedState>) -> axum::response::Response {
    if state.startup.is_ready() {
        axum::response::IntoResponse::into_response("ok")
    } else {
        axum::response::IntoResponse::into_response(axum::http::StatusCode::SERVICE_UNAVAILABLE)
    }
}

#[derive(Serialize)]
struct StartupInfo {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    processed: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

/// 启动状态：ready / starting（带进度）/ error（初始索引失败，message 为原因）
async fn startup(State(state): State<SharedState>) -> Json<Envelope<StartupInfo>> {
    let (is_ready, error, info) = state.startup.snapshot();
    let body = if let Some(message) = error {
        StartupInfo {
            status: "error",
            phase: None,
            processed: None,
            total: None,
            message: Some(message),
        }
    } else if is_ready {
        StartupInfo {
            status: "ready",
            phase: None,
            processed: None,
            total: None,
            message: None,
        }
    } else {
        StartupInfo {
            status: "starting",
            phase: Some(info.phase),
            processed: Some(info.processed),
            total: Some(info.total),
            message: None,
        }
    };
    Json(Envelope::ok(body))
}

#[derive(Serialize)]
struct TaskBacklog {
    pending: i32,
    active: i32,
}

#[derive(Serialize)]
struct IndexBacklog {
    pending: i32,
    active: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    processed: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total: Option<i32>,
}

#[derive(Serialize)]
struct TaskStatus {
    thumbnail: TaskBacklog,
    index: IndexBacklog,
}

/// 后台任务积压:轮询型客户端用(SSE 客户端订阅 task.progress 事件,两者同一份快照)
async fn status(State(state): State<SharedState>) -> Json<Envelope<TaskStatus>> {
    let (thumb_pending, thumb_active) = state.worker.backlog();
    let index = state.pipeline.index_progress();
    Json(Envelope::ok(TaskStatus {
        thumbnail: TaskBacklog {
            pending: thumb_pending,
            active: thumb_active,
        },
        index: IndexBacklog {
            pending: index.pending,
            active: index.active,
            phase: index.phase,
            processed: index.processed,
            total: index.total,
        },
    }))
}

#[derive(Serialize)]
struct AppInfo {
    version: &'static str,
    platform: &'static str,
    exec_path: String,
    access: &'static str,
}

/// 运行信息；access 级别由鉴权中间件写入请求扩展
async fn info(
    State(_state): State<SharedState>,
    Extension(access): Extension<AccessLevel>,
) -> Json<Envelope<AppInfo>> {
    let access = match access {
        AccessLevel::Admin => "admin",
        AccessLevel::Viewer => "viewer",
    };
    let platform = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let exec_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    Json(Envelope::ok(AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        platform,
        exec_path,
        access,
    }))
}

/// Token 发现：浏览器插件零配置接入。
/// 安全性依赖两点：响应不带 CORS 头（cors 中间件为该端点例外）；
/// Host 限定环回地址（防 DNS rebinding 伪装同源读取）
async fn token(
    State(state): State<SharedState>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, ApiError> {
    // HTTP/1.1 请求行是 origin-form（无 authority），Host 以 Host 头为准
    let host = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .trim_matches(['[', ']'])
        .to_string();
    let loopback = matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1" | "[::1]");
    if !loopback {
        return Err(ApiError::new(
            "INVALID_HOST",
            axum::http::StatusCode::BAD_REQUEST,
            "token discovery requires loopback host",
        ));
    }
    Ok(Json(serde_json::json!({
        "status": "success",
        "data": state.settings.token
    })))
}

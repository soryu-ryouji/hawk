//! app 端点：health / startup / status / info / token。
//! App 端点：启动握手、积压快照、token 发现的 Host 环回约束。

use crate::api::envelope::{ApiError, Envelope};
use crate::api::{AccessLevel, SharedState};
use axum::extract::{Extension, State};
use axum::Json;
use serde::Serialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn routes() -> OpenApiRouter<SharedState> {
    OpenApiRouter::new()
        .routes(routes!(health))
        .routes(routes!(startup))
        .routes(routes!(status))
        .routes(routes!(info))
        .routes(routes!(token))
}

/// 就绪探活：无需 token。初始索引完成前返回 503
#[utoipa::path(
    get,
    path = "/health",
    tags = ["app"],
    responses(
        (status = 200, description = "就绪：纯文本 ok", content_type = "text/plain", body = String),
        (status = 503, description = "初始索引构建中")
    )
)]
pub async fn health(State(state): State<SharedState>) -> axum::response::Response {
    if state.startup.is_ready() {
        axum::response::IntoResponse::into_response("ok")
    } else {
        axum::response::IntoResponse::into_response(axum::http::StatusCode::SERVICE_UNAVAILABLE)
    }
}

#[derive(Serialize, utoipa::ToSchema)]
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
#[utoipa::path(
    get,
    path = "/api/v1/app/startup",
    tags = ["app"],
    responses((status = 200, description = "OK", body = Envelope<StartupInfo>))
)]
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

#[derive(Serialize, utoipa::ToSchema)]
struct TaskBacklog {
    pending: i32,
    active: i32,
}

#[derive(Serialize, utoipa::ToSchema)]
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

#[derive(Serialize, utoipa::ToSchema)]
struct TaskStatus {
    thumbnail: TaskBacklog,
    index: IndexBacklog,
}

/// 后台任务积压：轮询型客户端用（SSE 客户端订阅 task.progress 事件，两者同一份快照）
#[utoipa::path(
    get,
    path = "/api/v1/app/status",
    tags = ["app"],
    responses((status = 200, description = "OK", body = Envelope<TaskStatus>))
)]
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

#[derive(Serialize, utoipa::ToSchema)]
struct LanInfo {
    active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct AppInfo {
    version: &'static str,
    platform: &'static str,
    exec_path: String,
    access: &'static str,
    /// 当前 token 是否可执行写操作：admin 恒 true；viewer 为 [web].writable
    /// （前端 viewerMode 依据，开启后 web 端展示全部写入口）
    writable: bool,
    lan: LanInfo,
}

/// 运行信息；access 级别由鉴权中间件写入请求扩展。lan 为局域网监听实况
/// （设置面板保存后轮询至此确认收敛/失败，热重绑无需重启 daemon）
#[utoipa::path(
    get,
    path = "/api/v1/app/info",
    tags = ["app"],
    responses((status = 200, description = "OK", body = Envelope<AppInfo>))
)]
async fn info(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessLevel>,
) -> Json<Envelope<AppInfo>> {
    let (access, writable) = match access {
        AccessLevel::Admin => ("admin", true),
        AccessLevel::Viewer { writable } => ("viewer", writable),
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
    let lan = state.lan.snapshot();
    Json(Envelope::ok(AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        platform,
        exec_path,
        access,
        writable,
        lan: LanInfo {
            active: lan.active,
            port: lan.port,
            error: lan.error,
        },
    }))
}

/// Token 发现：浏览器插件零配置接入。
/// 安全性依赖两点：响应不带 CORS 头（cors 中间件为该端点例外）；
/// Host 限定环回地址（防 DNS rebinding 伪装同源读取）。
/// 注意：远程访问隧道转发的请求 Host 同样是环回（B 侧代理地址），此检查对隧道无效——
/// remote 模块的隧道端必须拒绝转发本端点并改写 Host（见 docs/backend/remote-protocol.md 数据面）
#[utoipa::path(
    get,
    path = "/api/v1/app/token",
    tags = ["app"],
    responses((status = 200, description = "OK", body = TokenResponse))
)]
async fn token(
    State(state): State<SharedState>,
    req: axum::extract::Request,
) -> Result<Json<TokenResponse>, ApiError> {
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
    Ok(Json(TokenResponse {
        status: "success",
        data: state.settings.token.clone(),
    }))
}

/// token 发现端点响应（信封的 data 直接为 token 字符串）
#[derive(Serialize, utoipa::ToSchema)]
struct TokenResponse {
    status: &'static str,
    data: String,
}

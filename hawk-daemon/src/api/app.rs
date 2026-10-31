//! app 端点：health / startup / status / info / admin-token。
//! App 端点：启动握手、积压快照、admin token 发现的 Host 环回约束。

use crate::api::envelope::{ApiError, Envelope};
use crate::api::{require_manage, AccessContext, SharedState};
use crate::core::access::Role;
use axum::extract::{Extension, State};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn routes() -> OpenApiRouter<SharedState> {
    OpenApiRouter::new()
        .routes(routes!(health))
        .routes(routes!(startup))
        .routes(routes!(status))
        .routes(routes!(info))
        .routes(routes!(connection))
        .routes(routes!(create_app_session))
        .routes(routes!(admin_token))
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

/// 最近一轮全库扫描统计
#[derive(Serialize, utoipa::ToSchema)]
struct ScanStatsInfo {
    /// 扫描开始时间（Unix 毫秒）
    started_unix_ms: i64,
    duration_ms: i64,
    /// 本轮枚举到的文件数
    files: i32,
    /// 快照对比判定为 dirty、深入枚举的目录数
    dirty_dirs: i32,
    /// 本轮实际应用入库的位置数
    applied: i32,
}

#[derive(Serialize, utoipa::ToSchema)]
struct TaskStatus {
    thumbnail: TaskBacklog,
    index: IndexBacklog,
    /// 最近一轮全库扫描统计（未跑过为 null）
    #[serde(skip_serializing_if = "Option::is_none")]
    last_scan: Option<ScanStatsInfo>,
    /// 索引队列曾溢出（已触发兜底扫描）；下一轮扫描收尾后复位
    queue_overflow: bool,
    /// .hawk/config.toml 解析错误（保留上次有效配置继续运行）；正常为 null
    #[serde(skip_serializing_if = "Option::is_none")]
    config_error: Option<String>,
    /// SSE 订阅因消费落后被断开（lagged）的累计次数
    sse_lagged: u64,
}

/// 后台任务积压：轮询型客户端用（SSE 客户端订阅 task.progress 事件，两者同一份快照）
#[utoipa::path(
    get,
    path = "/api/v1/app/status",
    tags = ["app"],
    responses((status = 200, description = "OK", body = Envelope<TaskStatus>))
)]
async fn status(State(state): State<SharedState>) -> Result<Json<Envelope<TaskStatus>>, ApiError> {
    let lib = state.require_library()?;
    let (thumb_pending, thumb_active) = lib.worker.backlog();
    let index = lib.pipeline.index_progress();
    let last_scan = lib.pipeline.scan_stats().map(|s| ScanStatsInfo {
        started_unix_ms: s.started_unix_ms,
        duration_ms: s.duration_ms,
        files: s.files,
        dirty_dirs: s.dirty_dirs,
        applied: s.applied,
    });
    Ok(Json(Envelope::ok(TaskStatus {
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
        last_scan,
        queue_overflow: lib.pipeline.queue_overflowed(),
        config_error: lib.config.config_error(),
        sse_lagged: state.sse_lagged.load(std::sync::atomic::Ordering::Relaxed),
    })))
}

#[derive(Serialize, utoipa::ToSchema)]
struct LanInfo {
    /// 网站端口监听状态
    active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// hawk-app 直连端口监听状态
    peer_active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    peer_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    peer_error: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct AppInfo {
    version: &'static str,
    platform: &'static str,
    exec_path: String,
    /// 当前凭证的角色（viewer/importer/editor/admin）
    role: &'static str,
    /// 当前请求进入 daemon 的通道（loopback/lan_web/lan_peer/wan）
    channel: &'static str,
    /// 本地素材库是否就位（false = 远程查看模式：库端点不可用，仅远程管理面/会话代理）
    has_library: bool,
    lan: LanInfo,
}

/// 运行信息；role/channel 由鉴权中间件写入请求扩展。lan 为局域网监听实况
/// （设置面板保存后轮询至此确认收敛/失败，热重绑无需重启 daemon）
#[utoipa::path(
    get,
    path = "/api/v1/app/info",
    tags = ["app"],
    responses((status = 200, description = "OK", body = Envelope<AppInfo>))
)]
async fn info(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessContext>,
) -> Json<Envelope<AppInfo>> {
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
        role: access.role.as_str(),
        channel: access.channel.as_str(),
        has_library: state.library.is_some(),
        lan: LanInfo {
            active: lan.web.active,
            port: lan.web.port,
            error: lan.web.error,
            peer_active: lan.peer.active,
            peer_port: lan.peer.port,
            peer_error: lan.peer.error,
        },
    }))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct AppSessionBody {
    access_key: String,
}

#[derive(Serialize, utoipa::ToSchema)]
struct AppSessionDto {
    role: Role,
    channel: &'static str,
    expires_in: u64,
}

/// LAN Web：AccessKey 换有期 Session，并以 HttpOnly/Secure Cookie 下发。
#[utoipa::path(
    post,
    path = "/api/v1/app/session",
    tags = ["app"],
    request_body = AppSessionBody,
    responses((status = 200, description = "OK", body = Envelope<AppSessionDto>))
)]
async fn create_app_session(
    State(state): State<SharedState>,
    Extension(mode): Extension<super::ListenerMode>,
    Json(body): Json<AppSessionBody>,
) -> Result<axum::response::Response, ApiError> {
    if mode != super::ListenerMode::Web {
        return Err(ApiError::new(
            "INVALID_CHANNEL",
            axum::http::StatusCode::NOT_FOUND,
            "App Session 仅用于 LAN Web",
        ));
    }
    let lib = state.require_library()?;
    let library_id = crate::core::paths::library_key(&lib.paths.root);
    let credential = crate::remote::keys::sha256_hex(&body.access_key);
    let Some(entry) = state
        .remote
        .keys
        .resolve_for_library(Some(&library_id), &credential)
    else {
        return Err(ApiError::new(
            "UNAUTHORIZED",
            axum::http::StatusCode::UNAUTHORIZED,
            "AccessKey 无效",
        ));
    };
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| {
        ApiError::new(
            "RANDOM_FAILED",
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Session 生成失败: {e}"),
        )
    })?;
    let token = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let expires = std::time::Instant::now() + std::time::Duration::from_secs(30 * 24 * 60 * 60);
    let mut sessions = state.web_sessions.lock().unwrap();
    sessions.retain(|_, session| session.4 > std::time::Instant::now());
    if sessions.len() >= 64 {
        sessions.clear();
    }
    sessions.insert(
        token.clone(),
        (library_id, entry.id, credential, entry.role, expires),
    );
    let mut response = Json(Envelope::ok(AppSessionDto {
        role: entry.role,
        channel: "lan_web",
        expires_in: 30 * 24 * 60 * 60,
    }))
    .into_response();
    response.headers_mut().insert(
        axum::http::header::SET_COOKIE,
        format!(
            "hawk_session={token}; Path=/; HttpOnly; SameSite=Strict; Secure; Max-Age={}",
            30 * 24 * 60 * 60
        )
        .parse()
        .unwrap(),
    );
    Ok(response)
}

/// LocalIntegration 发现：浏览器插件零配置接入。
/// 返回 Loopback 限定的 importer Session，而不是进程 admin token。
/// 响应不带 CORS 头且 Host 限定环回；远程隧道端必须直接拒绝本端点。
#[utoipa::path(
    get,
    path = "/api/v1/app/admin-token",
    tags = ["app"],
    responses((status = 200, description = "OK", body = AdminTokenResponse))
)]
async fn admin_token(
    State(state): State<SharedState>,
    req: axum::extract::Request,
) -> Result<Json<AdminTokenResponse>, ApiError> {
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
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| {
        ApiError::new(
            "RANDOM_FAILED",
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("集成本地会话生成失败: {e}"),
        )
    })?;
    let token = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let mut sessions = state.integration_sessions.lock().unwrap();
    let now = std::time::Instant::now();
    sessions.retain(|_, expires| *expires > now);
    if sessions.len() >= 32 {
        sessions.clear();
    }
    sessions.insert(
        token.clone(),
        now + std::time::Duration::from_secs(24 * 60 * 60),
    );
    drop(sessions);
    Ok(Json(AdminTokenResponse {
        status: "success",
        data: token,
    }))
}

/// LocalIntegration Session 发现响应（data 为短期 importer token）
#[derive(Serialize, utoipa::ToSchema)]
struct AdminTokenResponse {
    status: &'static str,
    data: String,
}

/// 当前连接描述符：给本机生态客户端（浏览器插件）解析「本地 hawk 现在在看哪个库」。
/// 本地库 → 调用方继续用自己的 admin 地址；远端库 → 会话代理环回地址 + 会话令牌。
#[derive(Serialize, utoipa::ToSchema)]
struct AppConnectionDto {
    /// local（本地素材库）/ remote（远端素材库，经会话代理）/ none（未连接任何素材库）
    mode: &'static str,
    /// 仅 remote 有值：会话代理地址（http://127.0.0.1:<port>）
    #[serde(skip_serializing_if = "Option::is_none")]
    api_base: Option<String>,
    /// 仅 remote 有值：会话临时凭证
    #[serde(skip_serializing_if = "Option::is_none")]
    credential: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    /// 当前连接角色（本地库 admin；远端库为会话角色）
    role: Role,
}

/// 当前连接（远程查看模式下同样可用：无库 daemon 只暴露远程管理面与会话代理）
#[utoipa::path(
    get,
    path = "/api/v1/app/connection",
    tags = ["app"],
    responses((status = 200, description = "OK", body = Envelope<AppConnectionDto>))
)]
async fn connection(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessContext>,
) -> Result<Json<Envelope<AppConnectionDto>>, ApiError> {
    if !matches!(
        &access.principal,
        crate::api::Principal::LocalIntegration { .. }
    ) {
        require_manage(&access)?;
    }
    if let Some(session) = state.remote.sessions.snapshot() {
        let label = if session.target_library_name.is_empty() {
            session.target_name.clone()
        } else {
            format!("{} · {}", session.target_name, session.target_library_name)
        };
        return Ok(Json(Envelope::ok(AppConnectionDto {
            mode: "remote",
            api_base: Some(format!("http://127.0.0.1:{}", session.proxy_port)),
            credential: Some(session.session_token),
            device_id: Some(session.target_device_id),
            label: Some(label),
            role: session.role,
        })));
    }
    let has_library = state.library.is_some();
    Ok(Json(Envelope::ok(AppConnectionDto {
        mode: if has_library { "local" } else { "none" },
        api_base: None,
        credential: None,
        device_id: None,
        label: None,
        role: if has_library {
            Role::Admin
        } else {
            Role::Viewer
        },
    })))
}

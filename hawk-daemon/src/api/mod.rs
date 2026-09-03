//! 共享应用状态与路由装配。Api/ → Core/ 单向依赖。

use crate::core::config::LibraryConfig;
use crate::core::events::EventBus;
use crate::core::index::ItemIndex;
use crate::core::paths::LibraryPaths;
use crate::core::pipeline::IndexPipeline;
use crate::settings::Settings;
use crate::core::startup::StartupState;
use crate::core::taxonomy::{CategoryRegistry, TagRegistry};
use crate::core::thumbnail::ThumbnailService;
use crate::core::thumbnail_worker::ThumbnailWorker;
use crate::core::view_prefs::ViewPreferences;
use axum::response::IntoResponse;
use std::sync::Arc;

pub mod app;
pub mod envelope;
pub mod events;
pub mod folder;
pub mod item;
pub mod lan;
pub mod library;
pub mod openapi;
pub mod taxonomy;
pub mod trash;
pub mod view;
pub mod web_dist;

/// 请求级扩展：当前 token 的访问级别，app/info 据此报告。
/// Viewer 携带该 token 的写能力（[web] 的 writable/separate/write_token 共同决定，
/// 每请求解析，配置热生效）
#[derive(Clone, Copy)]
pub enum AccessLevel {
    Admin,
    Viewer { writable: bool },
}

pub struct AppState {
    pub settings: Settings,
    pub paths: LibraryPaths,
    pub config: Arc<LibraryConfig>,
    pub startup: Arc<StartupState>,
    pub index: Arc<ItemIndex>,
    pub bus: EventBus,
    pub pipeline: IndexPipeline,
    pub thumbs: ThumbnailService,
    pub prefs: Arc<ViewPreferences>,
    pub categories: Arc<CategoryRegistry>,
    pub tags: Arc<TagRegistry>,
    pub worker: Arc<ThumbnailWorker>,
    /// LAN 监听 supervisor（状态快照供 app/info；监听重绑由常驻任务自驱）
    pub lan: Arc<lan::LanSupervisor>,
}

pub type SharedState = Arc<AppState>;

/// 构建路由：中间件链 CORS → Auth → ReadyGate → Endpoints
pub fn build_router(state: SharedState) -> axum::Router {
    axum::Router::new()
        .route("/health", axum::routing::get(app::health))
        .route("/openapi/v1.json", axum::routing::get(openapi::openapi_schema))
        .merge(app::routes())
        .merge(lan::routes())
        .merge(library::routes())
        .merge(folder::routes())
        .merge(item::routes())
        .merge(taxonomy::routes())
        .merge(view::routes())
        .merge(trash::routes())
        .merge(events::routes())
        .fallback(web_dist::serve)
        .with_state(state.clone())
        // 请求体上限放宽到 256MB（axum 默认 2MB）：item/upload 整文件上传与 item/replace 的
        // base64 内容替换都会超过默认值；端点全部 token 鉴权，局域网/本机场景风险可控
        .layer(axum::extract::DefaultBodyLimit::max(256 * 1024 * 1024))
        // axum 中后注册的 layer 在外层：请求依次经过 cors → auth → ready_gate
        .layer(axum::middleware::from_fn_with_state(state.clone(), ready_gate))
        .layer(axum::middleware::from_fn_with_state(state.clone(), auth))
        .layer(axum::middleware::from_fn(cors))
}

/// CORS 全放开（localhost 工具，token 兜底）；唯一例外：token 发现端点不带 CORS 头
/// （跨源网页 JS 读不到响应，只有持 host_permissions 的扩展能读）
async fn cors(req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    use axum::http::header;
    let is_token_discovery = req.uri().path() == "/api/v1/app/token";
    let mut resp = next.run(req).await;
    if !is_token_discovery {
        let headers = resp.headers_mut();
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*".parse().unwrap());
        headers.insert(header::ACCESS_CONTROL_ALLOW_METHODS, "*".parse().unwrap());
        headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, "*".parse().unwrap());
    }
    resp
}

/// Token 鉴权：/api/* 请求必须携带 Authorization: Bearer <token>；
/// SSE 端点（/api/v1/events）无法设置请求头，改用查询参数 ?token=。
/// 双 token：admin（Electron 启动时生成，全权限）与 viewer（config.toml [web].token，只读）。
/// 例外：GET /api/v1/app/token（token 发现端点）无鉴权，安全性见端点实现。
async fn auth(
    axum::extract::State(state): axum::extract::State<SharedState>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let path = req.uri().path();
    if !path.starts_with("/api") {
        return next.run(req).await;
    }
    if path == "/api/v1/app/token" && req.method() == axum::http::Method::GET {
        return next.run(req).await;
    }

    let access = resolve_access(&state, &req);
    let Some(access) = access else {
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            axum::Json(
                serde_json::json!({ "status": "error", "error": { "code": envelope::codes::UNAUTHORIZED, "message": "missing or invalid token" } }),
            ),
        )
            .into_response();
    };

    if matches!(access, AccessLevel::Viewer { writable: false }) && !is_viewer_allowed(&req) {
        return (
            axum::http::StatusCode::FORBIDDEN,
            axum::Json(
                serde_json::json!({ "status": "error", "error": { "code": envelope::codes::READ_ONLY, "message": "viewer token is read-only" } }),
            ),
        )
            .into_response();
    }

    req.extensions_mut().insert(access);
    next.run(req).await
}

/// 返回 access 级别（含 viewer 的 per-token 写能力），token 无效返回 None
fn resolve_access(state: &AppState, req: &axum::extract::Request) -> Option<AccessLevel> {
    let headers = req.headers();
    if let Some(auth) = headers.get(axum::http::header::AUTHORIZATION) {
        if let Ok(value) = auth.to_str() {
            if let Some(token) = value.strip_prefix("Bearer ") {
                if token == state.settings.token {
                    return Some(AccessLevel::Admin);
                }
                if let Some(access) = viewer_access(state, token) {
                    return Some(access);
                }
            }
        }
    }

    // EventSource 与 <img> 均无法设置请求头，这几个 GET 端点放行查询参数 token
    let path = req.uri().path();
    let allow_query_token = req.method() == axum::http::Method::GET
        && matches!(
            path,
            "/api/v1/events" | "/api/v1/item/thumbnail" | "/api/v1/item/file"
        );
    if allow_query_token {
        if let Some(query) = req.uri().query() {
            let token: Option<&str> = query
                .split('&')
                .filter_map(|pair| pair.split_once('='))
                .find(|(k, _)| *k == "token")
                .map(|(_, v)| v);
            if let Some(token) = token {
                if token == state.settings.token {
                    return Some(AccessLevel::Admin);
                }
                if let Some(access) = viewer_access(state, token) {
                    return Some(access);
                }
            }
        }
    }
    None
}

/// 局域网 viewer token 的访问能力（每请求解析，配置变更热生效）：
/// - token：未拆分时随 writable，拆分（separate_write_token）时恒只读
/// - write_token：仅在启用写 + 拆分时有效，恒可写
fn viewer_access(state: &AppState, token: &str) -> Option<AccessLevel> {
    let web = &state.config.current().web;
    if !web.enabled {
        return None;
    }
    if web.token.as_deref() == Some(token) {
        return Some(AccessLevel::Viewer {
            writable: web.writable && !web.separate_write_token,
        });
    }
    if web.separate_write_token && web.writable && web.write_token.as_deref() == Some(token) {
        return Some(AccessLevel::Viewer { writable: true });
    }
    None
}

/// viewer（局域网 web 查看）默认只读：仅放行 GET 与查询类 POST；
/// 可写 token（[web] writable，拆分时为 write_token）解除限制（每请求经 current() 校验，保存即热生效）
fn is_viewer_allowed(req: &axum::extract::Request) -> bool {
    if req.method() == axum::http::Method::GET {
        return true;
    }
    // 查询类 POST（复杂过滤结构），语义只读
    matches!(
        req.uri().path(),
        "/api/v1/item/list" | "/api/v1/item/skeleton"
    )
}

/// 启动网关：初始索引完成前拒绝一切 /api/* 请求（503 NOT_READY），仅放行 /api/v1/app/startup。
/// /health 与 /openapi 不在 /api 前缀下，由各自端点自行处理
async fn ready_gate(
    axum::extract::State(state): axum::extract::State<SharedState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let path = req.uri().path();
    if !state.startup.is_ready()
        && path.starts_with("/api/")
        && path != "/api/v1/app/startup"
    {
        return envelope::ApiError::new(
            envelope::codes::NOT_READY,
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "initial index is still building",
        )
        .into_response();
    }
    next.run(req).await
}

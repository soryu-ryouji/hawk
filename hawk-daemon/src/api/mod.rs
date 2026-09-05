//! 共享应用状态与路由装配。Api/ → Core/ 单向依赖。

use crate::core::config::LibraryConfig;
use crate::core::events::EventBus;
use crate::core::global_filter::GlobalFilter;
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
pub mod global_filter;
pub mod item;
pub mod lan;
pub mod library;
pub mod openapi;
pub mod taxonomy;
pub mod trash;
pub mod view;
pub mod web_dist;

#[cfg(test)]
mod contract_tests;


/// 重建 OpenAPI 文档并序列化为 pretty JSON（LF 行尾）——/openapi/v1.json 与 --dump-openapi 共用
pub fn build_openapi_json() -> String {
    let (_router, mut doc) = api_router();
    openapi::attach_extra_schemas(&mut doc);
    doc.info.title = "hawk-daemon | v1".to_string();
    doc.info.version = "1.0.0".to_string();
    doc.servers = Some(vec![utoipa::openapi::Server::new("http://127.0.0.1:27371/")]);
    let mut value = serde_json::to_value(&doc).expect("OpenAPI 文档序列化失败");
    strip_schema_defaults(&mut value);
    let mut json = serde_json::to_string_pretty(&value).expect("OpenAPI 文档序列化失败");
    json.push('\n');
    json
}

/// 剥离 schema 中的 default 关键字：utoipa 把 #[serde(default)] 输出为 default，
/// 而 openapi-typescript 遇到 default 会把属性生成成必填（响应视角），
/// 与请求体「可省略」的语义冲突。default 对契约校验只是注解，剥除无损
fn strip_schema_defaults(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.remove("default");
            for v in map.values_mut() {
                strip_schema_defaults(v);
            }
        }
        serde_json::Value::Array(items) => {
            for v in items {
                strip_schema_defaults(v);
            }
        }
        _ => {}
    }
}

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
    // ---- 存储底座：路径布局 / 库配置 / 启动进度 ----
    pub paths: LibraryPaths,
    pub config: Arc<LibraryConfig>,
    pub startup: Arc<StartupState>,
    /// 目录结构缓存（folder/list 的递归建树结果；失效与 folder.changed 同线）
    pub folder_tree: Arc<folder::FolderTreeCache>,
    // ---- 索引流水线（单写者）：内存索引 + 事件总线 + 消费循环 ----
    pub index: Arc<ItemIndex>,
    pub bus: EventBus,
    pub pipeline: IndexPipeline,
    // ---- 缩略图派生：服务 + 后台 worker（结果经队列回流流水线） ----
    pub thumbs: ThumbnailService,
    pub worker: Arc<ThumbnailWorker>,
    // ---- 分类与视图偏好 ----
    pub prefs: Arc<ViewPreferences>,
    pub categories: Arc<CategoryRegistry>,
    pub tags: Arc<TagRegistry>,
    /// 全局列表隐藏项注册表（.hawk/global_filter.toml）
    pub global_filter: Arc<GlobalFilter>,
    /// LAN 监听 supervisor（状态快照供 app/info；监听重绑由常驻任务自驱）
    pub lan: Arc<lan::LanSupervisor>,
}

pub type SharedState = Arc<AppState>;

/// API 路由与 OpenAPI 文档的同一来源：OpenApiRouter 收集 #[utoipa::path] 标注的端点，
/// split 出路由与文档（文档由 openapi.rs 固化服务于 /openapi/v1.json）
pub fn api_router() -> (axum::Router<SharedState>, utoipa::openapi::OpenApi) {
    utoipa_axum::router::OpenApiRouter::new()
        .merge(app::routes())
        .merge(lan::routes())
        .merge(library::routes())
        .merge(folder::routes())
        .merge(item::routes())
        .merge(taxonomy::routes())
        .merge(global_filter::routes())
        .merge(view::routes())
        .merge(trash::routes())
        .merge(events::routes())
        .split_for_parts()
}

/// 构建路由：中间件链 CORS → Auth → ReadyGate → Endpoints
pub fn build_router(state: SharedState) -> axum::Router {
    let (api_routes, _doc) = api_router();
    axum::Router::new()
        .route("/openapi/v1.json", axum::routing::get(openapi::openapi_schema))
        .merge(api_routes)
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
    // 预检短路：OPTIONS 不携带凭据、不执行任何操作，直接 204 + 放开头（跨源 dev 前端
    // （localhost:5173 → 127.0.0.1）带 Authorization 头的请求全靠预检放行；token 校验仍在真实请求上执行）
    if req.method() == axum::http::Method::OPTIONS && !is_token_discovery {
        let mut resp = axum::response::Response::new(axum::body::Body::empty());
        *resp.status_mut() = axum::http::StatusCode::NO_CONTENT;
        let headers = resp.headers_mut();
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*".parse().unwrap());
        headers.insert(header::ACCESS_CONTROL_ALLOW_METHODS, "*".parse().unwrap());
        headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, "*".parse().unwrap());
        return resp;
    }
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

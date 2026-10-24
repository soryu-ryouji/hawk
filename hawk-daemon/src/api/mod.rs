//! 共享应用状态与路由装配。Api/ → Core/ 单向依赖。

use crate::core::config::LibraryConfig;
use crate::core::events::EventBus;
use crate::core::global_filter::GlobalFilter;
use crate::core::index::ItemIndex;
use crate::core::locks::Locks;
use crate::core::metadata_store::MetadataStore;
use crate::core::paths::LibraryPaths;
use crate::core::pipeline::IndexPipeline;
use crate::core::startup::StartupState;
use crate::core::taxonomy::{CategoryRegistry, TagRegistry};
use crate::core::thumbnail::ThumbnailService;
use crate::core::thumbnail_worker::ThumbnailWorker;
use crate::core::view_prefs::ViewPreferences;
use crate::settings::Settings;
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
pub mod locks;
pub mod openapi;
pub mod remote;
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
    doc.servers = Some(vec![utoipa::openapi::Server::new(
        "http://127.0.0.1:27371/",
    )]);
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
/// Viewer 携带该访问密钥的写能力（[web].keys 每把密钥独立 read/write，
/// 每请求解析，配置热生效）；RemoteWriter 为广域网隧道的受限写（仅导入/建文件夹）。
#[derive(Clone, Copy)]
pub enum AccessLevel {
    Admin,
    Viewer { writable: bool },
    RemoteWriter,
}

/// LAN 监听器类型：网页端口与 hawk-app 直连端口各自只接受对应密钥。
/// Local = 本机 daemon API（以及测试/隧道复用），兼容两类局域网密钥。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListenerMode {
    Local,
    Web,
    Peer,
}

/// admin 限定端点的统一门禁：viewer（含可写）与远程写 403，防止只读 token 经管理端点提权
pub(crate) fn require_admin(access: &AccessLevel) -> Result<(), envelope::ApiError> {
    if matches!(access, AccessLevel::Admin) {
        Ok(())
    } else {
        Err(envelope::ApiError::new(
            envelope::codes::READ_ONLY,
            axum::http::StatusCode::FORBIDDEN,
            "viewer token cannot access admin settings",
        ))
    }
}

/// 本地素材库状态（仅本地库模式存在；远程查看模式无本地库时为 None）。
/// 库端点统一经 [AppState::require_library] 取用，无库即 400 NO_LIBRARY
pub struct LibraryState {
    // ---- 存储底座：路径布局 / 库配置 ----
    pub paths: LibraryPaths,
    pub config: Arc<LibraryConfig>,
    /// 目录结构缓存（folder/list 的递归建树结果；失效与 folder.changed 同线）
    pub folder_tree: Arc<folder::FolderTreeCache>,
    // ---- 索引流水线（单写者）：内存索引 + 消费循环 ----
    pub index: Arc<ItemIndex>,
    pub pipeline: IndexPipeline,
    /// 元数据存储（存储方案模式/迁移入口的读取面）
    pub store: Arc<MetadataStore>,
    // ---- 缩略图派生：服务 + 后台 worker（结果经队列回流流水线） ----
    pub thumbs: ThumbnailService,
    pub worker: Arc<ThumbnailWorker>,
    // ---- 分类与视图偏好 ----
    pub prefs: Arc<ViewPreferences>,
    pub categories: Arc<CategoryRegistry>,
    pub tags: Arc<TagRegistry>,
    /// 全局列表隐藏项注册表（.hawk/global_filter.toml）
    pub global_filter: Arc<GlobalFilter>,
    /// 文件夹/分类/标签锁（.hawk/locks.toml，服务端强制）
    pub locks: Arc<Locks>,
}

pub struct AppState {
    pub settings: Settings,
    /// 启动进度（无库模式无索引构建，监听就绪即 ready）
    pub startup: Arc<StartupState>,
    /// 事件总线（库事件与远程事件共用；远程 supervisor 依赖它，故不属于 LibraryState）
    pub bus: EventBus,
    /// SSE 订阅因消费落后被断开（lagged）的累计次数（app/status 观测）
    pub sse_lagged: std::sync::atomic::AtomicU64,
    /// LAN 监听 supervisor（状态快照供 app/info；监听重绑由常驻任务自驱；无库不启动）
    pub lan: Arc<lan::LanSupervisor>,
    /// 远程访问 supervisor（remote.toml 启用才激活；状态快照供 api/remote）
    pub remote: Arc<crate::remote::Supervisor>,
    /// 本地素材库状态；None = 远程查看模式（仅远程管理面与会话代理可用）
    pub library: Option<Arc<LibraryState>>,
}

impl AppState {
    /// 本地库状态（库端点统一入口）；远程查看模式返回 400 NO_LIBRARY
    pub fn require_library(&self) -> Result<&Arc<LibraryState>, envelope::ApiError> {
        self.library.as_ref().ok_or_else(|| {
            envelope::ApiError::new(
                envelope::codes::NO_LIBRARY,
                axum::http::StatusCode::BAD_REQUEST,
                "远程查看模式无本地素材库",
            )
        })
    }
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
        .merge(locks::routes())
        .merge(view::routes())
        .merge(trash::routes())
        .merge(events::routes())
        .merge(remote::routes())
        .split_for_parts()
}

/// 构建本机 daemon API 路由（兼容两类局域网密钥；隧道/测试复用同一装配）
pub fn build_router(state: SharedState) -> axum::Router {
    build_router_for(state, ListenerMode::Local)
}

/// 按监听器类型构建路由：网页端口托管静态页面，peer 端口只提供 API，不暴露网页。
pub fn build_router_for(state: SharedState, mode: ListenerMode) -> axum::Router {
    let (api_routes, _doc) = api_router();
    let router = axum::Router::new()
        .route(
            "/openapi/v1.json",
            axum::routing::get(openapi::openapi_schema),
        )
        .merge(api_routes);
    let router = match mode {
        ListenerMode::Web => router.fallback(web_dist::serve),
        ListenerMode::Peer => router.fallback(|| async { axum::http::StatusCode::NOT_FOUND }),
        ListenerMode::Local => router.fallback(web_dist::serve),
    };
    router
        .with_state(state.clone())
        // 请求体上限放宽到 256MB（axum 默认 2MB）：item/upload 整文件上传与 item/replace 的
        // base64 内容替换都会超过默认值；端点全部 token 鉴权，局域网/本机场景风险可控
        .layer(axum::extract::DefaultBodyLimit::max(256 * 1024 * 1024))
        // axum 中后注册的 layer 在外层：请求依次经过 cors → auth → ready_gate
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            ready_gate,
        ))
        .layer(axum::middleware::from_fn_with_state(state.clone(), auth))
        .layer(axum::middleware::from_fn(cors))
        // 最外层兜底：handler/中间件 panic 转 500，不拖垮进程（进程级 panic 隔离见 main 的 panic hook）
        .layer(tower_http::catch_panic::CatchPanicLayer::new())
        // 请求扩展供 auth / pair handler 判断当前端口类型
        .layer(axum::Extension(mode))
}

/// CORS 全放开（localhost 工具，token 兜底）；唯一例外：token 发现端点不带 CORS 头
/// （跨源网页 JS 读不到响应，只有持 host_permissions 的扩展能读）
async fn cors(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
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
    let mode = req
        .extensions()
        .get::<ListenerMode>()
        .copied()
        .unwrap_or(ListenerMode::Local);
    if path == "/api/v1/app/token" && req.method() == axum::http::Method::GET {
        return next.run(req).await;
    }
    // 局域网配对免 Bearer：
    // - 桌面自动配对：源端创建请求、随后用随机 request_id 轮询目标端决策；
    // - 旧配对码：移动端/二维码仍以一次性码作为准入凭证。
    // 这些 handler 内部都限定只在 hawk-app 直连端口（Peer）生效。
    let is_pair_code = path == "/api/v1/app/lan/pair" && req.method() == axum::http::Method::POST;
    let is_pair_request_create =
        path == "/api/v1/app/lan/pair-requests" && req.method() == axum::http::Method::POST;
    let is_pair_request_status = req.method() == axum::http::Method::GET
        && path
            .strip_prefix("/api/v1/app/lan/pair-requests/")
            .is_some_and(|id| !id.is_empty() && !id.contains('/'));
    if is_pair_code || is_pair_request_create || is_pair_request_status {
        return next.run(req).await;
    }

    // 进程内受托标记优先：remote 隧道端转发时已插入 TunneledPeer（HTTP 客户端无法伪造扩展，
    // 忽略其携带的任何 token 头）。写会话是否生效还要看当前 remote.writable（应用级配置，热生效）。
    let access = if let Some(tunnel) = req.extensions().get::<crate::remote::data::TunneledPeer>() {
        if tunnel.writable && state.remote.remote_config().current().writable {
            Some(AccessLevel::RemoteWriter)
        } else {
            Some(AccessLevel::Viewer { writable: false })
        }
    } else {
        resolve_access(&state, &req, mode)
    }; // 无库模式仅 admin token 有效（viewer token 存于库内配置）
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
    if matches!(access, AccessLevel::RemoteWriter) && !is_remote_writer_allowed(&req) {
        return (
            axum::http::StatusCode::FORBIDDEN,
            axum::Json(
                serde_json::json!({ "status": "error", "error": { "code": envelope::codes::READ_ONLY, "message": "remote writer is limited to import and folder create" } }),
            ),
        )
            .into_response();
    }

    req.extensions_mut().insert(access);
    // 解锁票据 → 请求级锁守卫（未解锁锁集合）：列表/内容端点经 Extension 消费。
    // 票据不是鉴权凭证（鉴权已过），仅扩展本请求的可见范围；无库模式无锁集，守卫恒空
    let tickets = parse_unlock_tickets(&req);
    let guard = match &state.library {
        Some(lib) => lib.locks.guard(&tickets),
        None => crate::core::locks::LockGuard::default(),
    };
    req.extensions_mut().insert(guard);
    next.run(req).await
}

/// 解锁票据采集：`X-Hawk-Unlock` 头（逗号分隔多票）+ GET 的 `?unlock=` 查询参数
/// （路径集合与 token 查询参数通道一致：img/SSE 无法设头）。票据为 hex，无需 URL 解码
fn parse_unlock_tickets(req: &axum::extract::Request) -> Vec<String> {
    let mut tickets = Vec::new();
    if let Some(hv) = req.headers().get("x-hawk-unlock") {
        if let Ok(v) = hv.to_str() {
            tickets.extend(
                v.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
            );
        }
    }
    let path = req.uri().path();
    let allow_query = matches!(
        path,
        "/api/v1/events" | "/api/v1/item/thumbnail" | "/api/v1/item/file"
    );
    if allow_query {
        if let Some(query) = req.uri().query() {
            for pair in query.split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    if k == "unlock" {
                        tickets.extend(
                            v.split(',')
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                                .map(str::to_string),
                        );
                    }
                }
            }
        }
    }
    tickets
}

/// 返回 access 级别（含 viewer 的 per-token 写能力），token 无效返回 None
fn resolve_access(
    state: &AppState,
    req: &axum::extract::Request,
    mode: ListenerMode,
) -> Option<AccessLevel> {
    let headers = req.headers();
    if let Some(auth) = headers.get(axum::http::header::AUTHORIZATION) {
        if let Ok(value) = auth.to_str() {
            if let Some(token) = value.strip_prefix("Bearer ") {
                if token == state.settings.token {
                    return Some(AccessLevel::Admin);
                }
                if let Some(access) = viewer_access(state, token, mode) {
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
                if let Some(access) = viewer_access(state, token, mode) {
                    return Some(access);
                }
            }
        }
    }
    None
}

/// 局域网 viewer token 的访问能力（每请求解析，配置变更热生效）：
/// 在 [web].keys 中按 token 查找，密钥自身的 write 字段决定读写；密钥吊销/权限变更即刻生效。
///
/// 无库模式无库内 [web] 配置，viewer token 不生效
fn viewer_access(state: &AppState, token: &str, mode: ListenerMode) -> Option<AccessLevel> {
    let web = &state.library.as_ref()?.config.current().web;
    let lookup = |keys: &[crate::core::config::WebAccessKey]| {
        keys.iter()
            .find(|key| key.token == token)
            .map(|key| AccessLevel::Viewer {
                writable: key.write,
            })
    };
    match mode {
        // 网页端口只认网站密钥；网站关闭时该端口应已停止监听，这里是纵深防御。
        ListenerMode::Web => {
            if web.enabled {
                lookup(&web.keys)
            } else {
                None
            }
        }
        // hawk-app 端口只认直连密钥，不暴露网站密钥。
        ListenerMode::Peer => lookup(&web.peer_keys),
        // 本机 API/隧道测试兼容两类。
        ListenerMode::Local => {
            lookup(&web.peer_keys).or_else(|| if web.enabled { lookup(&web.keys) } else { None })
        }
    }
}

/// viewer（局域网 web 查看）默认只读：仅放行 GET 与查询类 POST；
/// 可写密钥（[web].keys 中 write=true）解除限制（每请求经 current() 校验，保存即热生效）。
/// 会话代理的只读纵深防御复用同一白名单（remote/data.rs）。
pub(crate) fn is_viewer_allowed(req: &axum::extract::Request) -> bool {
    if req.method() == axum::http::Method::GET {
        return true;
    }
    // 查询类 POST（复杂过滤结构），语义只读
    matches!(
        req.uri().path(),
        "/api/v1/item/list" | "/api/v1/item/skeleton" | "/api/v1/item/aggregate"
    )
}

/// 广域网受限写：只开放导入素材与新建文件夹；其余写端点仍按只读处理。
/// 导入两条通道：`item/add` 的 base64（插件）与 `item/upload` 的 multipart（客户端 / app 浏览远端库时
/// 读不到本机路径，只能传内容）；path 导入由 handler 再拦（防止远端读本机文件入库）。
fn is_remote_writer_allowed(req: &axum::extract::Request) -> bool {
    if req.method() == axum::http::Method::GET {
        return true;
    }
    matches!(
        req.uri().path(),
        "/api/v1/item/list"
            | "/api/v1/item/skeleton"
            | "/api/v1/item/aggregate"
            | "/api/v1/item/add"
            | "/api/v1/item/upload"
            | "/api/v1/folder/create"
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
    if !state.startup.is_ready() && path.starts_with("/api/") && path != "/api/v1/app/startup" {
        return envelope::ApiError::new(
            envelope::codes::NOT_READY,
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "initial index is still building",
        )
        .into_response();
    }
    next.run(req).await
}

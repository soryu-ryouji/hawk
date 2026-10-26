//! OpenAPI 契约校验：代码生成（utoipa）的 schema 与路由实现、固化产物的一致性。
//!
//! 1. 同步性：固化的 openapi.json 与 `build_openapi_json()` 语义相等
//!    ——改 API 后须重新固化：`cargo run -- --dump-openapi > openapi.json`
//! 2. 完备性：schema 声明的每个 path+method 必须恰好归入 SUCCESS_CASES（成功路径校验）/
//!    WRITE_SCRIPT（写端点剧本校验）/ ROUTE_ONLY（仅路由存在）/ SSE_ENDPOINTS（SSE 不发 HTTP 请求）
//!    之一——新增端点未归类即测试失败，防止端点静默游离于契约校验之外
//! 3. 响应结构：成功路径真实调用返回 200，响应体经 jsonschema 校验
//!    （$ref 经 components 提升为文档根解析，FolderNode 自引用等循环引用可处理）
//! 4. SSE：事件名集合与 SseEvents schema 双向比对；item.added 经真实订阅捕获校验载荷

use super::lan::LanSupervisor;
use super::{build_router, build_router_for, AppState, ListenerMode, SharedState};
use crate::core::config::{LibraryConfig, WebAccessKey, WebSettings};
use crate::core::events::{EventBus, LibraryEvents};
use crate::core::index::ItemIndex;
use crate::core::metadata_store::MetadataStore;
use crate::core::paths::LibraryPaths;
use crate::core::pipeline::IndexPipeline;
use crate::core::scanner::LibraryScanner;
use crate::core::startup::StartupState;
use crate::core::taxonomy::{CategoryRegistry, ItemEvents, TagRegistry, TaxonomyMigrator};
use crate::core::thumbnail::ThumbnailService;
use crate::core::thumbnail_worker::ThumbnailWorker;
use crate::core::view_prefs::ViewPreferences;
use crate::remote::data::TunneledPeer;
use crate::settings::Settings;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt;

const TOKEN: &str = "contract-test-token";

/// 空库上可直接成功调用的端点：(method, uri, json_body)。
/// 响应体若声明了 200 schema 则校验结构
const SUCCESS_CASES: &[(&str, &str, Option<&str>)] = &[
    ("GET", "/health", None),
    ("GET", "/api/v1/app/connection", None),
    ("GET", "/api/v1/app/info", None),
    ("GET", "/api/v1/app/lan", None),
    ("GET", "/api/v1/app/startup", None),
    ("GET", "/api/v1/app/status", None),
    ("GET", "/api/v1/app/token", None),
    ("GET", "/api/v1/category/list", None),
    ("GET", "/api/v1/folder/list", None),
    ("GET", "/api/v1/global_filter/list", None),
    (
        "PUT",
        "/api/v1/global_filter",
        Some(r#"{"kind":"category","name":"契约隐藏","hidden":true}"#),
    ),
    ("GET", "/api/v1/lock/list", None),
    ("GET", "/api/v1/remote", None),
    ("GET", "/api/v1/remote/session", None),
    (
        "PUT",
        "/api/v1/remote",
        Some(r#"{"enabled":false,"url":"","service_key":""}"#),
    ),
    ("GET", "/api/v1/remote/keys", None),
    (
        "POST",
        "/api/v1/remote/keys",
        Some(r#"{"label":"契约密钥","read":true,"write":false}"#),
    ),
    ("GET", "/api/v1/item/count", None),
    ("GET", "/api/v1/library/info", None),
    (
        "PATCH",
        "/api/v1/library/info",
        Some(r#"{"name":"契约测试库"}"#),
    ),
    ("GET", "/api/v1/tag/list", None),
    ("GET", "/api/v1/view/preferences", None),
    ("POST", "/api/v1/item/list", Some("{}")),
    ("POST", "/api/v1/item/aggregate", Some(r#"{"ids":["x"]}"#)),
    ("POST", "/api/v1/item/skeleton", Some("{}")),
    ("POST", "/api/v1/library/reindex", None),
    (
        "POST",
        "/api/v1/library/storage_mode",
        Some(r#"{"mode":"database"}"#),
    ),
    ("POST", "/api/v1/library/rescan", Some("{}")),
    ("PUT", "/api/v1/library/scan", Some(r#"{"periodic":true}"#)),
    (
        "POST",
        "/api/v1/library/refresh_cache",
        Some(r#"{"type":"library"}"#),
    ),
    ("POST", "/api/v1/library/cleanup_index", None),
    ("POST", "/api/v1/trash/clear", None),
    (
        "PUT",
        "/api/v1/view/preference",
        Some(r#"{"scope":"folder:","order_by":"modification_time","order":"desc"}"#),
    ),
    ("DELETE", "/api/v1/view/preference?scope=folder%3A", None),
];

/// 写端点成功路径剧本覆盖的端点（详见 write_endpoints_match_schema）
const WRITE_SCRIPT: &[(&str, &str)] = &[
    ("GET", "/api/v1/item/detail"),
    ("GET", "/api/v1/item/file"),
    ("GET", "/api/v1/item/thumbnail"),
    ("POST", "/api/v1/item/update"),
    ("POST", "/api/v1/item/batch_update"),
    ("POST", "/api/v1/item/refresh_thumbnail"),
    ("POST", "/api/v1/item/replace"),
    ("POST", "/api/v1/item/delete"),
    ("POST", "/api/v1/item/restore"),
    ("POST", "/api/v1/item/add"),
    ("POST", "/api/v1/item/upload"),
    ("POST", "/api/v1/category/create"),
    ("POST", "/api/v1/category/update"),
    ("POST", "/api/v1/category/delete"),
    ("POST", "/api/v1/tag/create"),
    ("POST", "/api/v1/tag/update"),
    ("POST", "/api/v1/tag/delete"),
    ("POST", "/api/v1/folder/create"),
    ("POST", "/api/v1/folder/update"),
    ("POST", "/api/v1/folder/delete"),
    ("POST", "/api/v1/folder/restore"),
    ("POST", "/api/v1/lock/set"),
    ("POST", "/api/v1/lock/unlock"),
    ("POST", "/api/v1/lock/remove"),
];

/// 只校验路由存在的端点：app/lan PUT 需运行中的 LAN supervisor 收敛，不在测试内成功调用
const ROUTE_ONLY: &[(&str, &str)] = &[
    ("PUT", "/api/v1/app/lan"),
    // 局域网发现依赖真实网卡与目标设备；自动配对请求/轮询由双端串联验证
    ("GET", "/api/v1/app/lan/peers"),
    ("GET", "/api/v1/app/lan/clients"),
    ("POST", "/api/v1/app/lan/device-keys"),
    ("PATCH", "/api/v1/app/lan/device-keys/{key_id}"),
    ("DELETE", "/api/v1/app/lan/device-keys/{key_id}"),
    ("POST", "/api/v1/app/lan/pair-requests"),
    ("GET", "/api/v1/app/lan/pair-requests"),
    ("GET", "/api/v1/app/lan/pair-requests/{request_id}"),
    (
        "POST",
        "/api/v1/app/lan/pair-requests/{request_id}/decision",
    ),
    // 依赖存活的 hawk-server（转发调用），契约环境无服务端可连
    ("GET", "/api/v1/remote/devices"),
    ("POST", "/api/v1/remote/session"),
    ("DELETE", "/api/v1/remote/session"),
    ("PATCH", "/api/v1/remote/device"),
    ("POST", "/api/v1/remote/device/rotate"),
    ("DELETE", "/api/v1/remote/devices/{device_id}"),
    // key 的 PATCH/DELETE 路径参数依赖前置签发，由 SUCCESS_CASES 的 POST 覆盖创建路径
    ("PATCH", "/api/v1/remote/keys/{key_id}"),
    ("DELETE", "/api/v1/remote/keys/{key_id}"),
];

/// SSE 长连接端点：不发 HTTP 请求（oneshot 会挂起），只做 schema 存在性与事件集合校验
const SSE_ENDPOINTS: &[(&str, &str)] = &[("GET", "/api/v1/events")];

fn spec() -> Value {
    serde_json::from_str(include_str!("../../openapi.json")).expect("openapi.json 解析失败")
}

/// 测试装配：与 main.rs 同一套组件接线（无 watcher / LAN 监听 / 周期对账），
/// 目录落在系统临时目录（用毕即删）
struct TestApp {
    base: PathBuf,
    router: axum::Router,
    state: SharedState,
}

fn test_app(name: &str) -> TestApp {
    let base =
        std::env::temp_dir().join(format!("hawk-contract-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    test_app_at(base)
}

/// 在指定目录上装配（同库重开 = 迁移测试的重启语义）
fn test_app_at(base: PathBuf) -> TestApp {
    let root = base.join("library");
    let cache = base.join("cache");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&cache).unwrap();

    let root_str = root.to_string_lossy().to_string();
    let paths = LibraryPaths::new(&root_str, Some(cache.to_string_lossy().to_string()));
    let folder_tree = Arc::new(crate::api::folder::FolderTreeCache::new());
    paths.ensure_layout();
    let config = Arc::new(LibraryConfig::new(paths.clone()));
    let startup = Arc::new(StartupState::default());
    let store = Arc::new(MetadataStore::new(paths.clone(), &startup));
    let index = Arc::new(ItemIndex::default());
    let bus = EventBus::new();
    let categories = Arc::new(CategoryRegistry::new(&paths));
    let tags = Arc::new(TagRegistry::new(&paths));
    let prefs = Arc::new(ViewPreferences::new(&paths));
    let global_filter = Arc::new(crate::core::global_filter::GlobalFilter::new(&paths));
    let locks = Arc::new(crate::core::locks::Locks::new(&paths));
    let thumbs = ThumbnailService::new(Arc::new(paths.clone()));
    let worker = ThumbnailWorker::new(thumbs.clone(), bus.clone());
    let migrator = Arc::new(TaxonomyMigrator::new(
        store.clone(),
        index.clone(),
        categories.clone(),
        tags.clone(),
        locks.clone(),
        bus.clone(),
    ));
    let scanner = LibraryScanner::new(paths.clone(), config.clone());
    let settings = Settings {
        library_root: Some(root_str),
        port: 0,
        token: TOKEN.to_string(),
        reconcile_interval_seconds: 0, // 关闭元数据周期对账：测试只断言显式驱动的行为
        cache_parent: None,
        web_dist: None,
        remote_config: None,
    };
    let pipeline = IndexPipeline::new(
        paths.clone(),
        config.clone(),
        store.clone(),
        index.clone(),
        thumbs.clone(),
        bus.clone(),
        scanner,
        migrator,
        prefs.clone(),
        global_filter.clone(),
        locks.clone(),
        worker.clone(),
        startup.clone(),
        settings.clone(),
    );
    worker.attach(index.clone(), store.clone(), pipeline.sender());
    pipeline.start();
    startup.mark_ready();

    let remote_config = crate::remote::config::RemoteConfigStore::open(None, Some(&paths));
    let state: SharedState = Arc::new(AppState {
        settings,
        startup,
        bus: bus.clone(),
        sse_lagged: std::sync::atomic::AtomicU64::new(0),
        lan: LanSupervisor::new(),
        remote: crate::remote::Supervisor::new(bus.clone(), remote_config),
        library: Some(Arc::new(crate::api::LibraryState {
            paths,
            config,
            folder_tree,
            index,
            pipeline,
            store,
            thumbs,
            worker,
            prefs,
            categories,
            tags,
            global_filter,
            locks,
        })),
    });
    let router = build_router(state.clone());
    state
        .library
        .as_ref()
        .unwrap()
        .folder_tree
        .spawn_invalidation(&state.bus);
    TestApp {
        base,
        router,
        state,
    }
}

impl Drop for TestApp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

impl TestApp {
    /// 放弃清理并返回库目录（同库重开场景：旧的 TestApp 句柄不能先删库）
    fn leak(self) -> PathBuf {
        let base = self.base.clone();
        std::mem::forget(self);
        base
    }

    fn library_root(&self) -> PathBuf {
        self.base.join("library")
    }

    /// 本地库状态（测试装配必有库）
    fn lib(&self) -> &crate::api::LibraryState {
        self.state.library.as_ref().unwrap()
    }

    /// 写入 8×8 纯色 PNG 并经流水线入库（mtime 拨回一天避开写入防抖），返回 item id
    async fn add_test_item(&self, rel: &str, rgb: [u8; 3]) -> String {
        let abs = self.library_root().join(rel);
        let img = image::RgbImage::from_pixel(8, 8, image::Rgb(rgb));
        img.save(&abs).unwrap();
        let day_ago = filetime::FileTime::from_unix_time(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64
                - 86400,
            0,
        );
        filetime::set_file_mtime(&abs, day_ago).unwrap();

        let result = self
            .lib()
            .pipeline
            .submit_upsert(abs.to_string_lossy().to_string(), None)
            .await
            .expect("upsert 失败")
            .expect("upsert 被防抖延迟（mtime 已拨回，不应发生）");
        result.item.id
    }
}

/// 网站端口与 hawk-app 直连端口必须各自只接受本命名空间的密钥。
#[tokio::test]
async fn lan_ports_enforce_separate_key_namespaces() {
    let app = test_app("lan-port-auth");
    let lib = app.state.library.as_ref().unwrap();
    lib.config
        .update_web(&WebSettings {
            enabled: true,
            port: 27372,
            peer_port: 27375,
            keys: vec![WebAccessKey {
                id: "web".into(),
                label: "web".into(),
                token: "web-token".into(),
                write: false,
                created_at_ms: 0,
                ..Default::default()
            }],
            peer_keys: vec![WebAccessKey {
                id: "peer".into(),
                label: "peer".into(),
                token: "peer-token".into(),
                write: false,
                created_at_ms: 0,
                ..Default::default()
            }],
        })
        .unwrap();
    let web_router = build_router_for(app.state.clone(), ListenerMode::Web);
    let peer_router = build_router_for(app.state.clone(), ListenerMode::Peer);

    let call_auth = |router: axum::Router, token: &str| {
        let req = Request::builder()
            .method("GET")
            .uri("/api/v1/app/info")
            .header("authorization", format!("Bearer {token}"))
            .header("host", "127.0.0.1")
            .body(Body::empty())
            .unwrap();
        async move { router.oneshot(req).await.unwrap().status() }
    };

    assert_eq!(
        call_auth(web_router.clone(), "web-token").await,
        StatusCode::OK
    );
    assert_eq!(
        call_auth(web_router, "peer-token").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call_auth(peer_router.clone(), "peer-token").await,
        StatusCode::OK
    );
    assert_eq!(
        call_auth(peer_router, "web-token").await,
        StatusCode::UNAUTHORIZED
    );
}

/// 配对协议 v2 为破坏性更新：缺少 reverse 回连 profile 的旧请求必须直接拒绝。
#[tokio::test]
async fn pair_request_rejects_missing_reverse_profile() {
    let app = test_app("lan-pair-v2-required");
    app.state.lan.mark_peer_active_for_test(27375);
    let peer_router = build_router_for(app.state.clone(), ListenerMode::Peer);
    let (status, _) = call_json(
        &peer_router,
        "POST",
        "/api/v1/app/lan/pair-requests",
        Some(json!({ "client_name": "OldClient", "writable": true })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

/// 设备访问密钥与网站访问密钥完全分离：网站未配置 key 时也能创建设备 key。
#[tokio::test]
async fn lan_device_keys_are_independent_from_website_keys() {
    let app = test_app("lan-device-keys");
    let lib = app.state.library.as_ref().unwrap();
    lib.config
        .update_web(&WebSettings {
            enabled: true,
            port: 27372,
            peer_port: 27375,
            keys: vec![],
            peer_keys: vec![],
        })
        .unwrap();

    let (status, bytes) = call_json(
        &app.router,
        "POST",
        "/api/v1/app/lan/device-keys",
        Some(json!({
            "label": "Target-Mac",
            "write": false,
            "peer_id": "peer-source",
            "library_name": "Source Library",
            "url": "http://192.168.1.10:27375"
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let created: Value = serde_json::from_slice(&bytes).unwrap();
    let key_id = created["data"]["id"].as_str().unwrap().to_string();
    assert_eq!(created["data"]["label"], "Target-Mac");
    assert_eq!(created["data"]["peer_id"], "peer-source");
    assert_eq!(created["data"]["library_name"], "Source Library");
    assert_eq!(created["data"]["url"], "http://192.168.1.10:27375");
    let web = lib.config.current().web;
    assert!(web.keys.is_empty(), "不得隐式创建网站访问密钥");
    assert_eq!(web.peer_keys.len(), 1);

    let path = format!("/api/v1/app/lan/device-keys/{key_id}");
    let (status, _) = call_json(&app.router, "PATCH", &path, Some(json!({ "write": true }))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(lib.config.current().web.peer_keys[0].write);

    let (status, _) = call_json(&app.router, "DELETE", &path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(lib.config.current().web.peer_keys.is_empty());
}

/// 目标端能看到某把 peer key 最近通过直连端口发过请求（在线状态的数据源）。
#[tokio::test]
async fn lan_peer_activity_reports_online_client() {
    let app = test_app("lan-activity");
    let lib = app.state.library.as_ref().unwrap();
    lib.config
        .update_web(&WebSettings {
            enabled: false,
            port: 27372,
            peer_port: 27375,
            keys: vec![],
            peer_keys: vec![WebAccessKey {
                id: "peer".into(),
                label: "Test-Mac".into(),
                token: "peer-token".into(),
                write: false,
                created_at_ms: 0,
                peer_id: Some("source-peer".into()),
                library_name: Some("Source Library".into()),
                url: Some("http://192.168.1.20:27375".into()),
            }],
        })
        .unwrap();
    let peer_router = build_router_for(app.state.clone(), ListenerMode::Peer);
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/app/info")
        .header("authorization", "Bearer peer-token")
        .header("host", "127.0.0.1")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        peer_router.oneshot(req).await.unwrap().status(),
        StatusCode::OK
    );

    let (status, bytes) = call_json(&app.router, "GET", "/api/v1/app/lan/clients", None).await;
    assert_eq!(status, StatusCode::OK);
    let data: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(data["data"]["clients"][0]["key_id"], "peer");
    assert_eq!(data["data"]["clients"][0]["online"], true);
    assert_eq!(data["data"]["clients"][0]["write"], false);
    assert_eq!(data["data"]["clients"][0]["peer_id"], "source-peer");
    assert_eq!(data["data"]["clients"][0]["library_name"], "Source Library");
    assert_eq!(
        data["data"]["clients"][0]["url"],
        "http://192.168.1.20:27375"
    );
}

/// 桌面自动配对：源端创建请求 → 目标端列表看到 → 允许 → 源端轮询拿到 token。
#[tokio::test]
async fn put_lan_auto_pair_request_flow() {
    let app = test_app("lan-auto-pair");
    app.state.lan.mark_peer_active_for_test(27375);
    let peer_router = build_router_for(app.state.clone(), ListenerMode::Peer);

    let (status, bytes) = call_json(
        &peer_router,
        "POST",
        "/api/v1/app/lan/pair-requests",
        Some(json!({
            "client_name": "MacBook",
            "writable": true,
            "reverse": {
                "peer_id": "source-peer",
                "name": "MacBook",
                "library_name": "Source Library",
                "urls": ["http://192.168.1.10:27375"],
                "token": "reverse-token",
                "writable": false
            }
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let created: Value = serde_json::from_slice(&bytes).unwrap();
    let request_id = created["data"]["request_id"].as_str().unwrap().to_string();

    let (status, bytes) =
        call_json(&app.router, "GET", "/api/v1/app/lan/pair-requests", None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let listed: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(listed["data"]["requests"][0]["id"], request_id);
    assert_eq!(listed["data"]["requests"][0]["client_name"], "MacBook");

    let decision_path = format!("/api/v1/app/lan/pair-requests/{request_id}/decision");
    let (status, bytes) = call_json(
        &app.router,
        "POST",
        &decision_path,
        Some(json!({ "allow": true, "write": true })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let decision: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decision["data"]["peer"]["id"], "source-peer");
    assert_eq!(decision["data"]["peer"]["url"], "http://192.168.1.10:27375");
    assert_eq!(decision["data"]["peer"]["token"], "reverse-token");
    let peer_key = &app
        .state
        .library
        .as_ref()
        .unwrap()
        .config
        .current()
        .web
        .peer_keys[0];
    assert_eq!(peer_key.peer_id.as_deref(), Some("source-peer"));
    assert_eq!(peer_key.library_name.as_deref(), Some("Source Library"));
    assert_eq!(peer_key.url.as_deref(), Some("http://192.168.1.10:27375"));

    let status_path = format!("/api/v1/app/lan/pair-requests/{request_id}");
    let (status, bytes) = call_json(&peer_router, "GET", &status_path, None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let approved: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(approved["data"]["status"], "approved");
    assert_eq!(approved["data"]["protocol"], 2);
    assert_eq!(approved["data"]["writable"], true);
    assert!(approved["data"]["token"]
        .as_str()
        .is_some_and(|token| !token.is_empty()));
    assert!(approved["data"]["peer_id"]
        .as_str()
        .is_some_and(|id| !id.is_empty()));

    let lib = app.state.library.as_ref().unwrap();
    assert!(lib
        .config
        .current()
        .web
        .peer_keys
        .iter()
        .any(|key| key.write && key.label == "MacBook"));
}

/// PUT /app/lan 必须真正唤醒 supervisor 并让 hawk-app 端口开始监听。
#[tokio::test]
async fn put_lan_starts_peer_listener() {
    let app = test_app("lan-put-start");
    tokio::spawn(app.state.lan.clone().run(app.state.clone()));
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let free_port = || {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.local_addr().unwrap().port()
    };
    let web_port = free_port();
    let mut peer_port = free_port();
    while peer_port == web_port {
        peer_port = free_port();
    }

    let body = json!({
        "enabled": false,
        "port": web_port,
        "peer_port": peer_port,
        "keys": [],
        "peer_keys": [{
            "id": "peer",
            "label": "peer",
            "token": "peer-token",
            "write": false,
            "created_at_ms": 0
        }]
    });
    let (status, bytes) = call_json(&app.router, "PUT", "/api/v1/app/lan", Some(body)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["data"]["peer_active"], true);
    assert_eq!(value["data"]["peer_port"], peer_port);
}

/// 直连端口被占用时，PUT /app/lan 自动向后选可用端口并成功监听。
#[tokio::test]
async fn put_lan_auto_selects_free_peer_port() {
    let app = test_app("lan-put-autoport");
    tokio::spawn(app.state.lan.clone().run(app.state.clone()));
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let occupied = std::net::TcpListener::bind(("0.0.0.0", 0)).unwrap();
    let occupied_port = occupied.local_addr().unwrap().port();
    let free_port = || {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.local_addr().unwrap().port()
    };
    let web_port = free_port();

    let body = json!({
        "enabled": false,
        "port": web_port,
        "peer_port": occupied_port,
        "keys": [],
        "peer_keys": [{
            "id": "peer",
            "label": "peer",
            "token": "peer-token",
            "write": false,
            "created_at_ms": 0
        }]
    });
    let (status, bytes) = call_json(&app.router, "PUT", "/api/v1/app/lan", Some(body)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    let selected = value["data"]["peer_port"].as_u64().unwrap() as u16;
    assert_ne!(selected, occupied_port);
    assert_eq!(value["data"]["peer_active"], true);
    drop(occupied);
}

/// 应用启动时，已有 peer_keys/peer_port 的配置必须自动开始监听，无需用户先打开高级设置。
#[tokio::test]
async fn lan_supervisor_starts_configured_peer_port_on_boot() {
    let app = test_app("lan-boot-listen");
    let peer_port = {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.local_addr().unwrap().port()
    };
    let lib = app.state.library.as_ref().unwrap();
    lib.config
        .update_web(&WebSettings {
            enabled: false,
            port: 27372,
            peer_port,
            keys: vec![],
            peer_keys: vec![WebAccessKey {
                id: "peer".into(),
                label: "peer".into(),
                token: "peer-token".into(),
                write: false,
                created_at_ms: 0,
                ..Default::default()
            }],
        })
        .unwrap();

    tokio::spawn(app.state.lan.clone().run(app.state.clone()));
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let status = app.state.lan.snapshot();
        if status.peer.active && status.peer.port == Some(peer_port) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "启动后未自动监听 peer 端口"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// 发起请求（携带 admin token 与环回 Host 头），返回状态码与响应体字节
async fn call(
    router: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<(&str, Vec<u8>)>,
) -> (StatusCode, Vec<u8>) {
    let (content_type, bytes) = match body {
        Some((ct, b)) => (ct.to_string(), b),
        None => ("application/json".to_string(), Vec::new()),
    };
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("host", "127.0.0.1")
        .header("content-type", content_type)
        .body(Body::from(bytes))
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap();
    (status, bytes.to_vec())
}

async fn call_json(
    router: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Vec<u8>) {
    call_json_hdr(router, method, uri, body, &[]).await
}

/// call_json 的带额外请求头变体（解锁票据 X-Hawk-Unlock 等）
async fn call_json_hdr(
    router: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    extra_headers: &[(&str, &str)],
) -> (StatusCode, Vec<u8>) {
    let (content_type, bytes) = match body.map(|b| serde_json::to_vec(&b).unwrap()) {
        Some(b) => ("application/json", b),
        None => ("application/json", Vec::new()),
    };
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("host", "127.0.0.1")
        .header("content-type", content_type);
    for (k, v) in extra_headers {
        builder = builder.header(*k, *v);
    }
    let resp = router
        .clone()
        .oneshot(builder.body(Body::from(bytes)).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap();
    (status, bytes.to_vec())
}

/// 取端点 200 响应的 JSON schema，包装为自包含文档（components 提升为根，
/// 使 `#/components/schemas/...` 引用——含 FolderNode 式循环引用——可解析）
fn response_schema(spec: &Value, method: &str, path: &str) -> Option<Value> {
    let pointer = format!(
        "/paths/{}/{}/responses/200/content/application~1json/schema",
        path.replace('/', "~1"),
        method.to_lowercase()
    );
    spec.pointer(&pointer).map(|s| {
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "components": spec["components"].clone(),
            "allOf": [s.clone()],
        })
    })
}

/// 断言 200 且响应体符合端点 200 schema（无 application/json content 的端点只验 200）
async fn expect_ok(app: &TestApp, spec: &Value, method: &str, uri: &str, body: Option<Value>) {
    let (status, bytes) = call_json(&app.router, method, uri, body).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{method} {uri} 期望 200，实际 {status}，响应: {}",
        String::from_utf8_lossy(&bytes)
    );

    let path = uri.split('?').next().unwrap();
    let Some(wrapper) = response_schema(spec, method, path) else {
        return; // 二进制/文本端点无 JSON schema，仅验 200
    };
    let validator = jsonschema::validator_for(&wrapper)
        .unwrap_or_else(|e| panic!("schema 编译失败 {method} {path}: {e}"));
    let body_json: Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("响应非 JSON {method} {uri}: {e}"));
    let errors: Vec<String> = validator
        .iter_errors(&body_json)
        .map(|e| e.to_string())
        .collect();
    assert!(
        errors.is_empty(),
        "响应不符合契约 {method} {uri}:\n{}\n响应体: {body_json}",
        errors.join("\n")
    );
}

/// 无库（远程查看模式）：app/connection 报 none，不谎报可写——
/// 浏览器插件据此提示「本地 hawk 未连接素材库」而不是发一个必然失败的保存
#[tokio::test]
async fn app_connection_reports_none_without_library() {
    let dir = tempfile::tempdir().unwrap();
    let remote_config = crate::remote::config::RemoteConfigStore::open(
        Some(dir.path().join("remote.toml").to_str().unwrap()),
        None,
    );
    let bus = EventBus::new();
    let state: SharedState = Arc::new(AppState {
        settings: Settings {
            library_root: None,
            port: 0,
            token: TOKEN.to_string(),
            reconcile_interval_seconds: 0,
            cache_parent: None,
            web_dist: None,
            remote_config: None,
        },
        startup: Arc::new(StartupState::default()),
        bus: bus.clone(),
        sse_lagged: std::sync::atomic::AtomicU64::new(0),
        lan: LanSupervisor::new(),
        remote: crate::remote::Supervisor::new(bus, remote_config),
        library: None,
    });
    state.startup.mark_ready();
    let router = build_router(state);

    let (status, body) = call_json(&router, "GET", "/api/v1/app/connection", None).await;
    assert_eq!(status, StatusCode::OK);
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["data"]["mode"], "none");
    assert_eq!(body["data"]["writable"], false);
}

/// 固化产物同步：openapi.json 与代码生成语义相等（Value 比较，免疫键序/空白差异）
#[test]
fn openapi_json_in_sync() {
    let generated: Value = serde_json::from_str(&super::build_openapi_json()).unwrap();
    let fixed = spec();
    assert!(
        generated == fixed,
        "openapi.json 与代码生成不同步，请重新固化：cargo run -- --dump-openapi > openapi.json"
    );
}

/// 文档-代码契约：docs/backend/server-rest-api-v1.md 必须覆盖全部端点与 SSE 事件，
/// 且不得提及不存在的端点——API 文档靠人维护，漂移由这条测试兜住
/// （历史上 library/cleanup_index 与 library.updated 事件都曾漏记）
#[test]
fn api_doc_covers_routes_and_events() {
    let spec = spec();
    let doc = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../docs/backend/server-rest-api-v1.md"
    ))
    .expect("API 文档存在");

    // 正向：schema 的每个端点在文档中出现
    for path in spec["paths"].as_object().unwrap().keys() {
        assert!(doc.contains(path.as_str()), "API 文档缺少端点 {path}");
    }
    // 反向：文档提及的 /api/v1/... 端点都必须在 schema 中
    for path in mentioned_paths(&doc) {
        assert!(
            spec["paths"].get(&path).is_some(),
            "API 文档提及了不存在的端点 {path}"
        );
    }
    // SSE 事件名
    let events = spec["components"]["schemas"]["SseEvents"]["properties"]
        .as_object()
        .expect("SseEvents schema 存在");
    for name in events.keys() {
        assert!(doc.contains(name.as_str()), "API 文档缺少 SSE 事件 {name}");
    }
}

/// 提取文档中形如 `/api/v1/xxx` 的端点引用（小写字母/下划线/斜杠）
fn mentioned_paths(doc: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = doc;
    while let Some(i) = rest.find("/api/v1/") {
        rest = &rest[i..];
        let end = rest
            .char_indices()
            .find(|(_, c)| !(c.is_ascii_lowercase() || *c == '_' || *c == '/'))
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
        let path = rest[..end].trim_end_matches('/');
        if path.len() > "/api/v1".len() {
            out.insert(path.to_string());
        }
        rest = &rest[end..];
    }
    out
}

/// 完备性：openapi.json 声明的端点集必须恰好等于四类归类的并集
#[test]
fn openapi_endpoints_all_classified() {
    let spec = spec();
    let mut declared = BTreeSet::new();
    for (path, item) in spec["paths"].as_object().unwrap() {
        for method in ["get", "post", "put", "patch", "delete"] {
            if item.get(method).is_some() {
                declared.insert((method.to_uppercase(), path.clone()));
            }
        }
    }

    let mut covered = BTreeSet::new();
    for group in [
        SUCCESS_CASES
            .iter()
            .map(|(m, u, _)| (*m, u.split('?').next().unwrap()))
            .collect::<Vec<_>>(),
        WRITE_SCRIPT.to_vec(),
        ROUTE_ONLY.to_vec(),
        SSE_ENDPOINTS.to_vec(),
    ] {
        for (method, path) in group {
            assert!(
                covered.insert((method.to_string(), path.to_string())),
                "端点重复归类: {method} {path}"
            );
        }
    }

    let missing: Vec<_> = declared.difference(&covered).collect();
    let extra: Vec<_> = covered.difference(&declared).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "契约测试归类与 openapi.json 不一致。\n未归类（请加入 SUCCESS_CASES / WRITE_SCRIPT / ROUTE_ONLY / SSE_ENDPOINTS）: {missing:?}\n清单多余（端点已从 schema 移除）: {extra:?}"
    );
}

/// 成功路径：空库真实调用返回 200，响应体符合 200 schema
#[tokio::test]
async fn success_cases_match_schema() {
    let app = test_app("success");
    let spec = spec();
    for (method, uri, body) in SUCCESS_CASES {
        let body = body.map(|s| serde_json::from_str(s).unwrap());
        expect_ok(&app, &spec, method, uri, body).await;
    }
}

/// cleanup_index：隐藏文件 / 源文件消失的索引残留被清除，正常条目保留，磁盘文件不动。
/// 残留构造：入库后直接改索引位置 + 磁盘改名（测试装配无 watcher，绕过正常过滤——
/// 模拟早期版本入库的隐藏文件，如今增量扫描靠目录快照不再触达、只能靠本端点收敛）
#[tokio::test]
async fn cleanup_index_removes_stale_entries() {
    let app = test_app("cleanup");
    // 正常条目（应保留）
    let keep_id = app.add_test_item("keep.png", [10, 200, 30]).await;
    // 隐藏文件残留：入库正常名后把索引位置改为 .stignore，磁盘文件同步改名
    let hidden_id = app.add_test_item("plain.png", [200, 30, 30]).await;
    let root = app.library_root();
    std::fs::rename(root.join("plain.png"), root.join(".stignore")).unwrap();
    let size = std::fs::metadata(root.join(".stignore")).unwrap().len() as i64;
    app.lib()
        .index
        .add_or_update_location(&hidden_id, ".stignore", size, 0);
    app.lib().index.remove_location("plain.png");
    // 源文件消失残留：入库后直接删磁盘文件（无 watcher，索引位置残留）
    let gone_id = app.add_test_item("gone.png", [30, 30, 200]).await;
    std::fs::remove_file(root.join("gone.png")).unwrap();

    let (status, bytes) = call(&app.router, "POST", "/api/v1/library/cleanup_index", None).await;
    assert_eq!(status, StatusCode::OK, "cleanup_index");
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let data = &body["data"];
    assert_eq!(data["checked"], json!(3));
    assert_eq!(data["hidden"], json!(1));
    assert_eq!(data["missing"], json!(1));
    assert_eq!(data["removed"], json!(2));

    // 清理经流水线单写者异步应用：轮询等待索引收敛
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let paths = app.lib().index.all_location_paths();
        if paths.len() == 1 && paths[0] == "keep.png" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "索引未收敛: {paths:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(app.lib().index.count(), 1);
    assert!(app.lib().index.contains(&keep_id));
    assert!(!app.lib().index.contains(&hidden_id));
    assert!(!app.lib().index.contains(&gone_id));
    // 只清索引不动磁盘：隐藏文件本体仍在
    assert!(root.join(".stignore").exists());
}

/// item/add 同名冲突：异内容自动重命名（“名字 2”递增），同内容幂等复用不写文件——不再报 FILE_EXISTS
#[tokio::test]
async fn item_add_conflict_renames_and_dedupes() {
    let app = test_app("add-conflict");
    let encode = |rgb: [u8; 3]| {
        let img = image::RgbImage::from_pixel(8, 8, image::Rgb(rgb));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, buf.into_inner())
    };
    let add = |name: &str, b64: String| {
        call_json(
            &app.router,
            "POST",
            "/api/v1/item/add",
            Some(json!({ "img_base64": b64, "name": name })),
        )
    };

    // 异内容同名：第二张自动重命名为 a 2.png
    let (status, _) = add("a", encode([200, 30, 30])).await;
    assert_eq!(status, StatusCode::OK);
    let (status, bytes) = add("a", encode([30, 30, 200])).await;
    assert_eq!(status, StatusCode::OK, "同名异内容应自动重命名而非 409");
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["data"]["item"]["path"], json!("a 2.png"));
    assert_eq!(body["data"]["already_existed"], json!(false));

    // 同内容重复保存：幂等复用既有位置，不新增副本
    let (status, bytes) = add("a", encode([200, 30, 30])).await;
    assert_eq!(status, StatusCode::OK, "同内容重复保存应幂等成功");
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["data"]["item"]["path"], json!("a.png"));
    assert_eq!(body["data"]["already_existed"], json!(true));

    // 两份文件都在磁盘上
    let root = app.library_root();
    assert!(root.join("a.png").is_file());
    assert!(root.join("a 2.png").is_file());
}

/// folder/list 建树：隐藏目录（.stfolder 等同步标记）与 ignore 命中的目录不进侧栏；
/// 普通目录正常递归。目录树来自磁盘枚举，与索引体检（cleanup_index）是两条独立路径
#[tokio::test]
async fn folder_list_excludes_hidden_dirs() {
    let app = test_app("folder-hidden");
    let root = app.library_root();
    std::fs::create_dir_all(root.join(".stfolder")).unwrap();
    std::fs::create_dir_all(root.join("普通")).unwrap();
    app.lib().folder_tree.invalidate(); // 测试直接建目录不经 API，缓存无事件失效，手动作废

    let (status, bytes) = call(&app.router, "GET", "/api/v1/folder/list", None).await;
    assert_eq!(status, StatusCode::OK, "folder/list");
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let names: Vec<&str> = body["data"]["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap())
        .collect();
    assert!(
        !names.contains(&".stfolder"),
        "隐藏目录不应出现在侧栏: {names:?}"
    );
    assert!(names.contains(&"普通"), "普通目录应保留: {names:?}");
}

/// 清理隐藏文件残留后重启（同库重开）不复活：索引摘除同步修剪元数据（do_delete），
/// 注水也不再接收 hidden/ignore 位置（hydrate_index）——否则残留每次启动从持久层回来
#[tokio::test]
async fn cleanup_survives_restart() {
    let app = test_app("cleanup-restart");
    let id = app.add_test_item("plain.png", [200, 30, 30]).await;
    let root = app.library_root();
    std::fs::rename(root.join("plain.png"), root.join(".stignore")).unwrap();
    let size = std::fs::metadata(root.join(".stignore")).unwrap().len() as i64;
    // 真实残留是索引与元数据都带着隐藏路径（历史版本扫描写入）；两处同步构造
    app.lib()
        .index
        .add_or_update_location(&id, ".stignore", size, 0);
    app.lib().index.remove_location("plain.png");
    if let Some(mut meta) = app.lib().store.try_get(&id) {
        for p in meta.paths.iter_mut() {
            if p.path == "plain.png" {
                p.path = ".stignore".to_string();
            }
        }
        let _ = app.lib().store.save(&id, &meta);
    }

    let (status, bytes) = call(&app.router, "POST", "/api/v1/library/cleanup_index", None).await;
    assert_eq!(status, StatusCode::OK, "cleanup_index");
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["data"]["hidden"],
        json!(1)
    );

    // 清理经队列异步应用：等索引与元数据都收敛后再重开
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.lib().index.count() > 0 || app.lib().store.find_hash_by_path(".stignore").is_some() {
        assert!(std::time::Instant::now() < deadline, "清理未收敛");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    // 同库重开：MetadataStore 从持久层注水，残留不应回索引/元数据
    let base = app.leak();
    let app2 = test_app_at(base);
    assert!(
        !app2
            .lib()
            .index
            .all_location_paths()
            .contains(&".stignore".to_string()),
        "重启后隐藏文件不应复活"
    );
    assert_eq!(app2.lib().index.count(), 0);
    assert!(
        app2.lib().store.find_hash_by_path(".stignore").is_none(),
        "元数据不应再含该位置"
    );
}

/// 缩略图未命中只派发后台生成，不把原图字节作为卡片/预览响应。
#[tokio::test]
async fn thumbnail_miss_does_not_fall_back_to_original() {
    let app = test_app("thumbnail-no-original-fallback");
    let id = app.add_test_item("uncached.png", [1, 2, 3]).await;
    assert!(!app.lib().thumbs.has_all(&id));
    let original = std::fs::read(app.library_root().join("uncached.png")).unwrap();
    let (status, bytes) = call(
        &app.router,
        "GET",
        &format!("/api/v1/item/thumbnail?id={id}&size=512"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_ne!(bytes, original);
}

/// 广域网标记来自进程内受托扩展，而不是 token 或客户端自报参数。
#[tokio::test]
async fn app_info_marks_tunneled_transport() {
    let app = test_app("app-info-remote");
    let (_, bytes) = call_json(&app.router, "GET", "/api/v1/app/info", None).await;
    let local: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(local["data"]["remote"], json!(false));

    let mut req = Request::builder()
        .method("GET")
        .uri("/api/v1/app/info")
        .header("host", "hawk-remote.tunnel")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(TunneledPeer { writable: false });
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let remote: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(remote["data"]["remote"], json!(true));
}

/// gzip 只压缩可压缩文本；图片保持原始字节流，SSE 由 tower-http 默认判定排除。
#[tokio::test]
async fn gzip_is_negotiated_for_json_but_not_images() {
    let app = test_app("gzip-negotiation");
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/item/skeleton")
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("host", "127.0.0.1")
        .header("content-type", "application/json")
        .header("accept-encoding", "gzip")
        .body(Body::from("{}"))
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get("content-encoding").unwrap(), "gzip");
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(&bytes[..2], &[0x1f, 0x8b]);

    let id = app.add_test_item("image.png", [10, 20, 30]).await;
    assert!(app.lib().thumbs.generate(
        &id,
        &app.library_root().join("image.png").to_string_lossy(),
        false,
    ));
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/item/thumbnail?id={id}&size=512"))
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("host", "127.0.0.1")
        .header("accept-encoding", "gzip")
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().get("content-encoding").is_none());
}

/// 写端点剧本：准备真实 item 后按依赖顺序调用全部写端点，校验 200 与响应 schema
#[tokio::test]
async fn write_endpoints_match_schema() {
    let app = test_app("write");
    let spec = spec();
    let id = app.add_test_item("a.png", [200, 30, 30]).await;

    // 读：详情 / 原图 / 缩略图（预生成缓存；未命中不回源原图）
    expect_ok(
        &app,
        &spec,
        "GET",
        &format!("/api/v1/item/detail?id={id}"),
        None,
    )
    .await;
    let (status, bytes) = call(
        &app.router,
        "GET",
        &format!("/api/v1/item/file?id={id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "item/file");
    assert!(!bytes.is_empty(), "item/file 响应为空");
    assert!(app.lib().thumbs.generate(
        &id,
        &app.library_root().join("a.png").to_string_lossy(),
        false,
    ));
    let (status, bytes) = call(
        &app.router,
        "GET",
        &format!("/api/v1/item/thumbnail?id={id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "item/thumbnail");
    assert!(!bytes.is_empty(), "item/thumbnail 响应为空");
    let (status, grid) = call(
        &app.router,
        "GET",
        &format!("/api/v1/item/thumbnail?id={id}&size=512"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "item/thumbnail size=512");
    assert!(!grid.is_empty(), "网格缩略图响应为空");
    let (status, _) = call(
        &app.router,
        "GET",
        &format!("/api/v1/item/thumbnail?id={id}&size=256"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "非法缩略图尺寸");

    // 元数据写
    expect_ok(
        &app,
        &spec,
        "POST",
        "/api/v1/item/update",
        Some(json!({"id": id, "star": 5, "tags": ["契约标记"]})),
    )
    .await;
    expect_ok(
        &app,
        &spec,
        "POST",
        "/api/v1/item/batch_update",
        Some(json!({"ids": [id], "add_categories": ["契约分类"]})),
    )
    .await;
    expect_ok(
        &app,
        &spec,
        "POST",
        "/api/v1/item/refresh_thumbnail",
        Some(json!({"id": id})),
    )
    .await;

    // 内容替换：同内容幂等（哈希不变，直接返回当前投影）
    let same_bytes = std::fs::read(app.library_root().join("a.png")).unwrap();
    let same_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &same_bytes);
    expect_ok(
        &app,
        &spec,
        "POST",
        "/api/v1/item/replace",
        Some(json!({"id": id, "img_base64": same_b64})),
    )
    .await;

    // 分类 / 标签生命周期
    expect_ok(
        &app,
        &spec,
        "POST",
        "/api/v1/category/create",
        Some(json!({"name": "剧本分类"})),
    )
    .await;
    expect_ok(
        &app,
        &spec,
        "POST",
        "/api/v1/category/update",
        Some(json!({"name": "剧本分类", "new_name": "剧本分类2"})),
    )
    .await;
    expect_ok(
        &app,
        &spec,
        "POST",
        "/api/v1/category/delete",
        Some(json!({"name": "剧本分类2"})),
    )
    .await;
    expect_ok(
        &app,
        &spec,
        "POST",
        "/api/v1/tag/create",
        Some(json!({"name": "剧本标签"})),
    )
    .await;
    expect_ok(
        &app,
        &spec,
        "POST",
        "/api/v1/tag/update",
        Some(json!({"name": "剧本标签", "new_name": "剧本标签2"})),
    )
    .await;
    expect_ok(
        &app,
        &spec,
        "POST",
        "/api/v1/tag/delete",
        Some(json!({"name": "剧本标签2"})),
    )
    .await;

    // 锁生命周期：设锁 → 解锁（票据）→ 解除（剧本目录已建，锁其子树与根目录 item 互不影响）
    expect_ok(
        &app,
        &spec,
        "POST",
        "/api/v1/lock/set",
        Some(json!({"dimension": "folder", "name": "剧本目录", "password": "pw"})),
    )
    .await;
    expect_ok(
        &app,
        &spec,
        "POST",
        "/api/v1/lock/unlock",
        Some(json!({"dimension": "folder", "name": "剧本目录", "password": "pw"})),
    )
    .await;
    expect_ok(
        &app,
        &spec,
        "POST",
        "/api/v1/lock/remove",
        Some(json!({"dimension": "folder", "name": "剧本目录", "password": "pw"})),
    )
    .await;

    // 文件夹生命周期（delete 入回收站后 restore 放回）
    expect_ok(
        &app,
        &spec,
        "POST",
        "/api/v1/folder/create",
        Some(json!({"name": "剧本目录"})),
    )
    .await;
    expect_ok(
        &app,
        &spec,
        "POST",
        "/api/v1/folder/update",
        Some(json!({"path": "剧本目录", "name": "剧本目录2"})),
    )
    .await;
    expect_ok(
        &app,
        &spec,
        "POST",
        "/api/v1/folder/delete",
        Some(json!({"path": "剧本目录2"})),
    )
    .await;
    expect_ok(
        &app,
        &spec,
        "POST",
        "/api/v1/folder/restore",
        Some(json!({"path": "剧本目录2"})),
    )
    .await;

    // 回收站往返
    expect_ok(
        &app,
        &spec,
        "POST",
        "/api/v1/item/delete",
        Some(json!({"id": id})),
    )
    .await;
    expect_ok(
        &app,
        &spec,
        "POST",
        "/api/v1/item/restore",
        Some(json!({"id": id})),
    )
    .await;

    // 入库两通道：base64 导入新内容 + multipart 上传
    let png_b = {
        let img = image::RgbImage::from_pixel(8, 8, image::Rgb([30, 200, 30]));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    };
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png_b);
    expect_ok(
        &app,
        &spec,
        "POST",
        "/api/v1/item/add",
        Some(json!({"img_base64": b64, "name": "b"})),
    )
    .await;

    let boundary = "contract-boundary";
    let mut multipart = Vec::new();
    multipart.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"c.png\"\r\nContent-Type: image/png\r\n\r\n").as_bytes());
    multipart.extend_from_slice(&png_b);
    // 分类/标签：同一字段名重复出现（multipart 无数组类型），入库时一并写入
    for (name, value) in [
        ("categories", "风景"),
        ("categories", "海报"),
        ("tags", "夏日"),
    ] {
        multipart.extend_from_slice(
            format!("\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}").as_bytes(),
        );
    }
    multipart.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let (status, bytes) = call(
        &app.router,
        "POST",
        "/api/v1/item/upload",
        Some(("multipart/form-data; boundary=contract-boundary", multipart)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "item/upload 期望 200，实际 {status}，响应: {}",
        String::from_utf8_lossy(&bytes)
    );
    let body_json: Value = serde_json::from_slice(&bytes).unwrap();
    let wrapper = response_schema(&spec, "POST", "/api/v1/item/upload").unwrap();
    let validator = jsonschema::validator_for(&wrapper).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&body_json)
        .map(|e| e.to_string())
        .collect();
    assert!(
        errors.is_empty(),
        "item/upload 响应不符合契约:\n{}",
        errors.join("\n")
    );
    assert_eq!(
        body_json["data"]["item"]["categories"],
        json!(["风景", "海报"]),
        "上传时附带的分类应写入元数据"
    );
    assert_eq!(
        body_json["data"]["item"]["tags"],
        json!(["夏日"]),
        "上传时附带的标签应写入元数据"
    );
}

/// 路由存在：ROUTE_ONLY 端点发请求，断言非路由缺失。
/// 缺参数/实体不存在等业务拒绝是 JSON 错误信封；路由缺失（fallback 空 404）不是
#[tokio::test]
async fn route_only_endpoints_exist() {
    let app = test_app("route");
    for (method, path) in ROUTE_ONLY {
        let body = matches!(*method, "POST" | "PUT" | "DELETE").then_some("{}");
        let (status, bytes) = call(
            &app.router,
            method,
            path,
            body.map(str::as_bytes)
                .map(|b| ("application/json", b.to_vec())),
        )
        .await;
        if status == StatusCode::NOT_FOUND || status == StatusCode::METHOD_NOT_ALLOWED {
            let is_error_envelope = serde_json::from_slice::<Value>(&bytes)
                .map(|v| v["status"] == "error")
                .unwrap_or(false);
            assert!(
                is_error_envelope,
                "openapi.json 声明但路由缺失: {method} {path}（{status} 空响应）"
            );
        }
    }
}

/// SSE 契约：事件名集合与 SseEvents schema 双向比对；item.added 真实订阅捕获校验载荷
#[tokio::test]
async fn sse_events_match_schema() {
    let spec = spec();
    let declared: BTreeSet<String> = spec["components"]["schemas"]["SseEvents"]["properties"]
        .as_object()
        .expect("SseEvents schema 缺失")
        .keys()
        .cloned()
        .collect();
    let implemented: BTreeSet<String> = [
        ItemEvents::ADDED,
        ItemEvents::ITEMS_ADDED,
        ItemEvents::UPDATED,
        ItemEvents::ITEMS_UPDATED,
        ItemEvents::TRASHED,
        ItemEvents::RESTORED,
        ItemEvents::REMOVED,
        ItemEvents::FOLDER_CHANGED,
        ItemEvents::TASK_PROGRESS,
        LibraryEvents::UPDATED,
        crate::core::global_filter::GLOBAL_FILTER_CHANGED,
        crate::core::locks::LOCKS_CHANGED,
        crate::remote::session::REMOTE_SESSION_EVENT,
        crate::remote::REMOTE_STATUS_EVENT,
        crate::core::events::LanEvents::ACCESS_CHANGED,
    ]
    .map(str::to_string)
    .into_iter()
    .collect();
    assert_eq!(
        declared, implemented,
        "SseEvents schema 与 ItemEvents 常量不一致（增删事件请同步两侧）"
    );

    // 真实捕获：订阅总线 → 入库新文件 → item.added 载荷过 schema
    let app = test_app("sse");
    let mut rx = app.state.bus.subscribe();
    app.add_test_item("sse.png", [30, 30, 200]).await;
    let event = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match rx.recv().await {
                Ok(e) if e.kind == ItemEvents::ADDED => break e,
                Ok(_) => continue, // task.progress 等跳过
                Err(e) => panic!("事件总线错误: {e}"),
            }
        }
    })
    .await
    .expect("5s 内未收到 item.added");

    let wrapper = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "components": spec["components"].clone(),
        "allOf": [spec["components"]["schemas"]["SseEvents"]["properties"]["item.added"].clone()],
    });
    let validator = jsonschema::validator_for(&wrapper).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&event.payload)
        .map(|e| e.to_string())
        .collect();
    assert!(
        errors.is_empty(),
        "item.added 载荷不符合契约:\n{}\n载荷: {}",
        errors.join("\n"),
        event.payload
    );
}

/// 全局列表隐藏：global_filter 端点读写 + item/list 的 exclude_folders/categories/tags 排除语义
#[tokio::test]
async fn global_filter_exclusion() {
    let app = test_app("gfhide");
    // 根目录与子目录各一项；子目录项挂上分类与标签
    let root_id = app.add_test_item("root.png", [10, 10, 10]).await;
    std::fs::create_dir_all(app.library_root().join("积压")).unwrap();
    let sub_id = app.add_test_item("积压/inner.png", [20, 20, 20]).await;
    let (status, _) = call_json(
        &app.router,
        "POST",
        "/api/v1/item/update",
        Some(json!({"id": sub_id, "categories": ["素材堆"], "tags": ["量大"]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "item/update 挂分类标签");

    // 端点读写：标记后可从 list 读回；取消后消失
    let (status, _) = call_json(
        &app.router,
        "PUT",
        "/api/v1/global_filter",
        Some(json!({"kind": "folder", "name": "积压", "hidden": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "PUT global_filter folder");
    let (_, bytes) = call_json(&app.router, "GET", "/api/v1/global_filter/list", None).await;
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["data"]["folders"], json!(["积压"]), "隐藏文件夹读回");

    // 无排除：两项都在
    let list_total = |body: Value| async {
        let (_, bytes) = call_json(&app.router, "POST", "/api/v1/item/list", Some(body)).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        (v["data"]["total"].as_u64().unwrap(), v["data"].clone())
    };
    let (total, _) = list_total(json!({})).await;
    assert_eq!(total, 2, "无排除时应为全部两项");

    // 文件夹子树排除：只剩根目录项
    let (total, data) = list_total(json!({"exclude_folders": ["积压"]})).await;
    assert_eq!(total, 1, "exclude_folders 子树排除");
    assert_eq!(data["items"][0]["id"], json!(root_id));

    // 分类/标签排除：命中即剔
    let (total, _) = list_total(json!({"exclude_categories": ["素材堆"]})).await;
    assert_eq!(total, 1, "exclude_categories 排除");
    let (total, _) = list_total(json!({"exclude_tags": ["量大"]})).await;
    assert_eq!(total, 1, "exclude_tags 排除");

    // 维度自身视图不受影响：正向 folders 过滤仍能看到子目录项
    let (total, _) = list_total(json!({"folders": ["积压"]})).await;
    assert_eq!(total, 1, "文件夹自身视图不过滤自身");

    // 取消隐藏
    let (status, _) = call_json(
        &app.router,
        "PUT",
        "/api/v1/global_filter",
        Some(json!({"kind": "folder", "name": "积压", "hidden": false})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "PUT global_filter 取消隐藏");
    let (_, bytes) = call_json(&app.router, "GET", "/api/v1/global_filter/list", None).await;
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["data"]["folders"], json!([]), "取消后列表为空");

    // 非法参数：空路径 / 非法维度
    let (status, _) = call_json(
        &app.router,
        "PUT",
        "/api/v1/global_filter",
        Some(json!({"kind": "folder", "name": "", "hidden": true})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "空文件夹路径应 400");
    let (status, _) = call_json(
        &app.router,
        "PUT",
        "/api/v1/global_filter",
        Some(json!({"kind": "other", "name": "x", "hidden": true})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "非法维度应 400");
}

/// 锁：lock 端点生命周期 + 服务端强制（查询 403/内容 403/全局排除）+ 票据解锁（独立持有）
#[tokio::test]
async fn lock_enforcement() {
    let app = test_app("locks");
    std::fs::create_dir_all(app.library_root().join("私密")).unwrap();
    let secret_id = app.add_test_item("私密/secret.png", [99, 20, 20]).await;
    let open_id = app.add_test_item("open.png", [10, 10, 10]).await;

    // 设锁（admin token）
    let (status, _) = call_json(
        &app.router,
        "POST",
        "/api/v1/lock/set",
        Some(json!({"dimension": "folder", "name": "私密", "password": "pw123"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "lock/set");
    let (_, bytes) = call_json(&app.router, "GET", "/api/v1/lock/list", None).await;
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["data"]["folders"], json!(["私密"]), "锁列表读回");

    // 锁定视图 403 LOCKED（list 与 skeleton）
    for uri in ["/api/v1/item/list", "/api/v1/item/skeleton"] {
        let (status, body) =
            call_json(&app.router, "POST", uri, Some(json!({"folders": ["私密"]}))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri} 锁定视图应 403");
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "LOCKED");
    }

    // 全局视图：仅剩未锁定项（排除而非 403）
    let (_, bytes) = call_json(&app.router, "POST", "/api/v1/item/list", Some(json!({}))).await;
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["data"]["total"], 1, "全局视图只剩未锁定项");
    assert_eq!(v["data"]["items"][0]["id"], json!(open_id));

    // detail / file / thumbnail：内容直连 403 LOCKED（防 hash 拼 URL 绕过）
    for uri in [
        format!("/api/v1/item/detail?id={secret_id}"),
        format!("/api/v1/item/file?id={secret_id}"),
        format!("/api/v1/item/thumbnail?id={secret_id}"),
    ] {
        let (status, _) = call(&app.router, "GET", &uri, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri} 未解锁应 403");
    }
    // 未锁定项不受影响
    let (status, _) = call(
        &app.router,
        "GET",
        &format!("/api/v1/item/file?id={open_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "未锁定内容不受影响");

    // 解锁：错误密码 401；正确密码发放票据；票据经 X-Hawk-Unlock 头附带
    let (status, _) = call_json(
        &app.router,
        "POST",
        "/api/v1/lock/unlock",
        Some(json!({"dimension": "folder", "name": "私密", "password": "wrong"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "错误密码应 401");
    let (status, bytes) = call_json(
        &app.router,
        "POST",
        "/api/v1/lock/unlock",
        Some(json!({"dimension": "folder", "name": "私密", "password": "pw123"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "unlock");
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    let ticket = v["data"]["unlock_token"].as_str().unwrap().to_string();

    // 带票据：锁定视图 200、内容可见；不带票据的其他客户端仍 403（独立持有）
    let (status, _) = call_json_hdr(
        &app.router,
        "POST",
        "/api/v1/item/list",
        Some(json!({"folders": ["私密"]})),
        &[("x-hawk-unlock", &ticket)],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "带票据的锁定视图应 200");
    let (status, _) = call(
        &app.router,
        "GET",
        &format!("/api/v1/item/file?id={secret_id}&unlock={ticket}"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "带票据的内容直连应 200（query 通道）"
    );
    let (status, _) = call(
        &app.router,
        "GET",
        &format!("/api/v1/item/file?id={secret_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "无票据客户端仍 403");

    // 改密需旧密码；解除锁需密码；解除后无需票据即全部可见
    let (status, _) = call_json(
        &app.router,
        "POST",
        "/api/v1/lock/set",
        Some(json!({"dimension": "folder", "name": "私密", "password": "new", "old_password": "bad"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "旧密码错误应 403");
    let (status, _) = call_json(
        &app.router,
        "POST",
        "/api/v1/lock/remove",
        Some(json!({"dimension": "folder", "name": "私密", "password": "pw123"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "lock/remove");
    let (status, _) = call(
        &app.router,
        "GET",
        &format!("/api/v1/item/file?id={secret_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "解除锁后无需票据可见");
}

/// 分辨率档位筛选：短边（min(width, height)）与阈值比较，gte/lte 可组合成区间；
/// 宽高未解析（0×0）的 item 两个方向均不命中
#[tokio::test]
async fn item_list_filters_by_resolution_tier() {
    let app = test_app("dimfilter");
    // 测试项均为 8×8 图（短边 8）
    let a = app.add_test_item("a.png", [10, 10, 10]).await;
    app.add_test_item("b.png", [20, 20, 20]).await;

    let list = |body: Value| async {
        let (_, bytes) = call_json(&app.router, "POST", "/api/v1/item/list", Some(body)).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        (
            v["data"]["total"].as_u64().unwrap(),
            v["data"]["items"].clone(),
        )
    };

    // 以上/以下：短边含等于阈值
    let (total, _) = list(json!({"min_side_gte": 8})).await;
    assert_eq!(total, 2, "短边 ≥ 8 命中全部（含等于）");
    let (total, _) = list(json!({"min_side_gte": 9})).await;
    assert_eq!(total, 0, "短边 ≥ 9 无命中");
    let (total, _) = list(json!({"min_side_lte": 8})).await;
    assert_eq!(total, 2, "短边 ≤ 8 命中全部（含等于）");
    let (total, _) = list(json!({"min_side_lte": 7})).await;
    assert_eq!(total, 0, "短边 ≤ 7 无命中");
    // 区间组合
    let (total, _) = list(json!({"min_side_gte": 8, "min_side_lte": 8})).await;
    assert_eq!(total, 2, "区间 [8, 8] 命中全部");
    // 宽/高独立区间（同样 8×8 图）
    let (total, _) = list(json!({"min_width": 8, "max_width": 8})).await;
    assert_eq!(total, 2, "宽度区间 [8, 8] 命中全部");
    let (total, _) = list(json!({"min_height": 9})).await;
    assert_eq!(total, 0, "高度 ≥ 9 无命中");
    // 宽高组合：宽 ≥ 8 且高 ≤ 7
    let (total, _) = list(json!({"min_width": 8, "max_height": 7})).await;
    assert_eq!(total, 0, "宽高交叉条件无命中");
    // 与其它条件 AND 组合
    let (total, items) = list(json!({"min_side_gte": 8, "keywords": ["a"]})).await;
    assert_eq!(total, 1, "档位与关键词 AND");
    assert_eq!(items[0]["id"], json!(a));
}

/// 文件夹树缓存新鲜度：API 增删目录后 folder/list 立即反映（端点内同步失效）
#[tokio::test]
async fn folder_tree_cache_stays_fresh() {
    let app = test_app("foldertree");
    async fn child_names(app: &TestApp) -> Vec<String> {
        let (_, bytes) = call_json(&app.router, "GET", "/api/v1/folder/list", None).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        v["data"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["name"].as_str().unwrap().to_string())
            .collect()
    }

    assert!(child_names(&app).await.is_empty(), "空库无子目录");
    // 建目录 → 立即可见（首个 folder/list 填充缓存，create 端点内失效）
    let (status, _) = call_json(
        &app.router,
        "POST",
        "/api/v1/folder/create",
        Some(json!({"name": "甲"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        child_names(&app).await,
        vec!["甲".to_string()],
        "创建后立即可见"
    );
    // 重命名 → 跟随
    let (status, _) = call_json(
        &app.router,
        "POST",
        "/api/v1/folder/update",
        Some(json!({"path": "甲", "name": "乙"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        child_names(&app).await,
        vec!["乙".to_string()],
        "重命名后跟随"
    );
    // 删除 → 消失
    let (status, _) = call_json(
        &app.router,
        "POST",
        "/api/v1/folder/delete",
        Some(json!({"path": "乙"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(child_names(&app).await.is_empty(), "删除后不再出现");
}

/// 目录被外部删除后的幂等删除：folder/delete 对不存在目录返回成功并清理残留（索引位置/目录设置），
/// 文件夹树即时反映（用户清走陈旧条目的入口）
#[tokio::test]
async fn folder_delete_missing_dir_is_idempotent() {
    let app = test_app("folder-missing");
    std::fs::create_dir_all(app.library_root().join("幽灵")).unwrap();
    let id = app.add_test_item("幽灵/inner.png", [40, 40, 40]).await;
    // 先拉一次文件夹树（缓存填充，含「幽灵」），再外部删除整个目录（模拟资源管理器操作）
    let (_, bytes) = call_json(&app.router, "GET", "/api/v1/folder/list", None).await;
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        body["data"]["children"][0]["name"],
        json!("幽灵"),
        "缓存应含新目录"
    );
    std::fs::remove_dir_all(app.library_root().join("幽灵")).unwrap();

    // 删除不存在的目录：成功而非 FOLDER_NOT_FOUND
    let (status, bytes) = call_json(
        &app.router,
        "POST",
        "/api/v1/folder/delete",
        Some(json!({"path": "幽灵"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "幂等删除应成功: {}",
        String::from_utf8_lossy(&bytes)
    );

    // 索引位置经 Delete job 按前缀清除（notify_deleted 为火忘任务，轮询索引收敛）
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.lib().index.contains(&id) && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(!app.lib().index.contains(&id), "目录下残留位置应被清除");

    // 文件夹树不含已删目录
    let (_, bytes) = call_json(&app.router, "GET", "/api/v1/folder/list", None).await;
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["data"]["children"], json!([]));
}

/// 存储方案迁移往返：新库默认数据库模式；切配置文件 → TOML 落盘、重开探测为 toml、数据完好；
/// 切回数据库 → metadata.db 存在、TOML 清除、重开探测回数据库
#[tokio::test]
async fn storage_mode_migration_roundtrip() {
    let app = test_app("storage-migrate");

    // 新库默认数据库模式
    async fn info_mode(app: &TestApp) -> String {
        let (_, bytes) = call_json(&app.router, "GET", "/api/v1/library/info", None).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        v["data"]["storage_mode"].as_str().unwrap().to_string()
    }
    assert_eq!(info_mode(&app).await, "database");

    let id = app.add_test_item("a.png", [1, 1, 1]).await;
    let (status, _) = call_json(
        &app.router,
        "POST",
        "/api/v1/item/update",
        Some(json!({"id": id, "tags": ["迁移标记"]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // database → toml：TOML 落盘且含标签
    let (status, bytes) = call_json(
        &app.router,
        "POST",
        "/api/v1/library/storage_mode",
        Some(json!({"mode": "toml"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "迁移到 toml: {}",
        String::from_utf8_lossy(&bytes)
    );
    let toml_file = app.library_root().join(format!(".hawk/metadata/{id}.toml"));
    let text = std::fs::read_to_string(&toml_file).expect("迁移后 TOML 应存在");
    assert!(text.contains("迁移标记"));

    // 重开：探测为 toml，数据完好
    let base = app.leak();
    let app2 = test_app_at(base.clone());
    assert_eq!(info_mode(&app2).await, "toml");
    let (_, bytes) = call_json(
        &app2.router,
        "GET",
        &format!("/api/v1/item/detail?id={id}"),
        None,
    )
    .await;
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["data"]["tags"], json!(["迁移标记"]));

    // toml → database：metadata.db 建立、TOML 清除
    let (status, _) = call_json(
        &app2.router,
        "POST",
        "/api/v1/library/storage_mode",
        Some(json!({"mode": "database"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!toml_file.exists(), "切回数据库后旧 TOML 应删除");
    assert!(app2.library_root().join(".hawk/metadata.db").exists());

    // 再重开：探测回 database，数据完好
    let base = app2.leak();
    let app3 = test_app_at(base.clone());
    assert_eq!(info_mode(&app3).await, "database");
    let (_, bytes) = call_json(
        &app3.router,
        "GET",
        &format!("/api/v1/item/detail?id={id}"),
        None,
    )
    .await;
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["data"]["tags"], json!(["迁移标记"]));

    let _ = std::fs::remove_dir_all(&base);
}

/// 选择集聚合：common_taxonomy 交集语义 + batch_update 的 remove_tags/remove_categories 摘除
#[tokio::test]
async fn aggregate_and_batch_remove() {
    let app = test_app("aggregate");
    let id_a = app.add_test_item("a.png", [11, 11, 11]).await;
    let id_b = app.add_test_item("b.png", [12, 12, 12]).await;
    // a: 共有+独有；b: 共有+另一独有 + 分类
    call_json(
        &app.router,
        "POST",
        "/api/v1/item/update",
        Some(json!({"id": id_a, "tags": ["共有", "仅A"], "categories": ["共有分类"]})),
    )
    .await;
    call_json(
        &app.router,
        "POST",
        "/api/v1/item/update",
        Some(json!({"id": id_b, "tags": ["共有", "仅B"], "categories": ["共有分类", "仅B分类"]})),
    )
    .await;

    // 交集：tags=[共有]，categories=[共有分类]
    let (_, bytes) = call_json(
        &app.router,
        "POST",
        "/api/v1/item/aggregate",
        Some(json!({"ids": [id_a, id_b]})),
    )
    .await;
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["data"]["common_tags"], json!(["共有"]));
    assert_eq!(body["data"]["common_categories"], json!(["共有分类"]));

    // 空 ids 报错；不存在的 id 静默跳过
    let (status, _) = call_json(
        &app.router,
        "POST",
        "/api/v1/item/aggregate",
        Some(json!({"ids": []})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, bytes) = call_json(
        &app.router,
        "POST",
        "/api/v1/item/aggregate",
        Some(json!({"ids": ["ghost"]})),
    )
    .await;
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["data"]["common_tags"], json!([]));

    // 批量摘除共有标签/分类：remove 语义生效
    let (status, bytes) = call_json(
        &app.router,
        "POST",
        "/api/v1/item/batch_update",
        Some(json!({"ids": [id_a, id_b], "remove_tags": ["共有"], "remove_categories": ["共有分类"]})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let (_, bytes) = call_json(
        &app.router,
        "POST",
        "/api/v1/item/aggregate",
        Some(json!({"ids": [id_a, id_b]})),
    )
    .await;
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["data"]["common_tags"], json!([]), "共有标签应已摘除");
    assert_eq!(body["data"]["common_categories"], json!([]));

    // 各自的独有标签不受影响
    let (_, bytes) = call_json(
        &app.router,
        "GET",
        &format!("/api/v1/item/detail?id={id_a}"),
        None,
    )
    .await;
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["data"]["tags"], json!(["仅A"]));
}

/// 广域网受限写：只放行导入（item/add base64、item/upload multipart，path 导入被 handler 拒）与
/// folder/create，其余写端点 403；只读隧道流量保持 viewer 只读。
#[tokio::test]
async fn remote_writer_is_limited_to_import_and_folder_create() {
    let app = test_app("remote-writer");
    let mut remote = app.state.remote.remote_config().current();
    remote.writable = true;
    app.state.remote.remote_config().update(&remote).unwrap();

    fn tunnel_req(method: &str, uri: &str, body: Value, writable: bool) -> Request<Body> {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("host", "hawk-remote.tunnel")
            .body(Body::from(body.to_string()))
            .unwrap();
        req.extensions_mut().insert(TunneledPeer { writable });
        req
    }

    // path 导入被 handler 拒绝（防止远端读取本机文件）
    let resp = app
        .router
        .clone()
        .oneshot(tunnel_req(
            "POST",
            "/api/v1/item/add",
            json!({ "path": "/etc/passwd" }),
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // 删除等库写操作仍被限制
    let resp = app
        .router
        .clone()
        .oneshot(tunnel_req(
            "POST",
            "/api/v1/item/delete",
            json!({ "id": "x" }),
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // multipart 上传属于导入（客户端无本机路径语义）：过门禁（缺文件由 handler 报错，不是 403）
    let resp = app
        .router
        .clone()
        .oneshot(tunnel_req("POST", "/api/v1/item/upload", json!({}), true))
        .await
        .unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "item/upload 应放行给受限写会话"
    );

    // 新建文件夹放行
    let resp = app
        .router
        .clone()
        .oneshot(tunnel_req(
            "POST",
            "/api/v1/folder/create",
            json!({ "name": "远程新建", "parent_path": null }),
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 只读隧道流量不能写
    let resp = app
        .router
        .clone()
        .oneshot(tunnel_req(
            "POST",
            "/api/v1/folder/create",
            json!({ "name": "只读不能建", "parent_path": null }),
            false,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

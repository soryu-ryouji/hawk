//! 远程查看模式（`remote` 子命令，无本地库）端到端契约：
//! 库端点一律 400 NO_LIBRARY；启动/信息/远程管理面（admin）可用；
//! 「允许其他设备访问」在无库时被拒（没有素材可服务）。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hawk_daemon::api::{build_router, build_router_for, AppState, ListenerMode, SharedState};
use hawk_daemon::core::events::EventBus;
use hawk_daemon::core::startup::StartupState;
use hawk_daemon::remote::config::RemoteConfigStore;
use hawk_daemon::remote::data::{serve_proxy, DataPlane, ProxyContext, HAWK_ALPN};
use hawk_daemon::remote::identity::Identity;
use hawk_daemon::settings::Settings;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const TOKEN: &str = "no-library-test-token";

/// 无库装配：远程配置显式落临时目录（无库模式无库内回退位，settings 层已强校验）
fn no_library_state(dir: &tempfile::TempDir) -> SharedState {
    let settings = Settings {
        library_root: None,
        port: 0,
        admin_token: TOKEN.to_string(),
        maintenance_interval_seconds: 0,
        cache_parent: None,
        web_dist: None,
        remote_config: Some(
            dir.path()
                .join("remote.toml")
                .to_string_lossy()
                .into_owned(),
        ),
    };
    let startup = Arc::new(StartupState::default());
    startup.mark_ready(); // 无库无索引构建，监听就绪即 ready（bootstrap 语义）
    let bus = EventBus::new();
    let remote_config = RemoteConfigStore::open(settings.remote_config.as_deref(), None);
    Arc::new(AppState {
        settings,
        web_sessions: std::sync::Mutex::new(std::collections::HashMap::new()),
        integration_sessions: std::sync::Mutex::new(std::collections::HashMap::new()),
        startup,
        bus: bus.clone(),
        sse_lagged: std::sync::atomic::AtomicU64::new(0),
        lan: hawk_daemon::api::lan::LanSupervisor::new(),
        remote: hawk_daemon::remote::Supervisor::new(bus, remote_config),
        library: None,
    })
}

#[tokio::test]
async fn lan_rendezvous_issues_target_identity_and_scoped_ticket() {
    let dir = tempfile::tempdir().unwrap();
    let state = no_library_state(&dir);
    let identity = Identity::load_or_create(dir.path().join("remote").to_str().unwrap()).unwrap();
    let plane = DataPlane::bind(&identity, 0, None, axum::Router::new())
        .await
        .unwrap();
    *state.remote.endpoint_slot().write().unwrap() = Some(plane);
    let router = build_router_for(state.clone(), ListenerMode::Peer);
    let source = iroh::SecretKey::generate();
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/rendezvous")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "source_endpoint_id": source.public().to_string(),
                "source_name": "手机",
            })
            .to_string(),
        ))
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    let session_id = body["data"]["session_id"].as_str().unwrap();
    let ticket = body["data"]["rendezvous_token"].as_str().unwrap();
    assert_eq!(body["data"]["state"], "ready");

    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/rendezvous/{session_id}"))
        .header("authorization", format!("Bearer {ticket}"))
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["data"]["connect"]["endpoint_id"], identity.endpoint_id);
    assert!(!body["data"]["connect"]["direct_addresses"]
        .as_array()
        .unwrap()
        .is_empty());

    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/api/v1/rendezvous/{session_id}"))
        .header("authorization", format!("Bearer {ticket}"))
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/rendezvous/{session_id}"))
        .header("authorization", format!("Bearer {ticket}"))
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn access_key_login_and_revocation_work_over_iroh() {
    let dir_a = tempfile::tempdir().unwrap();
    let state = no_library_state(&dir_a);
    let mut remote = state.remote.remote_config().current();
    remote.allow_import = true;
    state.remote.remote_config().update(&remote).unwrap();
    let (entry, access_key) = state
        .remote
        .keys
        .create(
            "test-library",
            "手机",
            hawk_daemon::core::access::Role::Importer,
            None,
        )
        .unwrap();
    let identity_a =
        Identity::load_or_create(dir_a.path().join("identity-a").to_str().unwrap()).unwrap();
    let id_a: iroh::EndpointId = identity_a.endpoint_id.parse().unwrap();
    let plane_a = DataPlane::bind(&identity_a, 0, None, build_router(state.clone()))
        .await
        .unwrap();
    *state.remote.endpoint_slot().write().unwrap() = Some(plane_a.clone());
    let dir_b = tempfile::tempdir().unwrap();
    let identity_b =
        Identity::load_or_create(dir_b.path().join("identity-b").to_str().unwrap()).unwrap();
    let id_b: iroh::EndpointId = identity_b.endpoint_id.parse().unwrap();
    plane_a.allow_access_session("s_access", id_b);
    let plane_b = DataPlane::bind(&identity_b, 0, None, axum::Router::new())
        .await
        .unwrap();
    let addr =
        iroh::EndpointAddr::new(id_a).with_addrs(plane_a.endpoint().unwrap().addr().addrs.clone());
    let conn = plane_b
        .endpoint()
        .unwrap()
        .connect(addr, HAWK_ALPN)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let ctx = std::sync::Arc::new(ProxyContext {
        conn: std::sync::Arc::new(std::sync::RwLock::new(conn)),
        session_token: "proxy-token".to_string(),
        session_id: "s_access".to_string(),
        role: std::sync::RwLock::new(hawk_daemon::core::access::Role::Viewer),
    });
    tokio::spawn(serve_proxy(ctx, listener));
    let client = reqwest::Client::new();

    // 未登录：只能调用登录端点，其他 API 即使存在也拒绝。
    let resp = client
        .get(format!("http://127.0.0.1:{port}/api/v1/app/info"))
        .bearer_auth("proxy-token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    let resp: Value = client
        .post(format!(
            "http://127.0.0.1:{port}/api/v1/remote/access-session"
        ))
        .bearer_auth("proxy-token")
        .json(&json!({ "access_key": access_key }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp["status"], "success", "{resp}");
    assert_eq!(resp["data"]["role"], "importer");
    let resp: Value = client
        .get(format!("http://127.0.0.1:{port}/api/v1/app/info"))
        .bearer_auth("proxy-token")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp["data"]["role"], "importer");

    // 删除 AccessKey 后，已有隧道下一请求立即失效。
    state.remote.keys.remove(&entry.id).unwrap();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/api/v1/app/info"))
        .bearer_auth("proxy-token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

async fn call(
    router: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("host", "127.0.0.1")
        .header("content-type", "application/json")
        .body(Body::from(
            body.map(|b| serde_json::to_vec(&b).unwrap())
                .unwrap_or_default(),
        ))
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
async fn no_library_mode_serves_remote_plane_only() {
    let dir = tempfile::tempdir().unwrap();
    let router = build_router(no_library_state(&dir));

    // 启动与信息：就绪 + has_library=false
    let (status, body) = call(&router, "GET", "/api/v1/app/startup", None).await;
    assert_eq!(status, StatusCode::OK, "startup");
    assert_eq!(body["data"]["status"], json!("ready"));
    let (status, body) = call(&router, "GET", "/api/v1/app/info", None).await;
    assert_eq!(status, StatusCode::OK, "app/info");
    assert_eq!(body["data"]["has_library"], json!(false));
    assert_eq!(body["data"]["role"], json!("admin"));
    assert_eq!(body["data"]["channel"], json!("loopback"));

    // 库端点一律 400 NO_LIBRARY（读/写/注册表各取一代表）
    for (method, uri, body) in [
        ("GET", "/api/v1/library/info", None),
        ("GET", "/api/v1/folder/list", None),
        ("POST", "/api/v1/item/list", Some(json!({}))),
        ("GET", "/api/v1/item/count", None),
        ("GET", "/api/v1/category/list", None),
        ("GET", "/api/v1/tag/list", None),
        ("GET", "/api/v1/lock/list", None),
        ("GET", "/api/v1/global_filter/list", None),
        ("GET", "/api/v1/view/preferences", None),
        ("POST", "/api/v1/trash/clear", None),
        ("GET", "/api/v1/app/status", None),
        ("GET", "/api/v1/app/lan", None),
    ] {
        let (status, resp) = call(&router, method, uri, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri} 应 400");
        assert_eq!(resp["error"]["code"], json!("NO_LIBRARY"), "{method} {uri}");
    }

    // 远程管理面（admin）可用：配置读写 + 密钥列表 + 会话读取
    let (status, _) = call(&router, "GET", "/api/v1/remote", None).await;
    assert_eq!(status, StatusCode::OK, "GET remote");
    let (status, _) = call(
        &router,
        "PUT",
        "/api/v1/remote",
        Some(json!({"enabled": false, "url": "", "tenant_key": ""})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "PUT remote");
    let (status, body) = call(&router, "GET", "/api/v1/remote/keys", None).await;
    assert_eq!(status, StatusCode::OK, "GET remote/keys");
    assert_eq!(body["data"]["keys"], json!([]));
    let (status, body) = call(&router, "GET", "/api/v1/remote/session", None).await;
    assert_eq!(status, StatusCode::OK, "GET remote/session");
    assert_eq!(body["data"], Value::Null);

    // 无库设备禁止打开「允许其他设备访问」（未注册场景先报未注册，注册语义由 signal 测试覆盖）
    let (status, resp) = call(
        &router,
        "PATCH",
        "/api/v1/remote/device",
        Some(json!({"allow_remote": true})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "无库开 allow_remote 应 400"
    );
    assert_eq!(resp["error"]["code"], json!("NO_LIBRARY"));
}

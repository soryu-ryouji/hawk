//! 远程查看模式（无本地库，--library 缺省）端到端契约：
//! 库端点一律 400 NO_LIBRARY；启动/信息/远程管理面（admin）可用；
//! 「允许其他设备访问」在无库时被拒（没有素材可服务）。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hawk_daemon::api::{build_router, AppState, SharedState};
use hawk_daemon::core::events::EventBus;
use hawk_daemon::core::startup::StartupState;
use hawk_daemon::remote::config::RemoteConfigStore;
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
        token: TOKEN.to_string(),
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
        startup,
        bus: bus.clone(),
        sse_lagged: std::sync::atomic::AtomicU64::new(0),
        lan: hawk_daemon::api::lan::LanSupervisor::new(),
        remote: hawk_daemon::remote::Supervisor::new(bus, remote_config),
        library: None,
    })
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
    assert_eq!(body["data"]["access"], json!("admin"));

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
        Some(json!({"enabled": false, "url": "", "service_key": ""})),
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

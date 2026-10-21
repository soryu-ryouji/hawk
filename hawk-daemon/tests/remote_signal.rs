//! remote 信令面集成测试：mock hawk-server（协议形状），验证注册 → 心跳 → WSS 控制通道
//! → session.request 自动接受的全流程，以及 401 致命错误状态。
//! mock 只复刻协议端点的最小行为，不复用 hawk-server 代码（两 crate 零代码共享）。

use axum::extract::ws::Message;
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use hawk_daemon::core::config::{LibraryConfig, RemoteSettings};
use hawk_daemon::core::paths::LibraryPaths;
use hawk_daemon::remote::{RemoteState, Supervisor};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const KEY: &str = "test-access-key";

#[derive(Clone, Default)]
struct MockHub {
    heartbeats: Arc<AtomicU64>,
    accepted: Arc<Mutex<Vec<String>>>,
}

impl MockHub {
    fn heartbeat_count(&self) -> u64 {
        self.heartbeats.load(Ordering::Relaxed)
    }

    fn accepted_sessions(&self) -> Vec<String> {
        self.accepted.lock().unwrap().clone()
    }
}

async fn spawn_mock_hub() -> (String, MockHub) {
    let hub = MockHub::default();
    let app = Router::new()
        .route("/api/v1/devices", post(register))
        .route("/api/v1/devices/heartbeat", post(heartbeat))
        .route("/api/v1/sessions/{session_id}/accept", post(accept))
        .route("/api/v1/ws", get(ws_channel))
        .with_state(hub.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), hub)
}

fn authorized(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        == Some(&format!("Bearer {KEY}")[..])
}

async fn register(headers: axum::http::HeaderMap, Json(body): Json<Value>) -> Response {
    if !authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"status":"error","error":{"message":"访问密钥无效"}})),
        )
            .into_response();
    }
    assert_eq!(body["status"], Value::Null); // 请求体不带信封
    assert!(body["fingerprint"].as_str().unwrap().len() == 64);
    Json(json!({"status":"success","data":{"device_id":"d_mock"}})).into_response()
}

async fn heartbeat(State(hub): State<MockHub>, headers: axum::http::HeaderMap) -> Response {
    if !authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"status":"error","error":{"message":"访问密钥无效"}})),
        )
            .into_response();
    }
    hub.heartbeats.fetch_add(1, Ordering::Relaxed);
    Json(json!({"status":"success","data":{"server_time_ms":0,"public_ip_seen":"127.0.0.1"}}))
        .into_response()
}

async fn accept(
    State(hub): State<MockHub>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(session_id): axum::extract::Path<String>,
) -> Response {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    hub.accepted.lock().unwrap().push(session_id);
    Json(json!({"status":"success"})).into_response()
}

async fn ws_channel(
    Query(query): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Response {
    if query.get("key").map(String::as_str) != Some(KEY) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let device_id = query.get("device_id").cloned().unwrap_or_default();
    ws.on_upgrade(move |mut socket| async move {
        let hello = json!({"type":"hello","device_id":device_id,"server_time_ms":0});
        socket
            .send(Message::Text(hello.to_string().into()))
            .await
            .unwrap();
        // 等 hello_ack 后推 session.request
        let _ = tokio::time::timeout(Duration::from_secs(5), socket.recv()).await;
        let request = json!({
            "type": "session.request",
            "session_id": "s_1",
            "from": {"device_id":"d_other","name":"对方","library_name":"库"},
            "mode": "direct",
        });
        socket
            .send(Message::Text(request.to_string().into()))
            .await
            .unwrap();
        // 持有连接直至对端关闭
        while let Some(Ok(_)) = socket.recv().await {}
    })
}

struct TestLibrary {
    _dir: tempfile::TempDir,
    paths: LibraryPaths,
    config: Arc<LibraryConfig>,
}

fn test_library() -> TestLibrary {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("library");
    std::fs::create_dir_all(&root).unwrap();
    let paths = LibraryPaths::new(
        root.to_str().unwrap(),
        Some(dir.path().join("cache").to_str().unwrap().to_string()),
    );
    let config = Arc::new(LibraryConfig::new(paths.clone()));
    TestLibrary {
        _dir: dir,
        paths,
        config,
    }
}

async fn wait_state(supervisor: &Supervisor, want: RemoteState, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if supervisor.snapshot().state == want {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "等待状态 {want:?} 超时"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn signal_registers_heartbeats_and_accepts_sessions() {
    let (base, hub) = spawn_mock_hub().await;
    let lib = test_library();
    lib.config
        .update_remote(&RemoteSettings {
            enabled: true,
            url: base,
            key: KEY.to_string(),
        })
        .unwrap();

    let supervisor = Supervisor::new();
    let task = tokio::spawn(
        supervisor
            .clone()
            .run(lib.paths.clone(), lib.config.clone()),
    );

    // 注册 + WSS 连接 → Online；首次心跳即刻发出
    wait_state(&supervisor, RemoteState::Online, Duration::from_secs(10)).await;
    assert_eq!(supervisor.snapshot().device_id.as_deref(), Some("d_mock"));
    assert!(hub.heartbeat_count() >= 1);

    // 设备身份落盘（含服务端分配的 device_id）
    let identity =
        hawk_daemon::remote::identity::Identity::load_or_create(&lib.paths.remote_dir).unwrap();
    assert_eq!(identity.device_id.as_deref(), Some("d_mock"));

    // mock 推送 session.request → 客户端自动 accept
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while hub.accepted_sessions() != vec!["s_1".to_string()] {
        assert!(tokio::time::Instant::now() < deadline, "等待会话接受超时");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    task.abort();
}

#[tokio::test]
async fn signal_reports_fatal_on_invalid_key() {
    let (base, _hub) = spawn_mock_hub().await;
    let lib = test_library();
    lib.config
        .update_remote(&RemoteSettings {
            enabled: true,
            url: base,
            key: "wrong-key".to_string(),
        })
        .unwrap();

    let supervisor = Supervisor::new();
    let task = tokio::spawn(
        supervisor
            .clone()
            .run(lib.paths.clone(), lib.config.clone()),
    );

    // 401 = 致命：状态置 Error（不重试风暴，等配置变更）
    wait_state(&supervisor, RemoteState::Error, Duration::from_secs(10)).await;
    assert!(supervisor
        .snapshot()
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("访问密钥无效"));
    task.abort();
}

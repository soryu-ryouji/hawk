//! remote 信令面集成测试：mock hawk-server（协议形状），验证注册 → 心跳 → WSS 控制通道
//! → session.request 自动接受的全流程，以及 401 致命错误状态。
//! mock 只复刻协议端点的最小行为，不复用 hawk-server 代码（两 crate 零代码共享）。

use axum::extract::ws::Message;
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use hawk_daemon::core::config::LibraryConfig;
use hawk_daemon::core::paths::LibraryPaths;
use hawk_daemon::remote::config::{RemoteConfigStore, RemoteSettings};
use hawk_daemon::remote::{RemoteState, Supervisor};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const KEY: &str = "test-access-key";

/// 串行化数据面测试：hot_restart 使用固定 UDP 端口，并发测试的 quic_port=0 可能抢占它
static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Default)]
struct MockHub {
    heartbeats: Arc<AtomicU64>,
    accepted: Arc<Mutex<Vec<String>>>,
    rejected: Arc<Mutex<Vec<String>>>,
    register_count: Arc<AtomicU64>,
    /// mock 服务端推送写会话请求（否则默认只读）
    write_request: Arc<std::sync::atomic::AtomicBool>,
    /// 模拟服务端设备表丢失：心跳一律 404
    lost_devices: Arc<std::sync::atomic::AtomicBool>,
    /// 模拟服务端设备表丢失（控制通道面）：WS 握手 404 并踢掉已连接通道
    lost_ws_devices: Arc<std::sync::atomic::AtomicBool>,
}

impl MockHub {
    fn heartbeat_count(&self) -> u64 {
        self.heartbeats.load(Ordering::Relaxed)
    }

    fn accepted_sessions(&self) -> Vec<String> {
        self.accepted.lock().unwrap().clone()
    }

    fn rejected_sessions(&self) -> Vec<String> {
        self.rejected.lock().unwrap().clone()
    }

    fn set_write_request(&self, on: bool) {
        self.write_request.store(on, Ordering::Relaxed);
    }

    fn register_count(&self) -> u64 {
        self.register_count.load(Ordering::Relaxed)
    }
}

async fn spawn_mock_hub() -> (String, MockHub) {
    let hub = MockHub::default();
    let app = Router::new()
        .route("/api/v1/devices", post(register).get(devices))
        .route("/api/v1/devices/heartbeat", post(heartbeat))
        .route("/api/v1/sessions/{session_id}/accept", post(accept))
        .route("/api/v1/sessions/{session_id}/reject", post(reject))
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

async fn register(
    State(hub): State<MockHub>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"status":"error","error":{"message":"访问密钥无效"}})),
        )
            .into_response();
    }
    assert_eq!(body["status"], Value::Null); // 请求体不带信封
    assert!(body["fingerprint"].as_str().unwrap().len() == 64);
    let n = hub.register_count.fetch_add(1, Ordering::Relaxed) + 1;
    Json(json!({"status":"success","data":{"device_id":format!("d_{n}")}})).into_response()
}

async fn heartbeat(State(hub): State<MockHub>, headers: axum::http::HeaderMap) -> Response {
    if !authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"status":"error","error":{"message":"访问密钥无效"}})),
        )
            .into_response();
    }
    if hub.lost_devices.load(Ordering::Relaxed) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"status":"error","error":{"message":"设备不存在"}})),
        )
            .into_response();
    }
    hub.heartbeats.fetch_add(1, Ordering::Relaxed);
    Json(json!({"status":"success","data":{"server_time_ms":0,"public_ip_seen":"127.0.0.1"}}))
        .into_response()
}

async fn devices(headers: axum::http::HeaderMap) -> Response {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let fingerprint = "11".repeat(32);
    Json(json!({"status":"success","data":{"devices":[
        {"device_id":"d_other","name":"对方","platform":"macos","library_name":"库",
         "online":true,"allow_remote":true,"allow_write":true,"fingerprint":fingerprint}
    ]}}))
    .into_response()
}

async fn reject(
    State(hub): State<MockHub>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(session_id): axum::extract::Path<String>,
) -> Response {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    hub.rejected.lock().unwrap().push(session_id);
    Json(json!({"status":"success"})).into_response()
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
    State(hub): State<MockHub>,
    Query(query): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Response {
    if query.get("key").map(String::as_str) != Some(KEY) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if hub.lost_ws_devices.load(Ordering::Relaxed) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"status":"error","error":{"message":"设备不存在"}})),
        )
            .into_response();
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
            "access": if hub.write_request.load(Ordering::Relaxed) { "write" } else { "read" },
        });
        socket
            .send(Message::Text(request.to_string().into()))
            .await
            .unwrap();
        // 持有连接直至对端关闭（或模拟服务端丢设备时主动踢下线）
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(20)) => {
                    if hub.lost_ws_devices.load(Ordering::Relaxed) {
                        break;
                    }
                }
                msg = socket.recv() => {
                    if !matches!(msg, Some(Ok(_))) {
                        break;
                    }
                }
            }
        }
    })
}

struct TestLibrary {
    _dir: tempfile::TempDir,
    paths: LibraryPaths,
    config: Arc<LibraryConfig>,
    remote: Arc<RemoteConfigStore>,
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
    let remote = RemoteConfigStore::open(
        Some(dir.path().join("remote.toml").to_str().unwrap()),
        &paths,
    );
    TestLibrary {
        _dir: dir,
        paths,
        config,
        remote,
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
    let _guard = TEST_LOCK.lock().await;
    let (base, hub) = spawn_mock_hub().await;
    let lib = test_library();
    lib.remote
        .update(&RemoteSettings {
            enabled: true,
            url: base,
            key: KEY.to_string(),
            quic_port: 0, // OS 分配，避免测试间端口冲突
            writable: false,
        })
        .unwrap();

    let supervisor = Supervisor::new(
        hawk_daemon::core::events::EventBus::new(),
        lib.remote.clone(),
    );
    let task = tokio::spawn(supervisor.clone().run(
        lib.paths.clone(),
        lib.config.clone(),
        axum::Router::new(),
    ));

    // 注册 + WSS 连接 → Online；首次心跳即刻发出
    wait_state(&supervisor, RemoteState::Online, Duration::from_secs(10)).await;
    assert_eq!(supervisor.snapshot().device_id.as_deref(), Some("d_1"));
    assert!(hub.heartbeat_count() >= 1);

    // 设备身份落盘（含服务端分配的 device_id）
    let identity =
        hawk_daemon::remote::identity::Identity::load_or_create(lib.remote.identity_dir()).unwrap();
    assert_eq!(identity.device_id.as_deref(), Some("d_1"));

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
    let _guard = TEST_LOCK.lock().await;
    let (base, _hub) = spawn_mock_hub().await;
    let lib = test_library();
    lib.remote
        .update(&RemoteSettings {
            enabled: true,
            url: base,
            key: "wrong-key".to_string(),
            quic_port: 0,
            writable: false,
        })
        .unwrap();

    let supervisor = Supervisor::new(
        hawk_daemon::core::events::EventBus::new(),
        lib.remote.clone(),
    );
    let task = tokio::spawn(supervisor.clone().run(
        lib.paths.clone(),
        lib.config.clone(),
        axum::Router::new(),
    ));

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

#[tokio::test]
async fn signal_re_registers_when_server_forgets_device() {
    let _guard = TEST_LOCK.lock().await;
    // 心跳提速：30s 协议间隔压缩到 200ms（仅本测试进程）
    unsafe { std::env::set_var("HAWK_REMOTE_HEARTBEAT_MS", "200") };
    let (base, hub) = spawn_mock_hub().await;
    let lib = test_library();
    lib.remote
        .update(&RemoteSettings {
            enabled: true,
            url: base,
            key: KEY.to_string(),
            quic_port: 0,
            writable: false,
        })
        .unwrap();

    let supervisor = Supervisor::new(
        hawk_daemon::core::events::EventBus::new(),
        lib.remote.clone(),
    );
    let task = tokio::spawn(supervisor.clone().run(
        lib.paths.clone(),
        lib.config.clone(),
        axum::Router::new(),
    ));

    wait_state(&supervisor, RemoteState::Online, Duration::from_secs(10)).await;
    assert_eq!(supervisor.snapshot().device_id.as_deref(), Some("d_1"));

    // 服务端设备表丢失 → 心跳 404 → 自动清除 device_id 重注册
    hub.lost_devices.store(true, Ordering::Relaxed);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while hub.register_count() < 2 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "重注册超时（register_count={}）",
            hub.register_count()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // 重注册后恢复在线，device_id 更新为新值（注册-404-再注册竞态下可能连续翻号，
    // 不能断言具体是 d_2——只需确认不再是 d_1）
    hub.lost_devices.store(false, Ordering::Relaxed);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let id = supervisor.snapshot().device_id;
        if id.is_some() && id.as_deref() != Some("d_1") {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "等待重注册后的 device_id 超时（当前 {:?}）",
            supervisor.snapshot()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    task.abort();
    unsafe { std::env::remove_var("HAWK_REMOTE_HEARTBEAT_MS") };
}

/// 配置变更热重启（改密钥/地址）必须能重新绑定 QUIC 端口：
/// 旧数据面若不随任务收尾，新任务会因端口仍被自己占用而绑定失败，
/// 状态停在 Error 直到进程重启（面板上即「QUIC 端口绑定失败: Address already in use」）
#[tokio::test]
async fn hot_restart_rebinds_quic_port() {
    let _guard = TEST_LOCK.lock().await;
    let (base, _hub) = spawn_mock_hub().await;
    // 固定端口：冲突只可能来自本进程自己上一轮的数据面
    let port = {
        let probe = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap().port()
    };
    let lib = test_library();
    let remote = |url: &str| RemoteSettings {
        enabled: true,
        url: url.to_string(),
        key: KEY.to_string(),
        quic_port: port,
        writable: false,
    };
    lib.remote.update(&remote(&base)).unwrap();

    let bus = hawk_daemon::core::events::EventBus::new();
    let supervisor = Supervisor::new(bus.clone(), lib.remote.clone());
    let task = tokio::spawn(supervisor.clone().run(
        lib.paths.clone(),
        lib.config.clone(),
        axum::Router::new(),
    ));
    wait_state(&supervisor, RemoteState::Online, Duration::from_secs(10)).await;

    // 热重启：改地址写法（末尾斜杠，normalize 后同一个服务端）→ watcher 语义（update_remote + wake）。
    // 端口不变，正是「新任务绑定自己刚占用的端口」这条回归路径
    let mut events = bus.subscribe();
    lib.remote.update(&remote(&format!("{base}/"))).unwrap();
    supervisor.wake();
    // 状态流：connecting → online；中途停到 error 就是本回归要防的绑定失败
    let mut seen_connecting = false;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.unwrap();
            match event.payload["state"].as_str() {
                Some("connecting") => seen_connecting = true,
                Some("error") => panic!("热重启失败: {}", event.payload["error"]),
                Some("online") if seen_connecting => return,
                _ => {}
            }
        }
    })
    .await
    .expect("热重启后未回到在线");

    // 停用：数据面随任务收尾，端口立即可被其他进程绑定
    lib.remote
        .update(&RemoteSettings {
            enabled: false,
            url: String::new(),
            key: String::new(),
            quic_port: port,
            writable: false,
        })
        .unwrap();
    supervisor.wake();
    wait_state(&supervisor, RemoteState::Disabled, Duration::from_secs(5)).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if std::net::UdpSocket::bind(("0.0.0.0", port)).is_ok() {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "停用后端口未释放");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    task.abort();
}

/// 服务端换了数据目录/重装（设备表丢失）→ 控制通道握手 404：
/// 必须按「设备不存在」重注册，而不是当成网络抖动无限重连——
/// 否则面板永远停在「接入中」，心跳也发不出去（心跳在通道内部）
#[tokio::test]
async fn ws_404_triggers_re_register() {
    let _guard = TEST_LOCK.lock().await;
    let (base, hub) = spawn_mock_hub().await;
    let lib = test_library();
    lib.remote
        .update(&RemoteSettings {
            enabled: true,
            url: base,
            key: KEY.to_string(),
            quic_port: 0,
            writable: false,
        })
        .unwrap();

    let supervisor = Supervisor::new(
        hawk_daemon::core::events::EventBus::new(),
        lib.remote.clone(),
    );
    let task = tokio::spawn(supervisor.clone().run(
        lib.paths.clone(),
        lib.config.clone(),
        axum::Router::new(),
    ));
    wait_state(&supervisor, RemoteState::Online, Duration::from_secs(10)).await;
    assert_eq!(supervisor.snapshot().device_id.as_deref(), Some("d_1"));
    assert_eq!(hub.register_count(), 1);

    // 服务端忘掉本设备：握手 404 + 现有通道被踢
    hub.lost_ws_devices.store(true, Ordering::Relaxed);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while hub.register_count() < 2 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "404 未触发重注册（register_count={}，状态 {:?}）",
            hub.register_count(),
            supervisor.snapshot()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // 恢复后重新上线，device_id 为新分配值
    hub.lost_ws_devices.store(false, Ordering::Relaxed);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = supervisor.snapshot();
        if snapshot.state == RemoteState::Online && snapshot.device_id.as_deref() != Some("d_1") {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "重注册后未恢复在线（当前 {snapshot:?}）"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    task.abort();
}

/// 目标设备未开启远程访问的 writable 时，写会话请求必须被拒绝而不是接受
#[tokio::test]
async fn write_session_rejected_when_writable_off() {
    let _guard = TEST_LOCK.lock().await;
    let (base, hub) = spawn_mock_hub().await;
    hub.set_write_request(true);
    let lib = test_library();
    lib.remote
        .update(&RemoteSettings {
            enabled: true,
            url: base,
            key: KEY.to_string(),
            quic_port: 0,
            writable: false,
        })
        .unwrap();

    let supervisor = Supervisor::new(
        hawk_daemon::core::events::EventBus::new(),
        lib.remote.clone(),
    );
    let task = tokio::spawn(supervisor.clone().run(
        lib.paths.clone(),
        lib.config.clone(),
        axum::Router::new(),
    ));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if !hub.rejected_sessions().is_empty() {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "写会话未被拒绝");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(hub.accepted_sessions().is_empty(), "写会话不应被接受");
    task.abort();
}

/// 开启远程访问的 writable 后写会话正常接受
#[tokio::test]
async fn write_session_accepted_when_writable_on() {
    let _guard = TEST_LOCK.lock().await;
    let (base, hub) = spawn_mock_hub().await;
    hub.set_write_request(true);
    let lib = test_library();
    lib.remote
        .update(&RemoteSettings {
            enabled: true,
            url: base,
            key: KEY.to_string(),
            quic_port: 0,
            writable: true,
        })
        .unwrap();

    let supervisor = Supervisor::new(
        hawk_daemon::core::events::EventBus::new(),
        lib.remote.clone(),
    );
    let task = tokio::spawn(supervisor.clone().run(
        lib.paths.clone(),
        lib.config.clone(),
        axum::Router::new(),
    ));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if !hub.accepted_sessions().is_empty() {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "写会话未被接受");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(hub.rejected_sessions().is_empty(), "写会话不应被拒绝");
    task.abort();
}

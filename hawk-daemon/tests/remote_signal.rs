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

/// mock 服务端的准入凭证（新模型下即 service-key）
const KEY: &str = "hs_test-service-key";

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
    /// session.request 携带的查看者凭证（sha256(key)）；None = app 间会话
    credential: Arc<Mutex<Option<String>>>,
    /// 模拟服务端设备表丢失：心跳一律 404
    lost_devices: Arc<std::sync::atomic::AtomicBool>,
    /// 模拟服务端设备表丢失（控制通道面）：WS 握手 404 并踢掉已连接通道
    lost_ws_devices: Arc<std::sync::atomic::AtomicBool>,
    /// 心跳限速（模拟服务端最小间隔）：上次心跳时间戳（毫秒）
    throttle: Arc<std::sync::atomic::AtomicBool>,
    last_heartbeat_ms: Arc<Mutex<u64>>,
    /// 踢掉当前控制通道一次（正常 Close；重连后自动复位）
    kick: Arc<std::sync::atomic::AtomicBool>,
    /// 控制通道连接计数（静默看门狗重连回归用）
    ws_connects: Arc<AtomicU64>,
    /// 「对端」设备的合法 endpoint_id（iroh 身份是 Ed25519 公钥，随机 hex 不一定是合法点——
    /// 无效身份会落入「接受但不放行」分支，执法断言全失效）
    peer_endpoint_id: String,
}

/// mock 心跳限速窗口（比协议的 10s 小，测试节奏用）
const MOCK_HEARTBEAT_MIN_MS: u64 = 1000;

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

    fn ws_connect_count(&self) -> u64 {
        self.ws_connects.load(Ordering::Relaxed)
    }
}

async fn spawn_mock_hub() -> (String, MockHub) {
    let hub = MockHub {
        peer_endpoint_id: iroh::SecretKey::generate().public().to_string(),
        ..MockHub::default()
    };
    let app = Router::new()
        .route("/api/v1/devices", post(register).get(devices))
        .route("/api/v1/devices/heartbeat", post(heartbeat))
        .route(
            "/api/v1/devices/{device_id}/keys",
            axum::routing::put(sync_keys),
        )
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
    assert!(body["endpoint_id"].as_str().unwrap().len() == 64);
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
    // 限速模拟：窗口内心跳 429（窗口内不收计数——与真实服务端一致）
    if hub.throttle.load(Ordering::Relaxed) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut last = hub.last_heartbeat_ms.lock().unwrap();
        if now.saturating_sub(*last) < MOCK_HEARTBEAT_MIN_MS {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({"status":"error","error":{"message":"心跳过于频繁"}})),
            )
                .into_response();
        }
        *last = now;
    }
    hub.heartbeats.fetch_add(1, Ordering::Relaxed);
    Json(json!({"status":"success","data":{"server_time_ms":0,"public_ip_seen":"127.0.0.1"}}))
        .into_response()
}

async fn devices(headers: axum::http::HeaderMap) -> Response {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let endpoint_id = "11".repeat(32); // 列表展示用（不走拨号，无需合法公钥）
    Json(json!({"status":"success","data":{"devices":[
        {"device_id":"d_other","name":"对方","platform":"macos","library_name":"库",
         "online":true,"allow_remote":true,"allow_write":true,"endpoint_id":endpoint_id}
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

/// mock 的 key 映射同步端点：收下即成功（只验证鉴权）
async fn sync_keys(headers: axum::http::HeaderMap) -> Response {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(json!({"status":"success","data":{"synced":0}})).into_response()
}

async fn ws_channel(
    State(hub): State<MockHub>,
    headers: axum::http::HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Response {
    // 鉴权同 REST：Authorization 头（新协议 key 不走 URL）
    if !authorized(&headers) {
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
    hub.ws_connects.fetch_add(1, Ordering::Relaxed);
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
            "from": {"device_id":"d_other","name":"对方","library_name":"库","endpoint_id": hub.peer_endpoint_id},
            "mode": "direct",
            "access": if hub.write_request.load(Ordering::Relaxed) { "write" } else { "read" },
            "credential": *hub.credential.lock().unwrap(),
        });
        socket
            .send(Message::Text(request.to_string().into()))
            .await
            .unwrap();
        // 持有连接直至对端关闭（或模拟服务端丢设备/踢下线时主动断开）
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(20)) => {
                    if hub.lost_ws_devices.load(Ordering::Relaxed) {
                        break;
                    }
                    // 踢下线一次：正常 Close（重连后标志已复位，不再踢）
                    if hub.kick.swap(false, Ordering::Relaxed) {
                        let _ = socket.send(Message::Close(None)).await;
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
        Some(&paths),
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
            service_key: KEY.to_string(),
            quic_port: 0, // OS 分配，避免测试间端口冲突
            writable: false,
            relay: false,
        })
        .unwrap();

    let supervisor = Supervisor::new(
        hawk_daemon::core::events::EventBus::new(),
        lib.remote.clone(),
    );
    let task = tokio::spawn(supervisor.clone().run(
        Some(hawk_daemon::remote::LibraryContext {
            paths: lib.paths.clone(),
            config: lib.config.clone(),
        }),
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
            service_key: "wrong-key".to_string(),
            quic_port: 0,
            writable: false,
            relay: false,
        })
        .unwrap();

    let supervisor = Supervisor::new(
        hawk_daemon::core::events::EventBus::new(),
        lib.remote.clone(),
    );
    let task = tokio::spawn(supervisor.clone().run(
        Some(hawk_daemon::remote::LibraryContext {
            paths: lib.paths.clone(),
            config: lib.config.clone(),
        }),
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
            service_key: KEY.to_string(),
            quic_port: 0,
            writable: false,
            relay: false,
        })
        .unwrap();

    let supervisor = Supervisor::new(
        hawk_daemon::core::events::EventBus::new(),
        lib.remote.clone(),
    );
    let task = tokio::spawn(supervisor.clone().run(
        Some(hawk_daemon::remote::LibraryContext {
            paths: lib.paths.clone(),
            config: lib.config.clone(),
        }),
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
        service_key: KEY.to_string(),
        quic_port: port,
        writable: false,
        relay: false,
    };
    lib.remote.update(&remote(&base)).unwrap();

    let bus = hawk_daemon::core::events::EventBus::new();
    let supervisor = Supervisor::new(bus.clone(), lib.remote.clone());
    let task = tokio::spawn(supervisor.clone().run(
        Some(hawk_daemon::remote::LibraryContext {
            paths: lib.paths.clone(),
            config: lib.config.clone(),
        }),
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
            service_key: String::new(),
            quic_port: port,
            writable: false,
            relay: false,
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
            service_key: KEY.to_string(),
            quic_port: 0,
            writable: false,
            relay: false,
        })
        .unwrap();

    let supervisor = Supervisor::new(
        hawk_daemon::core::events::EventBus::new(),
        lib.remote.clone(),
    );
    let task = tokio::spawn(supervisor.clone().run(
        Some(hawk_daemon::remote::LibraryContext {
            paths: lib.paths.clone(),
            config: lib.config.clone(),
        }),
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

/// 查看者写会话：本地执法三重裁决——无凭证拒、key 无写权限拒、本机 writable 关闭拒
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
            service_key: KEY.to_string(),
            quic_port: 0,
            writable: false,
            relay: false,
        })
        .unwrap();

    let supervisor = Supervisor::new(
        hawk_daemon::core::events::EventBus::new(),
        lib.remote.clone(),
    );
    // 签发一把带写权限的 key，mock 携带其验证子（writable=false 仍应拒绝）
    let (entry, _) = supervisor.keys.create("插件", true, true, None).unwrap();
    *hub.credential.lock().unwrap() = Some(entry.hash.clone());
    let task = tokio::spawn(supervisor.clone().run(
        Some(hawk_daemon::remote::LibraryContext {
            paths: lib.paths.clone(),
            config: lib.config.clone(),
        }),
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

/// 写会话不携带查看者凭证（app 间语义）→ 一律拒绝（写只对 key 持有者开放）
#[tokio::test]
async fn write_session_rejected_without_credential() {
    let _guard = TEST_LOCK.lock().await;
    let (base, hub) = spawn_mock_hub().await;
    hub.set_write_request(true);
    let lib = test_library();
    lib.remote
        .update(&RemoteSettings {
            enabled: true,
            url: base,
            service_key: KEY.to_string(),
            quic_port: 0,
            writable: true,
            relay: false,
        })
        .unwrap();

    let supervisor = Supervisor::new(
        hawk_daemon::core::events::EventBus::new(),
        lib.remote.clone(),
    );
    let task = tokio::spawn(supervisor.clone().run(
        Some(hawk_daemon::remote::LibraryContext {
            paths: lib.paths.clone(),
            config: lib.config.clone(),
        }),
        axum::Router::new(),
    ));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if !hub.rejected_sessions().is_empty() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "无凭证写会话未被拒绝"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(hub.accepted_sessions().is_empty(), "无凭证写会话不应被接受");
    task.abort();
}

/// 回归：踢线快速重连后的即时心跳落在服务端限速窗口内（429）——
/// 429 必须只跳过本跳，不得拆通道（否则会多绕一轮「接入中」，
/// 且重连-429-拆通道会形成自伤循环）
#[tokio::test]
async fn heartbeat_429_does_not_break_channel() {
    let _guard = TEST_LOCK.lock().await;
    // 心跳提速到 200ms：让重连后的即时心跳必然落在 mock 的 1s 限速窗口内
    unsafe { std::env::set_var("HAWK_REMOTE_HEARTBEAT_MS", "200") };
    let (base, hub) = spawn_mock_hub().await;
    hub.throttle.store(true, Ordering::Relaxed);
    let lib = test_library();
    lib.remote
        .update(&RemoteSettings {
            enabled: true,
            url: base,
            service_key: KEY.to_string(),
            quic_port: 0,
            writable: false,
            relay: false,
        })
        .unwrap();

    let bus = hawk_daemon::core::events::EventBus::new();
    let mut events = bus.subscribe();
    let supervisor = Supervisor::new(bus, lib.remote.clone());
    let task = tokio::spawn(supervisor.clone().run(
        Some(hawk_daemon::remote::LibraryContext {
            paths: lib.paths.clone(),
            config: lib.config.clone(),
        }),
        axum::Router::new(),
    ));
    wait_state(&supervisor, RemoteState::Online, Duration::from_secs(10)).await;
    let initial = hub.heartbeat_count();
    // 排空启动阶段的 connecting/online 事件（只观察 kick 之后的状态流）
    while events.try_recv().is_ok() {}

    // 服务端正常关闭通道（kick 一次）→ 客户端应立即重连（不闪「接入中」），
    // 重连后的即时心跳被限速 429 → 通道保持在线
    hub.kick.store(true, Ordering::Relaxed);

    // 收集 3s 内的信令状态事件：不应出现 connecting
    let mut saw_connecting = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Ok(event)) = tokio::time::timeout(Duration::from_millis(100), events.recv()).await
        {
            if event.payload["state"].as_str() == Some("connecting") {
                saw_connecting = true;
            }
        }
    }
    assert!(
        !saw_connecting,
        "重连后不应出现「接入中」（心跳 429 拆了通道）"
    );
    assert_eq!(supervisor.snapshot().state, RemoteState::Online);
    assert_eq!(hub.register_count(), 1, "不应触发重注册");

    // 限速窗口过后心跳恢复
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while hub.heartbeat_count() <= initial {
        assert!(tokio::time::Instant::now() < deadline, "心跳未恢复");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    task.abort();
    unsafe { std::env::remove_var("HAWK_REMOTE_HEARTBEAT_MS") };
}

/// 查看者会话携带已吊销/未知 key 的验证子 → 拒绝（本地执法不认 server 单方面声称）
#[tokio::test]
async fn viewer_session_rejected_with_unknown_credential() {
    let _guard = TEST_LOCK.lock().await;
    let (base, hub) = spawn_mock_hub().await;
    // 只读请求，但凭证本地查无（等效吊销后重放）
    *hub.credential.lock().unwrap() = Some(hawk_daemon::remote::keys::sha256_hex("revoked-key"));
    let lib = test_library();
    lib.remote
        .update(&RemoteSettings {
            enabled: true,
            url: base,
            service_key: KEY.to_string(),
            quic_port: 0,
            writable: true,
            relay: false,
        })
        .unwrap();

    let supervisor = Supervisor::new(
        hawk_daemon::core::events::EventBus::new(),
        lib.remote.clone(),
    );
    let task = tokio::spawn(supervisor.clone().run(
        Some(hawk_daemon::remote::LibraryContext {
            paths: lib.paths.clone(),
            config: lib.config.clone(),
        }),
        axum::Router::new(),
    ));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if !hub.rejected_sessions().is_empty() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "未知凭证会话未被拒绝"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(hub.accepted_sessions().is_empty());
    task.abort();
}

/// 开启 writable 且 key 有写权限后写会话正常接受
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
            service_key: KEY.to_string(),
            quic_port: 0,
            writable: true,
            relay: false,
        })
        .unwrap();

    let supervisor = Supervisor::new(
        hawk_daemon::core::events::EventBus::new(),
        lib.remote.clone(),
    );
    // 签发带写权限的 key，mock 携带其验证子
    let (entry, _) = supervisor.keys.create("插件", true, true, None).unwrap();
    *hub.credential.lock().unwrap() = Some(entry.hash.clone());
    let task = tokio::spawn(supervisor.clone().run(
        Some(hawk_daemon::remote::LibraryContext {
            paths: lib.paths.clone(),
            config: lib.config.clone(),
        }),
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

/// 半开连接回归：反代切换/NAT 会话老化可致通道下行静默死亡（上行 REST 心跳照常），
/// 服务端的保活 ping 与推送全都到不了客户端。daemon 必须以「收不到服务端帧」为判据
/// 主动拆通道重连——否则设备永远假在线，session.request 全丢（会话永远 pending）。
/// mock 从不在通道上发 ping，通道必然被看门狗判定静默。
#[tokio::test]
async fn silent_channel_triggers_reconnect() {
    let _guard = TEST_LOCK.lock().await;
    // 看门狗提速：90s 协议值压缩到 400ms（仅本测试进程）
    unsafe { std::env::set_var("HAWK_REMOTE_CHANNEL_SILENCE_MS", "400") };
    let (base, hub) = spawn_mock_hub().await;
    let lib = test_library();
    lib.remote
        .update(&RemoteSettings {
            enabled: true,
            url: base,
            service_key: KEY.to_string(),
            quic_port: 0,
            writable: false,
            relay: false,
        })
        .unwrap();

    let supervisor = Supervisor::new(
        hawk_daemon::core::events::EventBus::new(),
        lib.remote.clone(),
    );
    let task = tokio::spawn(supervisor.clone().run(
        Some(hawk_daemon::remote::LibraryContext {
            paths: lib.paths.clone(),
            config: lib.config.clone(),
        }),
        axum::Router::new(),
    ));
    wait_state(&supervisor, RemoteState::Online, Duration::from_secs(10)).await;
    let first = hub.ws_connect_count();
    assert!(first >= 1, "控制通道未建立");

    // 通道静默 → 看门狗触发 → 主动断开并重连（连接计数增长）
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while hub.ws_connect_count() <= first {
        assert!(
            tokio::time::Instant::now() < deadline,
            "静默通道未触发重连（ws_connect_count={}）",
            hub.ws_connect_count()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    task.abort();
    unsafe { std::env::remove_var("HAWK_REMOTE_CHANNEL_SILENCE_MS") };
}

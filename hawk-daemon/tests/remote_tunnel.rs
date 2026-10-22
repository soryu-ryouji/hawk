//! 数据面端到端测试：QUIC 隧道（指纹钉扎）+ A 侧进程内转发 + B 侧会话代理。
//! 不依赖 hawk-server（直连数据面自洽）；会话编排链路另起 mock hub 全流程验证。

use axum::extract::Request;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use hawk_daemon::core::events::EventBus;
use hawk_daemon::remote::data::{serve_proxy, DataPlane, ProxyContext, TunneledPeer};
use hawk_daemon::remote::identity::Identity;
use hawk_daemon::remote::session::SessionManager;
use hawk_daemon::remote::tls;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn new_identity(dir: &tempfile::TempDir, device_id: Option<&str>) -> Identity {
    let remote_dir = dir.path().join("remote");
    let mut identity = Identity::load_or_create(remote_dir.to_str().unwrap()).unwrap();
    identity.device_id = device_id.map(String::from);
    identity
}

fn fingerprint_hex(identity: &Identity) -> String {
    identity.fingerprint.clone()
}

/// 测试路由：/hello 校验 TunneledPeer 标记与写权限（进程内受托语义）；
/// /api/v1/app/token 若被隧道放行则返回 200（测试断言它被隧道端 404 拦截、永远到不了路由）
fn test_router() -> Router {
    Router::new()
        .route(
            "/hello",
            get(|req: Request| async move {
                match req.extensions().get::<TunneledPeer>() {
                    Some(peer) if peer.writable => "write from A",
                    Some(_) => "hi from A",
                    None => "missing marker",
                }
            }),
        )
        .fallback(|_req: Request| async move { Response::new("reached router".to_string()) })
}

async fn free_proxy(ctx: Arc<ProxyContext>) -> u16 {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(serve_proxy(ctx, listener));
    port
}

#[tokio::test]
async fn tunnel_carries_http_with_viewer_marker() {
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let identity_a = new_identity(&dir_a, Some("d_a"));
    let identity_b = new_identity(&dir_b, Some("d_b"));
    let fp_a = tls::parse_fingerprint(&identity_a.fingerprint).unwrap();
    let fp_b = tls::parse_fingerprint(&identity_b.fingerprint).unwrap();

    // A 侧：数据面 + 放行 B 的指纹
    let plane_a = DataPlane::bind(&identity_a, 0, test_router()).unwrap();
    let port_a = plane_a.local_addr().unwrap().port();
    plane_a.allow_session("s_tunnel", fp_b, false);

    // B 侧：钉扎 A 的指纹连接
    let (_endpoint, conn) = DataPlane::connect(&identity_b, "127.0.0.1", port_a, fp_a)
        .await
        .expect("隧道连接");

    // B 侧会话代理
    let ctx = Arc::new(ProxyContext {
        conn,
        token: "session-token".to_string(),
        session_id: "s_tunnel".to_string(),
    });
    let proxy_port = free_proxy(ctx).await;

    let client = reqwest::Client::new();
    // 正向：代理 → 隧道 → A 路由（携带 viewer 标记）
    let resp = client
        .get(format!("http://127.0.0.1:{proxy_port}/hello"))
        .bearer_auth("session-token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "hi from A");

    // 查询参数 token 同样放行（EventSource/<img> 场景）
    let resp = client
        .get(format!(
            "http://127.0.0.1:{proxy_port}/hello?token=session-token"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // 无 token → 代理门禁 401
    let resp = client
        .get(format!("http://127.0.0.1:{proxy_port}/hello"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // 隧道端拒绝转发 token 发现端点（协议安全模型）
    let resp = client
        .get(format!(
            "http://127.0.0.1:{proxy_port}/api/v1/app/token?token=session-token"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn await_first_conn_times_out_and_succeeds() {
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let identity_a = new_identity(&dir_a, Some("d_a"));
    let identity_b = new_identity(&dir_b, Some("d_b"));
    let fp_a = tls::parse_fingerprint(&identity_a.fingerprint).unwrap();
    let fp_b = tls::parse_fingerprint(&identity_b.fingerprint).unwrap();

    let plane_a = DataPlane::bind(&identity_a, 0, test_router()).unwrap();
    let port_a = plane_a.local_addr().unwrap().port();

    // 放行但无请求：超时返回 false（接受会话后的清理路径）
    plane_a.allow_session("s_first", fp_b, false);
    assert!(
        !plane_a
            .await_first_conn("s_first", Duration::from_millis(100))
            .await
    );

    // 对端连接并发出首个隧道请求：等待立即返回 true
    let (_endpoint, conn) = DataPlane::connect(&identity_b, "127.0.0.1", port_a, fp_a)
        .await
        .expect("隧道连接");
    let ctx = Arc::new(ProxyContext {
        conn,
        token: "session-token".to_string(),
        session_id: "s_first".to_string(),
    });
    let proxy_port = free_proxy(ctx).await;
    let waiter = {
        let plane = plane_a.clone();
        tokio::spawn(async move {
            plane
                .await_first_conn("s_first", Duration::from_secs(2))
                .await
        })
    };
    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{proxy_port}/hello"))
        .bearer_auth("session-token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(waiter.await.unwrap());
}

/// 同一设备指纹承载多个会话：取消一个只关闭它的连接，另一个继续可用（hawk-server 网页版 + 插件）
#[tokio::test]
async fn sessions_with_same_fingerprint_are_isolated() {
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let identity_a = new_identity(&dir_a, Some("d_a"));
    let identity_b = new_identity(&dir_b, Some("d_b"));
    let fp_a = tls::parse_fingerprint(&identity_a.fingerprint).unwrap();
    let fp_b = tls::parse_fingerprint(&identity_b.fingerprint).unwrap();

    let plane_a = DataPlane::bind(&identity_a, 0, test_router()).unwrap();
    let port_a = plane_a.local_addr().unwrap().port();
    plane_a.allow_session("s_a", fp_b, false);
    plane_a.allow_session("s_b", fp_b, false);

    let (_endpoint_a, conn_a) = DataPlane::connect(&identity_b, "127.0.0.1", port_a, fp_a)
        .await
        .expect("会话 A 隧道连接");
    let (_endpoint_b, conn_b) = DataPlane::connect(&identity_b, "127.0.0.1", port_a, fp_a)
        .await
        .expect("会话 B 隧道连接");
    let proxy_a = free_proxy(Arc::new(ProxyContext {
        conn: conn_a,
        token: "token-a".to_string(),
        session_id: "s_a".to_string(),
    }))
    .await;
    let proxy_b = free_proxy(Arc::new(ProxyContext {
        conn: conn_b,
        token: "token-b".to_string(),
        session_id: "s_b".to_string(),
    }))
    .await;

    let client = reqwest::Client::new();
    for (port, token) in [(proxy_a, "token-a"), (proxy_b, "token-b")] {
        let resp = client
            .get(format!("http://127.0.0.1:{port}/hello"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    // 取消 s_a：s_a 连接关闭，s_b 不受影响
    plane_a.disallow_session("s_a");
    let resp = client
        .get(format!("http://127.0.0.1:{proxy_a}/hello"))
        .bearer_auth("token-a")
        .send()
        .await
        .unwrap();
    assert_ne!(resp.status(), 200, "取消的会话不应继续可用");
    let resp = client
        .get(format!("http://127.0.0.1:{proxy_b}/hello"))
        .bearer_auth("token-b")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "其他会话被误伤");

    // 指纹仍被 s_b 引用：新连接仍可完成 QUIC 握手
    let (_endpoint_c, conn_c) = DataPlane::connect(&identity_b, "127.0.0.1", port_a, fp_a)
        .await
        .expect("s_b 存续期间指纹应保持放行");
    drop(conn_c);

    plane_a.disallow_session("s_b");
}

#[tokio::test]
async fn tunnel_rejects_wrong_fingerprint() {
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let identity_a = new_identity(&dir_a, Some("d_a"));
    let identity_b = new_identity(&dir_b, Some("d_b"));
    let fp_b = tls::parse_fingerprint(&identity_b.fingerprint).unwrap();

    let plane_a = DataPlane::bind(&identity_a, 0, test_router()).unwrap();
    let port_a = plane_a.local_addr().unwrap().port();
    plane_a.allow_session("s_wrong", fp_b, false);

    // B 钉扎一个错误指纹：握手必须失败
    let wrong = tls::fingerprint_of(b"not-a");
    let result = DataPlane::connect(&identity_b, "127.0.0.1", port_a, wrong).await;
    assert!(result.is_err(), "错误指纹不应建立隧道");
}

// ---------- 会话编排全流程（mock hub） ----------

use axum::Json;

#[derive(Default)]
struct HubState {
    cancelled: Mutex<Vec<String>>,
}

/// mock hub：devices（含 A 的指纹）+ sessions（发起即 ready，候选指向 A 侧数据面）
async fn spawn_hub(port_a: u16, fp_a_hex: String) -> (String, Arc<HubState>) {
    let hub = Arc::new(HubState::default());
    let fp_devices = fp_a_hex.clone();
    let fp_ready = fp_a_hex.clone();
    let app = Router::new()
        .route(
            "/api/v1/devices",
            axum::routing::get(move || {
                let fp = fp_devices.clone();
                async move {
                    Json(json!({"status":"success","data":{"devices":[
                        {"device_id":"d_a","name":"家里台式机","platform":"macos","library_name":"A 的库",
                         "online":true,"allow_remote":true,"fingerprint":fp},
                        {"device_id":"d_b","name":"笔记本","platform":"macos","library_name":"B 的库",
                         "online":true,"allow_remote":true,"fingerprint":"00"}
                    ]}}))
                }
            }),
        )
        .route(
            "/api/v1/sessions",
            axum::routing::post(|Json(_body): Json<Value>| async move {
                Json(json!({"status":"success","data":{"session_id":"s_1","state":"pending"}}))
            }),
        )
        .route(
            "/api/v1/sessions/s_1",
            axum::routing::get(move || async move {
                Json(json!({"status":"success","data":{
                    "session_id":"s_1","state":"ready",
                    "connect":{
                        "mode":"direct",
                        "candidates":[{"transport":"ipv4","host":"127.0.0.1","port":port_a}],
                        "fingerprint": fp_ready,
                    }
                }}))
            }),
        )
        .route(
            "/api/v1/sessions/s_1/cancel",
            axum::routing::post({
                let hub = hub.clone();
                move || {
                    let hub = hub.clone();
                    async move {
                        hub.cancelled.lock().unwrap().push("s_1".to_string());
                        Json(json!({"status":"success"}))
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), hub)
}

#[tokio::test]
async fn relay_session_end_to_end() {
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let identity_a = new_identity(&dir_a, Some("d_a"));
    let identity_b = new_identity(&dir_b, Some("d_b"));
    let fp_b = tls::parse_fingerprint(&identity_b.fingerprint).unwrap();

    // A 侧：放行 B 的指纹与会话（真实流程由 signal 的 session.request 驱动）
    let plane_a = DataPlane::bind(&identity_a, 0, test_router()).unwrap();
    plane_a.allow_session("s_r", fp_b, false);

    // mock hub：relay 会话即 ready + WS 配对转发
    let (base, _pairs) = spawn_relay_hub(fingerprint_hex(&identity_a)).await;

    // A 侧连中继 WS（role=a）并在其上接受 QUIC
    let ws_req_a =
        hawk_daemon::remote::relay::relay_ws_request(&base, "key", "d_a", "s_r", "a").unwrap();
    let (ws_a, _) = tokio_tungstenite::connect_async(ws_req_a).await.unwrap();
    let serve = {
        let identity_a = identity_a.clone();
        let plane_a = plane_a.clone();
        tokio::spawn(async move {
            let _ = hawk_daemon::remote::relay::serve_over_ws(
                &identity_a,
                &plane_a,
                ws_a,
                test_router(),
            )
            .await;
        })
    };

    // B 侧：经 SessionManager 以 relay 模式建立会话
    let http = reqwest::Client::new();
    let manager = SessionManager::new(EventBus::new());
    let dto = manager
        .connect(&http, &base, "key", &identity_b, "d_a", "relay")
        .await
        .expect("中继会话建立");

    // 经会话代理访问 A 的素材服务（QUIC 全程跑在中继字节流上）
    let resp = http
        .get(format!("http://127.0.0.1:{}/hello", dto.proxy_port))
        .bearer_auth(&dto.token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "hi from A");

    manager.disconnect(&http, &base, "key").await;
    serve.abort();
}

// ---------- 中继 mock hub（relay 会话 + WS 配对转发） ----------

type RelayPairs = Arc<
    Mutex<
        HashMap<(String, String), tokio::sync::mpsc::UnboundedSender<axum::extract::ws::Message>>,
    >,
>;

async fn spawn_relay_hub(fp_a_hex: String) -> (String, RelayPairs) {
    use axum::extract::ws::Message;
    use axum::extract::{Query, WebSocketUpgrade};
    use futures_util::{SinkExt, StreamExt};

    let pairs: RelayPairs = Arc::new(Mutex::new(HashMap::new()));
    let pairs_ws = pairs.clone();
    let fp_devices = fp_a_hex.clone();
    let fp_ready = fp_a_hex.clone();
    let app = Router::new()
        .route(
            "/api/v1/devices",
            axum::routing::get(move || {
                let fp = fp_devices.clone();
                async move {
                    Json(json!({"status":"success","data":{"devices":[
                        {"device_id":"d_a","name":"家里台式机","platform":"macos","library_name":"A 的库",
                         "online":true,"allow_remote":true,"fingerprint":fp}
                    ]}}))
                }
            }),
        )
        .route(
            "/api/v1/sessions",
            axum::routing::post(|Json(_body): Json<Value>| async move {
                Json(json!({"status":"success","data":{"session_id":"s_r","state":"pending"}}))
            }),
        )
        .route(
            "/api/v1/sessions/s_r",
            axum::routing::get(move || {
                let fp = fp_ready.clone();
                async move {
                    Json(json!({"status":"success","data":{
                        "session_id":"s_r","state":"ready",
                        "connect":{ "mode":"relay", "fingerprint": fp }
                    }}))
                }
            }),
        )
        .route(
            "/api/v1/sessions/s_r/cancel",
            axum::routing::post(|| async move { Json(json!({"status":"success"})) }),
        )
        .route(
            "/api/v1/relay",
            axum::routing::get(
                move |Query(q): Query<std::collections::HashMap<String, String>>,
                      ws: WebSocketUpgrade| {
                    let pairs = pairs_ws.clone();
                    let session = q.get("session_id").cloned().unwrap_or_default();
                    let role = q.get("role").cloned().unwrap_or_default();
                    async move {
                    ws.on_upgrade(move |socket| async move {
                        let (tx, mut rx) =
                            tokio::sync::mpsc::unbounded_channel::<Message>();
                        pairs
                            .lock()
                            .unwrap()
                            .insert((session.clone(), role.clone()), tx);
                        let peer_role = if role == "a" { "b" } else { "a" }.to_string();
                        let (mut sink, mut stream) = socket.split();
                        loop {
                            tokio::select! {
                                msg = rx.recv() => {
                                    let Some(msg) = msg else { break };
                                    if sink.send(msg).await.is_err() { break; }
                                }
                                incoming = stream.next() => {
                                    match incoming {
                                        Some(Ok(Message::Binary(data))) => {
                                            let peer = pairs
                                                .lock()
                                                .unwrap()
                                                .get(&(session.clone(), peer_role.clone()))
                                                .cloned();
                                            if let Some(peer) = peer {
                                                let _ = peer.send(Message::Binary(data));
                                            }
                                        }
                                        _ => break,
                                    }
                                }
                            }
                        }
                        })
                    }
                },
            ),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), pairs)
}

/// 打洞会话 mock hub：ready 候选为 punch（指向 A 侧数据面），携带 punch_peer（B 忽略）
async fn spawn_punch_hub(port_a: u16, fp_a_hex: String) -> String {
    let fp_devices = fp_a_hex.clone();
    let fp_ready = fp_a_hex.clone();
    let app = Router::new()
        .route(
            "/api/v1/devices",
            axum::routing::get(move || {
                let fp = fp_devices.clone();
                async move {
                    Json(json!({"status":"success","data":{"devices":[
                        {"device_id":"d_a","name":"家里台式机","platform":"macos","library_name":"A 的库",
                         "online":true,"allow_remote":true,"fingerprint":fp}
                    ]}}))
                }
            }),
        )
        .route(
            "/api/v1/sessions",
            axum::routing::post(|Json(_body): Json<Value>| async move {
                Json(json!({"status":"success","data":{"session_id":"s_p","state":"pending"}}))
            }),
        )
        .route(
            "/api/v1/sessions/s_p",
            axum::routing::get(move || {
                let fp = fp_ready.clone();
                async move {
                    Json(json!({"status":"success","data":{
                        "session_id":"s_p","state":"ready",
                        "connect":{
                            "mode":"direct",
                            "candidates":[{"transport":"punch","host":"127.0.0.1","port":port_a}],
                            "punch_peer":{"host":"127.0.0.1","port":27399},
                            "fingerprint": fp,
                        }
                    }}))
                }
            }),
        )
        .route(
            "/api/v1/sessions/s_p/cancel",
            axum::routing::post(|| async move { Json(json!({"status":"success"})) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

/// 探测口收包记录（源地址, 包原文）
type ObservedPackets = Arc<Mutex<Vec<(SocketAddr, Vec<u8>)>>>;

/// P3 打洞 B 侧端到端：session.probe 预绑报到 → punch 候选复用同 socket 建连。
/// loopback 无 NAT，验证的是编排链路与 socket 复用路径（真 NAT 靠手动验证）
#[tokio::test]
async fn punch_session_end_to_end() {
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let identity_a = new_identity(&dir_a, Some("d_a"));
    let identity_b = new_identity(&dir_b, Some("d_b"));
    let fp_b = tls::parse_fingerprint(&identity_b.fingerprint).unwrap();

    let plane_a = DataPlane::bind(&identity_a, 0, test_router()).unwrap();
    let port_a = plane_a.local_addr().unwrap().port();
    plane_a.allow_session("s_p", fp_b, false);

    // 探测口 sink：验证 prepare_punch 的报到包形状（协议 P3：定长 64B = magic + token）
    let probe_sink = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let probe_port = probe_sink.local_addr().unwrap().port();
    let observed: ObservedPackets = Arc::new(Mutex::new(vec![]));
    let observed_sink = observed.clone();
    tokio::spawn(async move {
        let mut buf = [0u8; 128];
        while let Ok((len, from)) = probe_sink.recv_from(&mut buf).await {
            observed_sink
                .lock()
                .unwrap()
                .push((from, buf[..len].to_vec()));
        }
    });

    let base = spawn_punch_hub(port_a, fingerprint_hex(&identity_a)).await;
    let http = reqwest::Client::new();
    let manager = SessionManager::new(EventBus::new());

    // 模拟 WSS session.probe 触发（协议 P3）：B 预绑 socket 并向探测口 3 连发报到
    let token = [7u8; 16];
    manager.prepare_punch("s_p", SocketAddr::from(([127, 0, 0, 1], probe_port)), token);

    let dto = manager
        .connect(&http, &base, "key", &identity_b, "d_a", "direct")
        .await
        .expect("打洞会话建立");

    // 隧道功能与直连一致：经会话代理访问 A 的素材服务
    let resp = http
        .get(format!("http://127.0.0.1:{}/hello", dto.proxy_port))
        .bearer_auth(&dto.token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "hi from A");

    // 探测报到：3 连发、同一源端口（同 socket 复用）、包形状符合协议
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let count = observed.lock().unwrap().len();
        if count >= 3 {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "探测报到超时");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let packets = observed.lock().unwrap().clone();
    for (from, packet) in &packets {
        assert_eq!(packet.len(), 64);
        assert_eq!(&packet[..5], b"HAWKP");
        assert_eq!(&packet[5..21], &token);
        assert_eq!(*from, packets[0].0, "三次报到须同一源端口");
    }

    manager.disconnect(&http, &base, "key").await;
}

/// 竞速 mock hub：首候选为黑洞地址（192.0.2.1 TEST-NET-1，串行尝试要等满 5s 超时）
async fn spawn_race_hub(port_a: u16, fp_a_hex: String) -> String {
    let fp_devices = fp_a_hex.clone();
    let fp_ready = fp_a_hex.clone();
    let app = Router::new()
        .route(
            "/api/v1/devices",
            axum::routing::get(move || {
                let fp = fp_devices.clone();
                async move {
                    Json(json!({"status":"success","data":{"devices":[
                        {"device_id":"d_a","name":"家里台式机","platform":"macos","library_name":"A 的库",
                         "online":true,"allow_remote":true,"fingerprint":fp}
                    ]}}))
                }
            }),
        )
        .route(
            "/api/v1/sessions",
            axum::routing::post(|Json(_body): Json<Value>| async move {
                Json(json!({"status":"success","data":{"session_id":"s_race","state":"pending"}}))
            }),
        )
        .route(
            "/api/v1/sessions/s_race",
            axum::routing::get(move || {
                let fp = fp_ready.clone();
                async move {
                    Json(json!({"status":"success","data":{
                        "session_id":"s_race","state":"ready",
                        "connect":{
                            "mode":"direct",
                            "candidates":[
                                {"transport":"ipv4","host":"192.0.2.1","port":1},
                                {"transport":"ipv4","host":"127.0.0.1","port":port_a}
                            ],
                            "fingerprint": fp,
                        }
                    }}))
                }
            }),
        )
        .route(
            "/api/v1/sessions/s_race/cancel",
            axum::routing::post(|| async move { Json(json!({"status":"success"})) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://127.0.0.1:{addr}")
}

/// Happy Eyeballs 竞速：黑洞候选在前不阻塞可用候选。串行尝试此场景必然 ≥5s（黑洞候选
/// 超时）；并行竞速下可用候选即时胜出，总耗时应远小于串行下限
#[tokio::test]
async fn candidates_race_skips_blackhole() {
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let identity_a = new_identity(&dir_a, Some("d_a"));
    let identity_b = new_identity(&dir_b, Some("d_b"));
    let fp_b = tls::parse_fingerprint(&identity_b.fingerprint).unwrap();

    let plane_a = DataPlane::bind(&identity_a, 0, test_router()).unwrap();
    let port_a = plane_a.local_addr().unwrap().port();
    plane_a.allow_session("s_race", fp_b, false);

    let base = spawn_race_hub(port_a, fingerprint_hex(&identity_a)).await;
    let http = reqwest::Client::new();
    let manager = SessionManager::new(EventBus::new());

    let started = tokio::time::Instant::now();
    let dto = manager
        .connect(&http, &base, "key", &identity_b, "d_a", "direct")
        .await
        .expect("竞速会话建立");
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(4),
        "竞速应跳过黑洞候选（串行下限 5s），实际 {elapsed:?}"
    );

    // 胜者隧道功能完整
    let resp = http
        .get(format!("http://127.0.0.1:{}/hello", dto.proxy_port))
        .bearer_auth(&dto.token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    manager.disconnect(&http, &base, "key").await;
}

#[tokio::test]
async fn session_orchestration_end_to_end() {
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let identity_a = new_identity(&dir_a, Some("d_a"));
    let identity_b = new_identity(&dir_b, Some("d_b"));
    let fp_b = tls::parse_fingerprint(&identity_b.fingerprint).unwrap();

    // A 侧数据面就绪并放行 B 与 mock 会话（真实流程由 signal 的 session.request 驱动，此处直接裁决）
    let plane_a = DataPlane::bind(&identity_a, 0, test_router()).unwrap();
    let port_a = plane_a.local_addr().unwrap().port();
    plane_a.allow_session("s_1", fp_b, false);

    let (base, _hub) = spawn_hub(port_a, fingerprint_hex(&identity_a)).await;
    let http = reqwest::Client::new();
    let manager = SessionManager::new(EventBus::new());

    let dto = manager
        .connect(&http, &base, "key", &identity_b, "d_a", "direct")
        .await
        .expect("会话建立");

    assert_eq!(dto.target_name, "家里台式机");
    assert_eq!(dto.target_library_name, "A 的库");
    assert!(manager.snapshot().is_some());

    // 经会话代理访问 A 的素材服务
    let resp = http
        .get(format!("http://127.0.0.1:{}/hello", dto.proxy_port))
        .bearer_auth(&dto.token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "hi from A");

    // 断开：本地收尾 + 通知服务端取消
    manager.disconnect(&http, &base, "key").await;
    assert!(manager.snapshot().is_none());
}

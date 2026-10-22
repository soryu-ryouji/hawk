//! 数据面端到端测试（iroh 基座）：隧道（EndpointId 认证）+ A 侧进程内转发 + B 侧会话代理。
//! 不依赖 hawk-server（控制面以 mock hub 复刻协议形状）；传输细节（打洞/迁移）由 iroh 自治，
//! 本套件只验证 hawk 自身的编排与执法语义。

use axum::extract::Request;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use hawk_daemon::core::events::EventBus;
use hawk_daemon::remote::config::RemoteConfigStore;
use hawk_daemon::remote::data::{serve_proxy, DataPlane, ProxyContext, TunneledPeer};
use hawk_daemon::remote::identity::Identity;
use hawk_daemon::remote::session::{EndpointSlot, SessionManager};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

fn new_identity(dir: &tempfile::TempDir, device_id: Option<&str>) -> Identity {
    let remote_dir = dir.path().join("remote");
    let mut identity = Identity::load_or_create(remote_dir.to_str().unwrap()).unwrap();
    identity.device_id = device_id.map(String::from);
    identity
}

/// 测试用 SessionManager（B 侧拨号 endpoint 由 data_plane_b 提供）
async fn test_manager(
    dir: &tempfile::TempDir,
    data_plane_b: &Arc<DataPlane>,
) -> Arc<SessionManager> {
    let remote =
        RemoteConfigStore::open(Some(dir.path().join("remote.toml").to_str().unwrap()), None);
    let slot: EndpointSlot = Default::default();
    *slot.write().unwrap() = Some(data_plane_b.clone());
    SessionManager::new(EventBus::new(), remote, slot)
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

/// ProxyContext 构造（连接单元格）
fn test_proxy_ctx(
    conn: iroh::endpoint::Connection,
    token: &str,
    session_id: &str,
) -> Arc<ProxyContext> {
    Arc::new(ProxyContext {
        conn: Arc::new(std::sync::RwLock::new(conn)),
        token: token.to_string(),
        session_id: session_id.to_string(),
    })
}

/// B 侧数据面（拨号用 endpoint 载体；relay = None 纯直连）。
/// 必须用 B 的真实身份——A 侧按 endpoint_id 放行，随机身份会被执法断开
async fn bind_b(identity: &Identity) -> Arc<DataPlane> {
    DataPlane::bind(identity, 0, None, Router::new())
        .await
        .expect("B 侧数据面绑定")
}

#[tokio::test]
async fn tunnel_carries_http_with_viewer_marker() {
    let dir_a = tempfile::tempdir().unwrap();
    let identity_a = new_identity(&dir_a, Some("d_a"));
    let identity_b = Identity::load_or_create(
        tempfile::tempdir()
            .unwrap()
            .path()
            .join("remote")
            .to_str()
            .unwrap(),
    )
    .unwrap();
    let id_a: iroh::EndpointId = identity_a.endpoint_id.parse().unwrap();
    let id_b: iroh::EndpointId = identity_b.endpoint_id.parse().unwrap();

    // A 侧：数据面 + 放行 B 的身份
    let plane_a = DataPlane::bind(&identity_a, 0, None, test_router())
        .await
        .unwrap();
    plane_a.allow_session("s_tunnel", id_b, false);

    // B 侧：拨号（直连地址取 A 的上报地址）
    let plane_b = bind_b(&identity_b).await;
    let addr_b =
        iroh::EndpointAddr::new(id_a).with_addrs(plane_a.endpoint().unwrap().addr().addrs.clone());
    let conn = plane_b
        .endpoint()
        .unwrap()
        .connect(addr_b, hawk_daemon::remote::data::HAWK_ALPN)
        .await
        .expect("隧道连接");

    let proxy_port = free_proxy(test_proxy_ctx(conn, "session-token", "s_tunnel")).await;

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

    // 无 token / 错 token 一律 401
    for req in [
        client.get(format!("http://127.0.0.1:{proxy_port}/hello")),
        client.get(format!("http://127.0.0.1:{proxy_port}/hello?token=wrong")),
    ] {
        let resp = req.send().await.unwrap();
        assert_eq!(resp.status(), 401);
    }

    // 隧道端拦截 token 发现端点（协议安全模型）
    let resp = client
        .get(format!(
            "http://127.0.0.1:{proxy_port}/api/v1/app/token?token=session-token"
        ))
        .send()
        .await;
    match resp {
        Ok(resp) => assert_eq!(resp.status(), 404),
        Err(e) => panic!("隧道端应返回 404 而非连接错误: {e}"),
    }
}

#[tokio::test]
async fn await_first_conn_times_out_and_succeeds() {
    let dir_a = tempfile::tempdir().unwrap();
    let identity_a = new_identity(&dir_a, Some("d_a"));
    let identity_b = Identity::load_or_create(
        tempfile::tempdir()
            .unwrap()
            .path()
            .join("remote")
            .to_str()
            .unwrap(),
    )
    .unwrap();
    let id_a: iroh::EndpointId = identity_a.endpoint_id.parse().unwrap();
    let id_b: iroh::EndpointId = identity_b.endpoint_id.parse().unwrap();

    let plane_a = DataPlane::bind(&identity_a, 0, None, test_router())
        .await
        .unwrap();

    // 放行但无请求：超时返回 false（接受会话后的清理路径）
    plane_a.allow_session("s_first", id_b, false);
    assert!(
        !plane_a
            .await_first_conn("s_first", Duration::from_millis(100))
            .await
    );

    // 对端连接并发出首个隧道请求：等待立即返回 true
    let plane_b = bind_b(&identity_b).await;
    let addr =
        iroh::EndpointAddr::new(id_a).with_addrs(plane_a.endpoint().unwrap().addr().addrs.clone());
    let conn = plane_b
        .endpoint()
        .unwrap()
        .connect(addr, hawk_daemon::remote::data::HAWK_ALPN)
        .await
        .expect("隧道连接");
    let proxy_port = free_proxy(test_proxy_ctx(conn, "session-token", "s_first")).await;
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

/// 同一设备身份承载多个会话：取消一个只关闭它的连接，另一个继续可用（网页版 + 插件共用身份）
#[tokio::test]
async fn sessions_with_same_identity_are_isolated() {
    let dir_a = tempfile::tempdir().unwrap();
    let identity_a = new_identity(&dir_a, Some("d_a"));
    let identity_b = Identity::load_or_create(
        tempfile::tempdir()
            .unwrap()
            .path()
            .join("remote")
            .to_str()
            .unwrap(),
    )
    .unwrap();
    let id_a: iroh::EndpointId = identity_a.endpoint_id.parse().unwrap();
    let id_b: iroh::EndpointId = identity_b.endpoint_id.parse().unwrap();

    let plane_a = DataPlane::bind(&identity_a, 0, None, test_router())
        .await
        .unwrap();
    plane_a.allow_session("s_a", id_b, false);
    plane_a.allow_session("s_b", id_b, false);

    let plane_b = bind_b(&identity_b).await;
    let endpoint_b = plane_b.endpoint().unwrap();
    let addr = || {
        iroh::EndpointAddr::new(id_a).with_addrs(plane_a.endpoint().unwrap().addr().addrs.clone())
    };
    let conn_a = endpoint_b
        .connect(addr(), hawk_daemon::remote::data::HAWK_ALPN)
        .await
        .expect("会话 A 隧道连接");
    let conn_b = endpoint_b
        .connect(addr(), hawk_daemon::remote::data::HAWK_ALPN)
        .await
        .expect("会话 B 隧道连接");
    let proxy_a = free_proxy(test_proxy_ctx(conn_a, "token-a", "s_a")).await;
    let proxy_b = free_proxy(test_proxy_ctx(conn_b, "token-b", "s_b")).await;

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

    // 身份仍被 s_b 引用：新连接仍可建立并服务
    let conn_c = endpoint_b
        .connect(addr(), hawk_daemon::remote::data::HAWK_ALPN)
        .await
        .expect("s_b 存续期间身份应保持放行");
    drop(conn_c);

    plane_a.disallow_session("s_b");
}

/// 未放行身份的连接：握手完成后被 A 侧断开（执法点），隧道请求不可用
#[tokio::test]
async fn tunnel_rejects_unknown_identity() {
    let dir_a = tempfile::tempdir().unwrap();
    let identity_a = new_identity(&dir_a, Some("d_a"));
    let identity_b = Identity::load_or_create(
        tempfile::tempdir()
            .unwrap()
            .path()
            .join("remote")
            .to_str()
            .unwrap(),
    )
    .unwrap();
    let id_a: iroh::EndpointId = identity_a.endpoint_id.parse().unwrap();

    let plane_a = DataPlane::bind(&identity_a, 0, None, test_router())
        .await
        .unwrap();
    // 放行的是另一台设备：B 的身份不在放行集合
    let id_other: iroh::EndpointId = iroh::SecretKey::generate().public();
    plane_a.allow_session("s_wrong", id_other, false);

    let plane_b = bind_b(&identity_b).await;
    let addr =
        iroh::EndpointAddr::new(id_a).with_addrs(plane_a.endpoint().unwrap().addr().addrs.clone());
    let conn = plane_b
        .endpoint()
        .unwrap()
        .connect(addr, hawk_daemon::remote::data::HAWK_ALPN)
        .await
        .expect("握手本身不校验放行（执法点在连接建立后）");
    let proxy_port = free_proxy(test_proxy_ctx(conn, "session-token", "s_wrong")).await;
    // A 侧随即断开（close 帧传播有竞态，轮询至生效）：隧道请求必须失败
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let resp = client
            .get(format!("http://127.0.0.1:{proxy_port}/hello"))
            .bearer_auth("session-token")
            .send()
            .await;
        match resp {
            Ok(r) if r.status() == 200 => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "未放行身份不应能用隧道"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            // 401/403/502/连接错误：任一失败形态都算执法生效
            _ => break,
        }
    }
}

// ---------- 会话编排全流程（mock hub） ----------

use axum::Json;

/// mock hub：devices（含 A 的 endpoint_id）+ sessions（发起即 ready，connect 指向 A 侧数据面）
async fn spawn_hub(plane_a: &DataPlane, id_a: &str) -> String {
    let endpoint_id = id_a.to_string();
    let addrs = plane_a
        .endpoint()
        .unwrap()
        .addr()
        .addrs
        .iter()
        .filter_map(|t| match t {
            iroh::TransportAddr::Ip(a) => Some(a.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let endpoint_id_ready = endpoint_id.clone();
    let app = Router::new()
        .route(
            "/api/v1/devices",
            axum::routing::get(move || {
                let fp = endpoint_id.clone();
                async move {
                    Json(json!({"status":"success","data":{"devices":[
                        {"device_id":"d_a","name":"家里台式机","platform":"macos","library_name":"A 的库",
                         "online":true,"allow_remote":true,"endpoint_id":fp}
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
                        "endpoint_id": endpoint_id_ready,
                        "direct_addresses": addrs,
                    }
                }}))
            }),
        )
        .route(
            "/api/v1/sessions/s_1/cancel",
            axum::routing::post(|| async move { Json(json!({"status":"success"})) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn session_orchestration_end_to_end() {
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let identity_a = new_identity(&dir_a, Some("d_a"));
    let identity_b = new_identity(&dir_b, Some("d_b"));
    let id_b: iroh::EndpointId = identity_b.endpoint_id.parse().unwrap();

    // A 侧数据面就绪并放行 B 与 mock 会话（真实流程由 signal 的 session.request 驱动，此处直接裁决）
    let plane_a = DataPlane::bind(&identity_a, 0, None, test_router())
        .await
        .unwrap();
    plane_a.allow_session("s_1", id_b, false);

    let base = spawn_hub(&plane_a, &identity_a.endpoint_id).await;
    let http = reqwest::Client::new();
    let plane_b = DataPlane::bind(&identity_b, 0, None, Router::new())
        .await
        .unwrap();
    let manager = test_manager(&dir_b, &plane_b).await;

    let dto = manager
        .connect(&http, &base, "key", &identity_b, "d_a")
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

/// 连接级重拨兜底：连接死亡 → 原会话内重拨新连接，代理恢复
#[tokio::test]
async fn session_rebuilds_in_place_when_connection_dies() {
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let identity_a = new_identity(&dir_a, Some("d_a"));
    let identity_b = new_identity(&dir_b, Some("d_b"));
    let id_b: iroh::EndpointId = identity_b.endpoint_id.parse().unwrap();

    let plane_a = DataPlane::bind(&identity_a, 0, None, test_router())
        .await
        .unwrap();
    plane_a.allow_session("s_1", id_b, false);
    let base = spawn_hub(&plane_a, &identity_a.endpoint_id).await;

    let http = reqwest::Client::new();
    let plane_b = DataPlane::bind(&identity_b, 0, None, Router::new())
        .await
        .unwrap();
    let manager = test_manager(&dir_b, &plane_b).await;
    let dto = manager
        .connect(&http, &base, "key", &identity_b, "d_a")
        .await
        .expect("会话建立");
    let conn_before = manager.debug_active_stable_id().unwrap();

    // 连接级死亡（A 侧会话放行仍在，重拨可重新握手）
    manager.debug_close_current_conn();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let resp = http
            .get(format!("http://127.0.0.1:{}/hello", dto.proxy_port))
            .bearer_auth(&dto.token)
            .send()
            .await;
        if resp.is_ok_and(|r| r.status() == 200) {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "重拨后代理未恢复");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let conn_after = manager.debug_active_stable_id().unwrap();
    assert_ne!(conn_before, conn_after, "重拨应产生新连接");
    manager.disconnect(&http, &base, "key").await;
}

/// 中继形态端到端：内嵌 iroh-relay + 两端 RelayMode::Custom + 地址仅带 relay_url
#[tokio::test]
async fn relay_session_end_to_end() {
    // 内嵌中继（无 token 准入：测试 AllowAll；token 路径由 server 侧测试覆盖）
    let mut config = iroh_relay::server::ServerConfig::default();
    config.relay = Some(iroh_relay::server::RelayConfig::new(
        "127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap(),
    ));
    let relay = iroh_relay::server::Server::spawn(config).await.unwrap();
    let relay_addr = relay.http_addr().expect("relay 监听地址");
    let relay_url = format!("http://{relay_addr}");

    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let identity_a = new_identity(&dir_a, Some("d_a"));
    let identity_b = new_identity(&dir_b, Some("d_b"));
    let id_b: iroh::EndpointId = identity_b.endpoint_id.parse().unwrap();

    let plane_a = DataPlane::bind(
        &identity_a,
        0,
        Some((relay_url.clone(), String::new())),
        test_router(),
    )
    .await
    .unwrap();
    plane_a.allow_session("s_1", id_b, false);
    let plane_b = DataPlane::bind(
        &identity_b,
        0,
        Some((relay_url.clone(), String::new())),
        Router::new(),
    )
    .await
    .unwrap();

    // 拨号地址只带 relay_url：连接必须全程经中继
    let addr = iroh::EndpointAddr::new(plane_a.endpoint().unwrap().id())
        .with_relay_url(relay_url.parse().unwrap());
    let conn = tokio::time::timeout(
        Duration::from_secs(10),
        plane_b
            .endpoint()
            .unwrap()
            .connect(addr, hawk_daemon::remote::data::HAWK_ALPN),
    )
    .await
    .expect("中继握手超时")
    .expect("中继握手");
    let proxy_port = free_proxy(test_proxy_ctx(conn, "session-token", "s_1")).await;
    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{proxy_port}/hello"))
        .bearer_auth("session-token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "hi from A");
    relay.shutdown().await.unwrap();
}

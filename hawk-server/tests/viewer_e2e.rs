//! 网页版全链路互操作测试：浏览器 → hawk-server（两级登录/会话/代理）→ 中继 → 真 A 侧
//! （dev-dep hawk-daemon 的 DataPlane/relay serve，协议级真实对端，产物零代码共享）。
//! 鉴权模型：service-key（池门禁）→ 门令牌 → hawk-app key（逐 key 读/写权限）→ bt。

use axum::extract::Request;
use axum::routing::get;
use axum::Router;
use futures_util::StreamExt;
use hawk_daemon::remote::data::{DataPlane, TunneledPeer};
use hawk_daemon::remote::identity::Identity as DaemonIdentity;
use hawk_server::api;
use hawk_server::keystore::{sha256_hex, KeyStore};
use hawk_server::state::AppState;
use hawk_server::store::Store;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// A 侧素材库对外发放的 hawk-app key（明文只在测试内可见）
const READ_KEY: &str = "viewer-read-key";
const WRITE_KEY: &str = "viewer-write-key-0123456789";

async fn spawn_server(dir: &Path) -> (String, String) {
    let keys = KeyStore::open(dir).unwrap();
    let (_, service_key) = keys.tenant_add("test").unwrap();
    let state = Arc::new(AppState::new(Arc::new(keys), Store::load(dir).unwrap()));
    let identity = hawk_server::identity::Identity::load_or_create(dir).unwrap();
    let device_id = hawk_server::viewer::ensure_viewer_device(&state, &identity).unwrap();
    let mut identity = identity;
    if identity.device_id.is_none() {
        identity.device_id = Some(device_id);
        identity.persist(dir).unwrap();
    }
    let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .alpns(vec![hawk_daemon::remote::data::HAWK_ALPN.to_vec()])
        .secret_key(identity.secret_key().unwrap())
        .relay_mode(iroh::RelayMode::Disabled)
        .bind()
        .await
        .unwrap();
    let viewer = hawk_server::viewer::ViewerManager::new(state.clone(), identity, endpoint, dir);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(
            listener,
            api::router(state, viewer).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (format!("127.0.0.1:{port}"), service_key)
}

/// A 侧素材路由：/api/v1/hello 校验 TunneledPeer 标记（只读/写会话）；
/// /app 与 /assets/* 模拟 daemon 的 web_dist 静态托管（验证非 API 路径也经隧道转发）
fn a_router() -> Router {
    Router::new()
        .route(
            "/api/v1/hello",
            get(|req: Request| async move {
                match req.extensions().get::<TunneledPeer>() {
                    Some(peer) if peer.writable => "write from A",
                    Some(_) => "hi from A",
                    None => "missing marker",
                }
            }),
        )
        .route(
            "/app",
            get(|| async {
                (
                    [("content-type", "text/html")],
                    "<title>hawk-spa-shell</title>",
                )
            }),
        )
        .route(
            "/assets/app.js",
            get(|| async { ([("content-type", "text/javascript")], "console.log('spa')") }),
        )
}

/// 模拟 A 侧 daemon remote 模块：注册/心跳/key 映射同步/WSS 待命 → session.request(relay)
/// → 凭证执法 + 放行 + 接受 + 中继服务
async fn start_device_a(base: &str, service_key: &str, dir: &Path) -> (String, Arc<DataPlane>) {
    let remote_dir = dir.join("remote");
    let identity = DaemonIdentity::load_or_create(remote_dir.to_str().unwrap()).unwrap();
    let client = reqwest::Client::new();

    // 注册 + 心跳上线
    let resp: Value = client
        .post(format!("http://{base}/api/v1/devices"))
        .bearer_auth(service_key)
        .json(&json!({
            "name": "家里台式机", "platform": "macos", "endpoint_id": identity.endpoint_id,
            "library_name": "A 的库", "allow_remote": true, "allow_write": true,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let device_a = resp["data"]["device_id"].as_str().unwrap().to_string();

    // key 映射同步（daemon 上送 sha256，server 不见明文）
    let resp = client
        .put(format!("http://{base}/api/v1/devices/{device_a}/keys"))
        .bearer_auth(service_key)
        .json(&json!({ "keys": [
            { "s": sha256_hex(READ_KEY), "read": true, "write": false },
            { "s": sha256_hex(WRITE_KEY), "read": true, "write": true },
        ]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "key 映射同步失败");

    let plane = DataPlane::bind(&identity, 0, None, a_router())
        .await
        .unwrap();
    // 心跳上报真实直连地址（网页版宿主经此拨号；iroh 传输层自治路径选择）
    let resp_hb = client
        .post(format!("http://{base}/api/v1/devices/heartbeat"))
        .bearer_auth(service_key)
        .json(&json!({
            "device_id": device_a,
            "addrs": plane.direct_addrs(),
        }))
        .send()
        .await
        .unwrap();
    let status = resp_hb.status();
    let body = resp_hb.text().await.unwrap();
    assert!(status.is_success(), "心跳失败 {status}: {body}");

    // A 侧本地 key 执法依据（真实 daemon 由 remote::keys::KeyStore 承载；此处内联模拟）
    let known_keys: Arc<std::sync::Mutex<Vec<(String, bool)>>> =
        Arc::new(std::sync::Mutex::new(vec![
            (sha256_hex(READ_KEY), false),
            (sha256_hex(WRITE_KEY), true),
        ]));

    // WSS 待命：收到 relay 会话请求 → 凭证执法 → 放行对端指纹 → accept → 经中继 WS 提供 QUIC
    {
        let base = base.to_string();
        let service_key = service_key.to_string();
        let device_a = device_a.clone();
        let plane = plane.clone();
        tokio::spawn(async move {
            use tokio_tungstenite::tungstenite::client::IntoClientRequest;
            let mut req = format!("ws://{base}/api/v1/ws?device_id={device_a}")
                .into_client_request()
                .unwrap();
            req.headers_mut().insert(
                "Authorization",
                format!("Bearer {service_key}").parse().unwrap(),
            );
            let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
            let http = reqwest::Client::new();
            while let Some(Ok(msg)) = ws.next().await {
                let Ok(text) = msg.into_text() else { continue };
                let Ok(v) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                if v["type"] != "session.request" {
                    continue;
                }
                let session_id = v["session_id"].as_str().unwrap().to_string();
                // 本地执法：credential 必须命中本地 key 映射，且写会话需写权限
                let requested_write = v["access"].as_str() == Some("write");
                let credential = v["credential"].as_str().unwrap_or_default();
                let granted = known_keys
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|(s, _)| s == credential)
                    .map(|(_, w)| *w);
                match granted {
                    Some(write_perm) if !requested_write || write_perm => {}
                    _ => {
                        let _ = http
                            .post(format!("http://{base}/api/v1/sessions/{session_id}/reject"))
                            .bearer_auth(&service_key)
                            .json(&json!({ "device_id": device_a }))
                            .send()
                            .await;
                        continue;
                    }
                }
                // 对端身份随 session.request 下发（协议：from.endpoint_id）
                let Some(peer) = v["from"]["endpoint_id"]
                    .as_str()
                    .and_then(|s| s.parse::<iroh::EndpointId>().ok())
                else {
                    continue;
                };
                plane.allow_session(&session_id, peer, requested_write);
                // accept（之后网页版宿主直接按 connect 的直连地址拨号过来）
                let _resp = http
                    .post(format!("http://{base}/api/v1/sessions/{session_id}/accept"))
                    .bearer_auth(&service_key)
                    .json(&json!({ "device_id": device_a }))
                    .send()
                    .await
                    .unwrap();
            }
        });
    }

    (device_a, plane)
}

fn init_log() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("hawk_server=debug,hawk_daemon=debug")
            }),
        )
        .try_init();
}

/// 两级登录：service-key → 门令牌；门令牌 + hawk-app key → bt
async fn gate_and_login(base: &str, service_key: &str, app_key: &str) -> Value {
    let client = reqwest::Client::new();
    let resp: Value = client
        .post(format!("http://{base}/api/v1/viewer/gate"))
        .json(&json!({ "service_key": service_key }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp["status"], "success", "gate 失败: {resp}");
    let gate_token = resp["data"]["gate_token"].as_str().unwrap();
    client
        .post(format!("http://{base}/api/v1/viewer/login"))
        .json(&json!({ "gate_token": gate_token, "key": app_key }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn viewer_browses_library_through_domain() {
    init_log();
    let server_dir = tempfile::tempdir().unwrap();
    let (base, service_key) = spawn_server(server_dir.path()).await;
    let a_dir = tempfile::tempdir().unwrap();
    let (device_a, _plane_a) = start_device_a(&base, &service_key, a_dir.path()).await;

    let client = reqwest::Client::new();
    // ---- 第一级：错误 service-key → 401 ----
    let resp = client
        .post(format!("http://{base}/api/v1/viewer/gate"))
        .json(&json!({ "service_key": "hs_wrong" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // ---- 第二级：错误 hawk-app key → 401 ----
    let resp: Value = client
        .post(format!("http://{base}/api/v1/viewer/gate"))
        .json(&json!({ "service_key": service_key }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let gate_token = resp["data"]["gate_token"].as_str().unwrap().to_string();
    let resp = client
        .post(format!("http://{base}/api/v1/viewer/login"))
        .json(&json!({ "gate_token": gate_token, "key": "wrong" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // ---- 正确两级登录 → bt（绑定设备） ----
    let resp = gate_and_login(&base, &service_key, READ_KEY).await;
    assert_eq!(resp["status"], "success", "登录失败: {resp}");
    let bt = resp["data"]["token"].as_str().unwrap().to_string();
    assert_eq!(resp["data"]["access"], "read");
    assert_eq!(resp["data"]["device_id"], device_a);

    // ---- 设备列表只含 bt 绑定的设备（无池枚举面） ----
    let resp: Value = client
        .get(format!("http://{base}/api/v1/viewer/devices"))
        .bearer_auth(&bt)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let devices = resp["data"]["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0]["device_id"], device_a);

    // ---- 打开查看会话（服务端作为 B 侧经中继连到 A） ----
    let resp: Value = client
        .post(format!("http://{base}/api/v1/viewer/session"))
        .bearer_auth(&bt)
        .json(&json!({ "target_device_id": device_a }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp["status"], "success", "查看会话建立失败: {resp}");
    assert_eq!(resp["data"]["target_name"], "家里台式机");

    // ---- 浏览器经域名代理访问 A 的素材 API ----
    let resp = client
        .get(format!("http://{base}/api/v1/hello"))
        .bearer_auth(&bt)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "hi from A");

    // 查询参数令牌（img/SSE 场景）
    let resp = client
        .get(format!("http://{base}/api/v1/hello?token={bt}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // 无令牌 → 401
    let resp = client
        .get(format!("http://{base}/api/v1/hello"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // 错误令牌 → 401
    let resp = client
        .get(format!("http://{base}/api/v1/hello"))
        .bearer_auth("hb_wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // ---- 查看器静态资源同样经隧道由 A 侧 daemon 托管（服务端不再部署 web-dist） ----
    let resp = client
        .get(format!("http://{base}/app?token={bt}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.text().await.unwrap().contains("hawk-spa-shell"));

    let resp = client
        .get(format!("http://{base}/assets/app.js?token={bt}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "console.log('spa')");

    // 页面 URL 带的 token 下发给静态子资源（<script>/<img> 无法带请求头），
    // 后续 assets 请求仅靠 cookie 就能过隧道
    let resp = client
        .get(format!("http://{base}/app?token={bt}"))
        .header("accept", "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let cookie = resp
        .headers()
        .get("set-cookie")
        .expect("页面导航未下发 token cookie")
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    assert!(cookie.starts_with("hawk_bt=hb_"), "cookie 异常: {cookie}");

    let resp = client
        .get(format!("http://{base}/assets/app.js"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "console.log('spa')");

    // cookie 不适用于 API 请求（不认自动携带的凭据，避免 CSRF 面）
    let resp = client
        .get(format!("http://{base}/api/v1/hello"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // 地址栏直接打开 /app 而令牌失效 → 302 回选择器页（不是浏览器看不懂的裸 401）
    let no_redirect = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let resp = no_redirect
        .get(format!("http://{base}/app?token=hb_stale"))
        .header("accept", "text/html,application/xhtml+xml")
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_redirection(),
        "期望重定向，得到 {}",
        resp.status()
    );
    assert_eq!(resp.headers().get("location").unwrap(), "/?token=hb_stale");

    // 静态资源/XHR 拿到 401 而非重定向（重定向只会把 JS 请求变成一份 HTML）
    let resp = no_redirect
        .get(format!("http://{base}/assets/app.js"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // ---- 断开 ----
    let resp: Value = client
        .delete(format!("http://{base}/api/v1/viewer/session"))
        .bearer_auth(&bt)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp["status"], "success");
    let resp: Value = client
        .get(format!("http://{base}/api/v1/viewer/session"))
        .bearer_auth(&bt)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp["data"], Value::Null);

    // 选择器页可访问（两级登录表单）
    let resp = client.get(format!("http://{base}/")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let html = resp.text().await.unwrap();
    assert!(html.contains("hawk 网页版"));
    assert!(html.contains("service-key"));
}

/// 只读 key 不能开写会话；写 key 开写会话且隧道请求带 writable 标记
#[tokio::test]
async fn viewer_write_requires_write_key() {
    init_log();
    let server_dir = tempfile::tempdir().unwrap();
    let (base, service_key) = spawn_server(server_dir.path()).await;
    let a_dir = tempfile::tempdir().unwrap();
    let (device_a, _plane_a) = start_device_a(&base, &service_key, a_dir.path()).await;
    let client = reqwest::Client::new();

    // 只读 key 的 bt 不能开写会话
    let resp = gate_and_login(&base, &service_key, READ_KEY).await;
    let read_bt = resp["data"]["token"].as_str().unwrap().to_string();
    let resp = client
        .post(format!("http://{base}/api/v1/viewer/session"))
        .bearer_auth(&read_bt)
        .json(&json!({ "target_device_id": device_a, "access": "write" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403, "只读 key 不应能开写会话");

    // 写 key：bt access=write，写会话成功，隧道请求带 writable 标记
    let resp = gate_and_login(&base, &service_key, WRITE_KEY).await;
    assert_eq!(resp["data"]["access"], "write");
    let write_bt = resp["data"]["token"].as_str().unwrap().to_string();

    let resp: Value = client
        .post(format!("http://{base}/api/v1/viewer/session"))
        .bearer_auth(&write_bt)
        .json(&json!({ "target_device_id": device_a, "access": "write" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp["status"], "success", "写会话建立失败: {resp}");
    assert_eq!(resp["data"]["access"], "write");

    let resp = client
        .get(format!("http://{base}/api/v1/hello"))
        .bearer_auth(&write_bt)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "write from A");
}

/// 已吊销的 key（映射被设备侧移除）登录即失败
#[tokio::test]
async fn revoked_key_cannot_login() {
    init_log();
    let server_dir = tempfile::tempdir().unwrap();
    let (base, service_key) = spawn_server(server_dir.path()).await;
    let a_dir = tempfile::tempdir().unwrap();
    let (device_a, _plane_a) = start_device_a(&base, &service_key, a_dir.path()).await;
    let client = reqwest::Client::new();

    // 吊销 READ_KEY（覆盖式同步只剩 WRITE_KEY）
    let resp = client
        .put(format!("http://{base}/api/v1/devices/{device_a}/keys"))
        .bearer_auth(&service_key)
        .json(&json!({ "keys": [
            { "s": sha256_hex(WRITE_KEY), "read": true, "write": true },
        ]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let resp = gate_and_login(&base, &service_key, READ_KEY).await;
    assert_eq!(resp["status"], "error", "已吊销 key 不应登录成功");
}

/// 隧道断开后代理返回 401/502 而非挂起（断线清理路径）
#[tokio::test]
async fn viewer_session_expires_when_tunnel_drops() {
    let server_dir = tempfile::tempdir().unwrap();
    let (base, service_key) = spawn_server(server_dir.path()).await;
    let a_dir = tempfile::tempdir().unwrap();
    let (_device_a, plane_a) = start_device_a(&base, &service_key, a_dir.path()).await;

    let resp = gate_and_login(&base, &service_key, READ_KEY).await;
    let bt = resp["data"]["token"].as_str().unwrap().to_string();
    let device_a = resp["data"]["device_id"].as_str().unwrap().to_string();
    let client = reqwest::Client::new();
    client
        .post(format!("http://{base}/api/v1/viewer/session"))
        .bearer_auth(&bt)
        .json(&json!({ "target_device_id": device_a }))
        .send()
        .await
        .unwrap();

    // A 侧收回放行并断开（模拟设备离线/会话取消）
    plane_a.reset_peers();
    tokio::time::sleep(Duration::from_millis(300)).await;

    // 会话已清理：current 为 null
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let resp: Value = client
            .get(format!("http://{base}/api/v1/viewer/session"))
            .bearer_auth(&bt)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if resp["data"].is_null() {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "断线会话未及时清理");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

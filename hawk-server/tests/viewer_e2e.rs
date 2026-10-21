//! 网页版全链路互操作测试：浏览器 → hawk-server（登录/选择器/会话/代理）→ 中继 → 真 A 侧
//! （dev-dep hawk-daemon 的 DataPlane/relay serve，协议级真实对端，产物零代码共享）。

use axum::extract::Request;
use axum::routing::get;
use axum::Router;
use futures_util::StreamExt;
use hawk_daemon::remote::data::{DataPlane, TunneledViewer};
use hawk_daemon::remote::identity::Identity as DaemonIdentity;
use hawk_daemon::remote::{relay as daemon_relay, tls as daemon_tls};
use hawk_server::api;
use hawk_server::state::AppState;
use hawk_server::store::Store;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const KEY: &str = "viewer-e2e-key";

async fn spawn_server(dir: &Path) -> String {
    let store = Store::load(dir).unwrap();
    let state = Arc::new(AppState::new(KEY.to_string(), store));
    let identity = hawk_server::identity::Identity::load_or_create(dir).unwrap();
    let device_id = hawk_server::viewer::ensure_viewer_device(&state, &identity).unwrap();
    let mut identity = identity;
    if identity.device_id.is_none() {
        identity.device_id = Some(device_id);
        identity.persist(dir).unwrap();
    }
    let viewer = hawk_server::viewer::ViewerManager::new(state.clone(), identity, dir);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(
            listener,
            api::router(state, viewer, None).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    format!("127.0.0.1:{port}")
}

/// A 侧素材路由：/hello 校验 TunneledViewer 标记（只读受托语义）
fn a_router() -> Router {
    Router::new().route(
        "/api/v1/hello",
        get(|req: Request| async move {
            if req.extensions().get::<TunneledViewer>().is_some() {
                "hi from A"
            } else {
                "missing marker"
            }
        }),
    )
}

/// 模拟 A 侧 daemon remote 模块：注册/心跳/WSS 待命 → session.request(relay) → 放行+接受+中继服务
async fn start_device_a(base: &str, dir: &Path) -> (String, Arc<DataPlane>) {
    let remote_dir = dir.join("remote");
    let identity = DaemonIdentity::load_or_create(remote_dir.to_str().unwrap()).unwrap();
    let client = reqwest::Client::new();

    // 注册 + 心跳上线
    let resp: Value = client
        .post(format!("http://{base}/api/v1/devices"))
        .bearer_auth(KEY)
        .json(&json!({
            "name": "家里台式机", "platform": "macos", "fingerprint": identity.fingerprint,
            "library_name": "A 的库", "allow_remote": true,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let device_a = resp["data"]["device_id"].as_str().unwrap().to_string();
    client
        .post(format!("http://{base}/api/v1/devices/heartbeat"))
        .bearer_auth(KEY)
        .json(&json!({
            "device_id": device_a,
            "reachability": { "mapped_port": null, "mapping_type": "none", "public_ipv6": null, "ipv6_port": null }
        }))
        .send()
        .await
        .unwrap();

    let plane = DataPlane::bind(&identity, 0, a_router()).unwrap();

    // WSS 待命：收到 relay 会话请求 → 放行对端指纹 → accept → 经中继 WS 提供 QUIC
    {
        let base = base.to_string();
        let device_a = device_a.clone();
        let plane = plane.clone();
        let identity = identity.clone();
        tokio::spawn(async move {
            let url = format!("ws://{base}/api/v1/ws?key={KEY}&device_id={device_a}");
            let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
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
                let from = v["from"]["device_id"].as_str().unwrap().to_string();
                // 对端指纹（设备列表中的「网页版」服务端设备）
                let devices: Value = http
                    .get(format!("http://{base}/api/v1/devices"))
                    .bearer_auth(KEY)
                    .send()
                    .await
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                let fp_hex = devices["data"]["devices"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|d| d["device_id"] == from)
                    .unwrap()["fingerprint"]
                    .as_str()
                    .unwrap()
                    .to_string();
                let fp = daemon_tls::parse_fingerprint(&fp_hex).unwrap();
                plane.allow_peer(fp);
                // accept
                let _resp = http
                    .post(format!("http://{base}/api/v1/sessions/{session_id}/accept"))
                    .bearer_auth(KEY)
                    .json(&json!({ "device_id": device_a }))
                    .send()
                    .await
                    .unwrap();
                // 中继 WS（role=a）+ QUIC 服务
                let relay_url = daemon_relay::relay_ws_url(
                    &format!("http://{base}"),
                    KEY,
                    &device_a,
                    &session_id,
                    "a",
                )
                .unwrap();
                let (relay_ws, _) = match tokio_tungstenite::connect_async(&relay_url).await {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                let plane2 = plane.clone();
                let identity2 = identity.clone();
                tokio::spawn(async move {
                    let _ = daemon_relay::serve_over_ws(&identity2, &plane2, relay_ws, a_router())
                        .await;
                });
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

#[tokio::test]
async fn viewer_browses_library_through_domain() {
    init_log();
    let server_dir = tempfile::tempdir().unwrap();
    let base = spawn_server(server_dir.path()).await;
    let a_dir = tempfile::tempdir().unwrap();
    let (device_a, _plane_a) = start_device_a(&base, a_dir.path()).await;

    let client = reqwest::Client::new();
    // ---- 登录（access_key 换浏览器令牌） ----
    let resp: Value = client
        .post(format!("http://{base}/api/v1/viewer/login"))
        .json(&json!({ "access_key": "wrong" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp["status"], "error");

    let resp: Value = client
        .post(format!("http://{base}/api/v1/viewer/login"))
        .json(&json!({ "access_key": KEY }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let bt = resp["data"]["token"].as_str().unwrap().to_string();

    // ---- 设备列表（含 A，不含服务端自身在前端过滤前的原始列表） ----
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
    assert!(devices.iter().any(|d| d["device_id"] == device_a));
    assert!(devices.iter().any(|d| d["platform"] == "server"));

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

    // 选择器页可访问
    let resp = client.get(format!("http://{base}/")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.text().await.unwrap().contains("hawk 网页版"));
}

/// 隧道断开后代理返回 401/502 而非挂起（断线清理路径）
#[tokio::test]
async fn viewer_session_expires_when_tunnel_drops() {
    let server_dir = tempfile::tempdir().unwrap();
    let base = spawn_server(server_dir.path()).await;
    let a_dir = tempfile::tempdir().unwrap();
    let (device_a, plane_a) = start_device_a(&base, a_dir.path()).await;

    let client = reqwest::Client::new();
    let resp: Value = client
        .post(format!("http://{base}/api/v1/viewer/login"))
        .json(&json!({ "access_key": KEY }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let bt = resp["data"]["token"].as_str().unwrap().to_string();
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

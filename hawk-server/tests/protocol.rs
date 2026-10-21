//! 协议级集成测试：模拟 hawk-remote 客户端行为，覆盖注册/心跳/列表/会话全流程/鉴权/持久化。

use futures_util::StreamExt;
use hawk_server::{api, state::AppState, store::Store};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const KEY: &str = "test-access-key-0123456789";

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn spawn_server(dir: &Path) -> String {
    let store = Store::load(dir).unwrap();
    let state = Arc::new(AppState::new(KEY.to_string(), store));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(
            listener,
            api::router(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    format!("127.0.0.1:{port}")
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

async fn register(base: &str, name: &str, allow_remote: bool) -> String {
    let resp: Value = client()
        .post(format!("http://{base}/api/v1/devices"))
        .bearer_auth(KEY)
        .json(&json!({
            "name": name,
            "platform": "macos",
            "fingerprint": format!("fp-{name}"),
            "library_name": "我的素材库",
            "allow_remote": allow_remote,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp["status"], "success", "register failed: {resp}");
    resp["data"]["device_id"].as_str().unwrap().to_string()
}

async fn heartbeat(base: &str, device_id: &str, reachability: Value) -> reqwest::Response {
    client()
        .post(format!("http://{base}/api/v1/devices/heartbeat"))
        .bearer_auth(KEY)
        .json(&json!({ "device_id": device_id, "reachability": reachability }))
        .send()
        .await
        .unwrap()
}

fn full_reachability() -> Value {
    json!({
        "public_ip": null,
        "mapped_port": 27373,
        "mapping_type": "upnp",
        "public_ipv6": "240e::1",
        "ipv6_port": 27373,
        "nat_type": "unknown",
    })
}

fn empty_reachability() -> Value {
    json!({
        "public_ip": null,
        "mapped_port": null,
        "mapping_type": "none",
        "public_ipv6": null,
        "ipv6_port": null,
    })
}

async fn connect_ws(base: &str, device_id: &str) -> Ws {
    let url = format!("ws://{base}/api/v1/ws?key={KEY}&device_id={device_id}");
    let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    ws
}

async fn next_json(ws: &mut Ws) -> Value {
    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("ws 消息超时")
        .expect("ws 已关闭")
        .expect("ws 消息错误");
    serde_json::from_str(msg.to_text().unwrap()).unwrap()
}

async fn create_session(base: &str, source: &str, target: &str) -> (reqwest::StatusCode, Value) {
    let resp = client()
        .post(format!("http://{base}/api/v1/sessions"))
        .bearer_auth(KEY)
        .json(&json!({
            "source_device_id": source,
            "target_device_id": target,
            "mode": "direct",
        }))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    (status, resp.json().await.unwrap())
}

async fn session_state(base: &str, session_id: &str) -> Value {
    let resp: Value = client()
        .get(format!("http://{base}/api/v1/sessions/{session_id}"))
        .bearer_auth(KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    resp["data"].clone()
}

#[tokio::test]
async fn register_heartbeat_and_list() {
    let dir = tempfile::tempdir().unwrap();
    let base = spawn_server(dir.path()).await;
    let a = register(&base, "家里台式机", true).await;
    let b = register(&base, "笔记本", true).await;

    let resp: Value = client()
        .get(format!("http://{base}/api/v1/devices"))
        .bearer_auth(KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let devices = resp["data"]["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0]["online"], false);
    assert_eq!(devices[0]["last_seen_at_ms"], Value::Null);

    let resp = heartbeat(&base, &a, full_reachability()).await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["data"]["public_ip_seen"], "127.0.0.1");

    let resp: Value = client()
        .get(format!("http://{base}/api/v1/devices"))
        .bearer_auth(KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let devices = resp["data"]["devices"].as_array().unwrap();
    let da = &devices[0];
    assert_eq!(da["device_id"], a);
    assert_eq!(da["online"], true);
    assert_eq!(da["reachability"]["public_ip"], "127.0.0.1");
    assert_eq!(da["reachability"]["mapped_port"], 27373);
    assert_eq!(da["reachability"]["public_ipv6"], "240e::1");
    let db = &devices[1];
    assert_eq!(db["device_id"], b);
    assert_eq!(db["online"], false);
}

#[tokio::test]
async fn auth_required_on_all_endpoints() {
    let dir = tempfile::tempdir().unwrap();
    let base = spawn_server(dir.path()).await;

    // 无 key
    let resp = client()
        .get(format!("http://{base}/api/v1/devices"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "error");
    assert!(body["error"]["message"].is_string());

    // 错误 key
    let resp = client()
        .get(format!("http://{base}/api/v1/devices"))
        .bearer_auth("wrong-key")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // WSS 错误 key：握手被拒
    let device = register(&base, "台式机", true).await;
    let result = tokio_tungstenite::connect_async(format!(
        "ws://{base}/api/v1/ws?key=wrong&device_id={device}"
    ))
    .await;
    assert!(result.is_err());

    // WSS 未注册设备
    let result =
        tokio_tungstenite::connect_async(format!("ws://{base}/api/v1/ws?key={KEY}&device_id=d_x"))
            .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn heartbeat_throttled_below_min_interval() {
    let dir = tempfile::tempdir().unwrap();
    let base = spawn_server(dir.path()).await;
    let device = register(&base, "台式机", true).await;

    let resp = heartbeat(&base, &device, full_reachability()).await;
    assert_eq!(resp.status(), 200);
    // 协议限速：心跳最小间隔 10s
    let resp = heartbeat(&base, &device, full_reachability()).await;
    assert_eq!(resp.status(), 429);
}

#[tokio::test]
async fn session_direct_flow_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let base = spawn_server(dir.path()).await;
    let target = register(&base, "家里台式机", true).await;
    let source = register(&base, "笔记本", true).await;
    assert_eq!(
        heartbeat(&base, &target, full_reachability())
            .await
            .status(),
        200
    );

    let mut target_ws = connect_ws(&base, &target).await;
    let hello = next_json(&mut target_ws).await;
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["device_id"], target);

    // 发起会话 → 目标经 WSS 收到 session.request
    let (status, body) = create_session(&base, &source, &target).await;
    assert_eq!(status, 200);
    assert_eq!(body["data"]["state"], "pending");
    let session_id = body["data"]["session_id"].as_str().unwrap().to_string();

    let request = next_json(&mut target_ws).await;
    assert_eq!(request["type"], "session.request");
    assert_eq!(request["session_id"], session_id);
    assert_eq!(request["from"]["device_id"], source);
    assert_eq!(request["from"]["name"], "笔记本");
    assert_eq!(request["mode"], "direct");

    // 目标接受 → 双方收到 session.update(ready)，候选 IPv4 映射优先、IPv6 次之
    let resp = client()
        .post(format!("http://{base}/api/v1/sessions/{session_id}/accept"))
        .bearer_auth(KEY)
        .json(&json!({ "device_id": target }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let update = next_json(&mut target_ws).await;
    assert_eq!(update["type"], "session.update");
    assert_eq!(update["state"], "ready");
    let candidates = update["connect"]["candidates"].as_array().unwrap();
    assert_eq!(candidates[0]["transport"], "ipv4");
    assert_eq!(candidates[0]["host"], "127.0.0.1");
    assert_eq!(candidates[0]["port"], 27373);
    assert_eq!(candidates[1]["transport"], "ipv6");
    assert_eq!(candidates[1]["host"], "240e::1");
    assert_eq!(update["connect"]["fingerprint"], "fp-家里台式机");

    // 轮询兜底看到同一状态
    let state = session_state(&base, &session_id).await;
    assert_eq!(state["state"], "ready");
    assert_eq!(state["connect"]["mode"], "direct");
}

#[tokio::test]
async fn session_rejected_when_allow_remote_false() {
    let dir = tempfile::tempdir().unwrap();
    let base = spawn_server(dir.path()).await;
    let target = register(&base, "台式机", false).await;
    let source = register(&base, "笔记本", true).await;
    assert_eq!(
        heartbeat(&base, &target, full_reachability())
            .await
            .status(),
        200
    );

    let (status, body) = create_session(&base, &source, &target).await;
    assert_eq!(status, 200);
    assert_eq!(body["data"]["state"], "rejected");
}

#[tokio::test]
async fn session_requires_target_online() {
    let dir = tempfile::tempdir().unwrap();
    let base = spawn_server(dir.path()).await;
    let target = register(&base, "台式机", true).await;
    let source = register(&base, "笔记本", true).await;
    // 目标从未心跳 → 不在线
    let (status, body) = create_session(&base, &source, &target).await;
    assert_eq!(status, 404);
    assert_eq!(body["status"], "error");
}

#[tokio::test]
async fn session_reject_flow() {
    let dir = tempfile::tempdir().unwrap();
    let base = spawn_server(dir.path()).await;
    let target = register(&base, "台式机", true).await;
    let source = register(&base, "笔记本", true).await;
    assert_eq!(
        heartbeat(&base, &target, full_reachability())
            .await
            .status(),
        200
    );

    let (_, body) = create_session(&base, &source, &target).await;
    let session_id = body["data"]["session_id"].as_str().unwrap().to_string();

    let resp = client()
        .post(format!("http://{base}/api/v1/sessions/{session_id}/reject"))
        .bearer_auth(KEY)
        .json(&json!({ "device_id": target }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(session_state(&base, &session_id).await["state"], "rejected");

    // 终态不可再响应
    let resp = client()
        .post(format!("http://{base}/api/v1/sessions/{session_id}/accept"))
        .bearer_auth(KEY)
        .json(&json!({ "device_id": target }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);
}

#[tokio::test]
async fn session_unreachable_when_no_candidates() {
    let dir = tempfile::tempdir().unwrap();
    let base = spawn_server(dir.path()).await;
    let target = register(&base, "台式机", true).await;
    let source = register(&base, "笔记本", true).await;
    assert_eq!(
        heartbeat(&base, &target, empty_reachability())
            .await
            .status(),
        200
    );

    let (_, body) = create_session(&base, &source, &target).await;
    let session_id = body["data"]["session_id"].as_str().unwrap().to_string();
    let resp = client()
        .post(format!("http://{base}/api/v1/sessions/{session_id}/accept"))
        .bearer_auth(KEY)
        .json(&json!({ "device_id": target }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        session_state(&base, &session_id).await["state"],
        "unreachable"
    );
}

#[tokio::test]
async fn session_cancel_notifies_peer() {
    let dir = tempfile::tempdir().unwrap();
    let base = spawn_server(dir.path()).await;
    let target = register(&base, "台式机", true).await;
    let source = register(&base, "笔记本", true).await;
    assert_eq!(
        heartbeat(&base, &target, full_reachability())
            .await
            .status(),
        200
    );

    let mut target_ws = connect_ws(&base, &target).await;
    next_json(&mut target_ws).await; // hello

    let (_, body) = create_session(&base, &source, &target).await;
    let session_id = body["data"]["session_id"].as_str().unwrap().to_string();
    next_json(&mut target_ws).await; // session.request

    let resp = client()
        .post(format!("http://{base}/api/v1/sessions/{session_id}/cancel"))
        .bearer_auth(KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let cancelled = next_json(&mut target_ws).await;
    assert_eq!(cancelled["type"], "session.cancelled");
    assert_eq!(cancelled["session_id"], session_id);
    assert_eq!(
        session_state(&base, &session_id).await["state"],
        "cancelled"
    );
}

#[tokio::test]
async fn device_table_persists_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let base = spawn_server(dir.path()).await;
    register(&base, "台式机", true).await;

    // 模拟重启：同一数据目录重新加载（原实例仍在跑，load 只读不冲突）
    let base2 = spawn_server(dir.path()).await;
    let resp: Value = client()
        .get(format!("http://{base2}/api/v1/devices"))
        .bearer_auth(KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let devices = resp["data"]["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0]["name"], "台式机");
    assert_eq!(devices[0]["online"], false);
}

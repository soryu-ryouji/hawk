//! 协议级集成测试：模拟 hawk-remote 客户端行为，覆盖注册/心跳/列表/会话全流程/鉴权/持久化/
//! 租户隔离/key 映射同步/风控。

use futures_util::StreamExt;
use hawk_server::{api, keystore::KeyStore, state::AppState, store::Store};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// 拉起服务并创建一个租户，返回（地址，service-key）
async fn spawn_server(dir: &Path) -> (String, String) {
    let (base, keys) = spawn_server_multi(dir, 1).await;
    (base, keys.into_iter().next().unwrap())
}

/// 拉起服务并创建多个租户（同一运行实例，多池形态），返回（地址，各租户 service-key）
async fn spawn_server_multi(dir: &Path, tenants: usize) -> (String, Vec<String>) {
    let (base, keys) = spawn_server_full(dir, tenants, false).await;
    (base, keys)
}

/// 拉起提供中继的服务：connect.relay 随会话下发（url + token）
async fn spawn_server_with_relay(dir: &Path) -> (String, String) {
    let (base, keys) = spawn_server_full(dir, 1, true).await;
    (base, keys.into_iter().next().unwrap())
}

async fn spawn_server_full(dir: &Path, tenants: usize, with_relay: bool) -> (String, Vec<String>) {
    let keys = KeyStore::open(dir).unwrap();
    let service_keys: Vec<String> = (0..tenants)
        .map(|i| keys.tenant_add(&format!("pool-{i}")).unwrap().1)
        .collect();
    let mut state = AppState::new(Arc::new(keys), Store::load(dir).unwrap());
    if with_relay {
        state.relay_info = Some(json!({
            "url": "http://127.0.0.1:9102",
            "token": "test-relay-token",
        }));
    }
    let state = Arc::new(state);
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
    (format!("127.0.0.1:{port}"), service_keys)
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

async fn register(base: &str, key: &str, name: &str, allow_remote: bool) -> String {
    let resp: Value = client()
        .post(format!("http://{base}/api/v1/devices"))
        .bearer_auth(key)
        .json(&json!({
            "name": name,
            "platform": "macos",
            "endpoint_id": format!("ep-{name}"),
            "library_name": "我的素材库",
            "allow_remote": allow_remote,
            "allow_import": false,
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

async fn heartbeat(base: &str, key: &str, device_id: &str, addrs: Value) -> reqwest::Response {
    client()
        .post(format!("http://{base}/api/v1/devices/heartbeat"))
        .bearer_auth(key)
        .json(&json!({ "device_id": device_id, "addrs": addrs }))
        .send()
        .await
        .unwrap()
}

fn full_addrs() -> Value {
    json!(["192.168.1.5:27373", "1.2.3.4:27373"])
}

fn empty_addrs() -> Value {
    json!([])
}

/// WSS 控制通道连接（key 走 Authorization 头，不走 URL）
async fn connect_ws(base: &str, key: &str, device_id: &str) -> Ws {
    let mut req = format!("ws://{base}/api/v1/ws?device_id={device_id}")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("Authorization", format!("Bearer {key}").parse().unwrap());
    let (ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
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

async fn create_session(
    base: &str,
    key: &str,
    source: &str,
    target: &str,
) -> (reqwest::StatusCode, Value) {
    let resp = client()
        .post(format!("http://{base}/api/v1/sessions"))
        .bearer_auth(key)
        .json(&json!({
            "source_device_id": source,
            "target_device_id": target,
        }))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    (status, resp.json().await.unwrap())
}

async fn session_state(base: &str, key: &str, session_id: &str) -> Value {
    let resp: Value = client()
        .get(format!("http://{base}/api/v1/sessions/{session_id}"))
        .bearer_auth(key)
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
    let (base, key) = spawn_server(dir.path()).await;
    let a = register(&base, &key, "家里台式机", true).await;
    let b = register(&base, &key, "笔记本", true).await;

    let resp: Value = client()
        .get(format!("http://{base}/api/v1/devices"))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let devices = resp["data"]["devices"].as_array().unwrap();
    // 租户列表只含本池设备：全局的「网页版」服务端设备不在其列
    assert_eq!(devices.len(), 2);
    assert!(devices.iter().all(|d| d["platform"] != "server"));
    assert_eq!(devices[0]["online"], false);
    assert_eq!(devices[0]["last_seen_at_ms"], Value::Null);

    let resp = heartbeat(&base, &key, &a, full_addrs()).await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["data"]["public_ip_seen"], "127.0.0.1");

    let resp: Value = client()
        .get(format!("http://{base}/api/v1/devices"))
        .bearer_auth(&key)
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
    assert_eq!(da["addrs"], json!(["192.168.1.5:27373", "1.2.3.4:27373"]));
    let db = &devices[1];
    assert_eq!(db["device_id"], b);
    assert_eq!(db["online"], false);

    // 设备身份跨库共享：心跳携带 library_name，服务端设备表随之更新
    let resp = client()
        .post(format!("http://{base}/api/v1/devices/heartbeat"))
        .bearer_auth(&key)
        .json(&json!({
            "device_id": b,
            "library_name": "改名后的库",
            "reachability": full_addrs(),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let resp: Value = client()
        .get(format!("http://{base}/api/v1/devices"))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let db = resp["data"]["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["device_id"] == b)
        .unwrap();
    assert_eq!(db["library_name"], "改名后的库");

    // 反代部署：X-Real-IP / X-Forwarded-For 覆盖连接源地址（直连候选依赖真实公网 IP）
    let c = register(&base, &key, "第三台", true).await;
    let d = register(&base, &key, "第四台", true).await;
    let resp = client()
        .post(format!("http://{base}/api/v1/devices/heartbeat"))
        .bearer_auth(&key)
        .header("x-real-ip", "1.2.3.4")
        .json(&json!({ "device_id": c, "reachability": full_addrs() }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["data"]["public_ip_seen"], "1.2.3.4");

    let resp = client()
        .post(format!("http://{base}/api/v1/devices/heartbeat"))
        .bearer_auth(&key)
        .header("x-forwarded-for", "5.6.7.8, 10.0.0.1")
        .json(&json!({ "device_id": d, "reachability": full_addrs() }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["data"]["public_ip_seen"], "5.6.7.8");
}

#[tokio::test]
async fn auth_required_on_all_endpoints() {
    let dir = tempfile::tempdir().unwrap();
    let (base, key) = spawn_server(dir.path()).await;

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
        .bearer_auth("hs_wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // WSS 错误 key：握手被拒
    let device = register(&base, &key, "台式机", true).await;
    let mut req = format!("ws://{base}/api/v1/ws?device_id={device}")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("Authorization", "Bearer hs_wrong".parse().unwrap());
    assert!(tokio_tungstenite::connect_async(req).await.is_err());

    // WSS 未注册设备
    let mut req = format!("ws://{base}/api/v1/ws?device_id=d_x")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("Authorization", format!("Bearer {key}").parse().unwrap());
    assert!(tokio_tungstenite::connect_async(req).await.is_err());
}

#[tokio::test]
async fn tenant_isolation() {
    let dir = tempfile::tempdir().unwrap();
    let (base, keys) = spawn_server_multi(dir.path(), 2).await;
    let key_a = keys[0].clone();
    let key_b = keys[1].clone();
    let device_a = register(&base, &key_a, "A 的台式机", true).await;
    let device_b = register(&base, &key_b, "B 的笔记本", true).await;

    // B 池列表看不到 A 的设备
    let resp: Value = client()
        .get(format!("http://{base}/api/v1/devices"))
        .bearer_auth(&key_b)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let devices = resp["data"]["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0]["device_id"], device_b);

    // B 池 key 操作 A 的设备：一律 404（不泄露他池存在性）
    let resp = client()
        .patch(format!("http://{base}/api/v1/devices/{device_a}"))
        .bearer_auth(&key_b)
        .json(&json!({ "name": "劫持" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let resp = heartbeat(&base, &key_b, &device_a, full_addrs()).await;
    assert_eq!(resp.status(), 404);

    // 跨池会话：B 向 A 的设备发起 → 404
    let (status, _) = create_session(&base, &key_b, &device_b, &device_a).await;
    assert_eq!(status, 404);

    // 吊销租户 A（模拟 CLI 独立进程操作）：注册表以文件为事实源，运行实例即刻感知
    let tenants = KeyStore::open(dir.path()).unwrap().tenant_list();
    let tenant_a = tenants
        .iter()
        .find(|t| t.label == "pool-0")
        .unwrap()
        .id
        .clone();
    KeyStore::open(dir.path())
        .unwrap()
        .tenant_revoke(&tenant_a)
        .unwrap();
    let resp = client()
        .get(format!("http://{base}/api/v1/devices"))
        .bearer_auth(&key_a)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "吊销后 service-key 应立即失效");
    // B 池不受影响
    let resp = client()
        .get(format!("http://{base}/api/v1/devices"))
        .bearer_auth(&key_b)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn device_keys_sync_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let (base, key) = spawn_server(dir.path()).await;
    let device = register(&base, &key, "台式机", true).await;

    // 同步两条 key
    let s1 = hawk_server::keystore::sha256_hex("read-key-1");
    let s2 = hawk_server::keystore::sha256_hex("write-key-1");
    let resp = client()
        .put(format!("http://{base}/api/v1/devices/{device}/keys"))
        .bearer_auth(&key)
        .json(&json!({ "keys": [
            { "s": s1, "role": "viewer" },
            { "s": s2, "role": "importer" },
        ] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // 非法条目整体拒绝（s 不是 sha256 十六进制）
    let resp = client()
        .put(format!("http://{base}/api/v1/devices/{device}/keys"))
        .bearer_auth(&key)
        .json(&json!({ "keys": [ { "s": "short", "role": "viewer" } ] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // 他设备 → 404
    let resp = client()
        .put(format!("http://{base}/api/v1/devices/d_x/keys"))
        .bearer_auth(&key)
        .json(&json!({ "keys": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn heartbeat_throttled_below_min_interval() {
    let dir = tempfile::tempdir().unwrap();
    let (base, key) = spawn_server(dir.path()).await;
    let device = register(&base, &key, "台式机", true).await;

    let resp = heartbeat(&base, &key, &device, full_addrs()).await;
    assert_eq!(resp.status(), 200);
    // 协议限速：心跳最小间隔 10s
    let resp = heartbeat(&base, &key, &device, full_addrs()).await;
    assert_eq!(resp.status(), 429);
}

#[tokio::test]
async fn session_direct_flow_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let (base, key) = spawn_server(dir.path()).await;
    let target = register(&base, &key, "家里台式机", true).await;
    let source = register(&base, &key, "笔记本", true).await;
    assert_eq!(
        heartbeat(&base, &key, &target, full_addrs()).await.status(),
        200
    );

    let mut target_ws = connect_ws(&base, &key, &target).await;
    let hello = next_json(&mut target_ws).await;
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["device_id"], target);

    // 发起会话 → 目标经 WSS 收到 session.request（app 间会话不带 credential）
    let (status, body) = create_session(&base, &key, &source, &target).await;
    assert_eq!(status, 200);
    assert_eq!(body["data"]["state"], "pending");
    let session_id = body["data"]["session_id"].as_str().unwrap().to_string();

    let request = next_json(&mut target_ws).await;
    assert_eq!(request["type"], "session.request");
    assert_eq!(request["session_id"], session_id);
    assert_eq!(request["from"]["device_id"], source);
    assert_eq!(request["from"]["name"], "笔记本");
    assert_eq!(request["credential"], Value::Null);
    assert!(request["from"]["endpoint_id"].is_string());

    // 目标接受 → 双方收到 session.update(ready)：endpoint_id + 直连地址（无中继部署无 relay 字段）
    let resp = client()
        .post(format!("http://{base}/api/v1/sessions/{session_id}/accept"))
        .bearer_auth(&key)
        .json(&json!({ "device_id": target }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let update = next_json(&mut target_ws).await;
    assert_eq!(update["type"], "session.update");
    assert_eq!(update["state"], "ready");
    assert_eq!(update["connect"]["endpoint_id"], "ep-家里台式机");
    assert_eq!(
        update["connect"]["direct_addresses"],
        json!(["192.168.1.5:27373", "1.2.3.4:27373"])
    );
    assert!(update["connect"].get("relay").is_none());

    // 轮询兜底看到同一状态
    let state = session_state(&base, &key, &session_id).await;
    assert_eq!(state["state"], "ready");
    assert_eq!(state["connect"]["endpoint_id"], "ep-家里台式机");
}

#[tokio::test]
async fn session_rejected_when_allow_remote_false() {
    let dir = tempfile::tempdir().unwrap();
    let (base, key) = spawn_server(dir.path()).await;
    let target = register(&base, &key, "台式机", false).await;
    let source = register(&base, &key, "笔记本", true).await;
    assert_eq!(
        heartbeat(&base, &key, &target, full_addrs()).await.status(),
        200
    );

    let (status, body) = create_session(&base, &key, &source, &target).await;
    assert_eq!(status, 200);
    assert_eq!(body["data"]["state"], "rejected");
}

#[tokio::test]
async fn session_requires_target_online() {
    let dir = tempfile::tempdir().unwrap();
    let (base, key) = spawn_server(dir.path()).await;
    let target = register(&base, &key, "台式机", true).await;
    let source = register(&base, &key, "笔记本", true).await;
    // 目标从未心跳 → 不在线
    let (status, body) = create_session(&base, &key, &source, &target).await;
    assert_eq!(status, 404);
    assert_eq!(body["status"], "error");
}

#[tokio::test]
async fn session_reject_flow() {
    let dir = tempfile::tempdir().unwrap();
    let (base, key) = spawn_server(dir.path()).await;
    let target = register(&base, &key, "台式机", true).await;
    let source = register(&base, &key, "笔记本", true).await;
    assert_eq!(
        heartbeat(&base, &key, &target, full_addrs()).await.status(),
        200
    );

    let (_, body) = create_session(&base, &key, &source, &target).await;
    let session_id = body["data"]["session_id"].as_str().unwrap().to_string();

    let resp = client()
        .post(format!("http://{base}/api/v1/sessions/{session_id}/reject"))
        .bearer_auth(&key)
        .json(&json!({ "device_id": target }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        session_state(&base, &key, &session_id).await["state"],
        "rejected"
    );

    // 终态不可再响应
    let resp = client()
        .post(format!("http://{base}/api/v1/sessions/{session_id}/accept"))
        .bearer_auth(&key)
        .json(&json!({ "device_id": target }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);
}

#[tokio::test]
async fn session_unreachable_when_no_candidates() {
    let dir = tempfile::tempdir().unwrap();
    let (base, key) = spawn_server(dir.path()).await;
    let target = register(&base, &key, "台式机", true).await;
    let source = register(&base, &key, "笔记本", true).await;
    assert_eq!(
        heartbeat(&base, &key, &target, empty_addrs())
            .await
            .status(),
        200
    );

    let (_, body) = create_session(&base, &key, &source, &target).await;
    let session_id = body["data"]["session_id"].as_str().unwrap().to_string();
    let resp = client()
        .post(format!("http://{base}/api/v1/sessions/{session_id}/accept"))
        .bearer_auth(&key)
        .json(&json!({ "device_id": target }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        session_state(&base, &key, &session_id).await["state"],
        "unreachable"
    );
}

#[tokio::test]
async fn session_cancel_notifies_peer() {
    let dir = tempfile::tempdir().unwrap();
    let (base, key) = spawn_server(dir.path()).await;
    let target = register(&base, &key, "台式机", true).await;
    let source = register(&base, &key, "笔记本", true).await;
    assert_eq!(
        heartbeat(&base, &key, &target, full_addrs()).await.status(),
        200
    );

    let mut target_ws = connect_ws(&base, &key, &target).await;
    next_json(&mut target_ws).await; // hello

    let (_, body) = create_session(&base, &key, &source, &target).await;
    let session_id = body["data"]["session_id"].as_str().unwrap().to_string();
    next_json(&mut target_ws).await; // session.request

    let resp = client()
        .post(format!("http://{base}/api/v1/sessions/{session_id}/cancel"))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let cancelled = next_json(&mut target_ws).await;
    assert_eq!(cancelled["type"], "session.cancelled");
    assert_eq!(cancelled["session_id"], session_id);
    assert_eq!(
        session_state(&base, &key, &session_id).await["state"],
        "cancelled"
    );
}

#[tokio::test]
async fn connect_carries_relay_when_server_offers_it() {
    // 服务端提供中继（relay_info 配置）→ ready 的 connect 携带 relay（url + 准入 token）；
    // 未提供时无 relay 字段（纯直连部署）
    let dir = tempfile::tempdir().unwrap();
    let (base, key) = spawn_server_with_relay(dir.path()).await;
    let target = register(&base, &key, "台式机", true).await;
    let source = register(&base, &key, "笔记本", true).await;
    assert_eq!(
        heartbeat(&base, &key, &target, full_addrs()).await.status(),
        200
    );

    let (_, body) = create_session(&base, &key, &source, &target).await;
    let session_id = body["data"]["session_id"].as_str().unwrap().to_string();
    let resp = client()
        .post(format!("http://{base}/api/v1/sessions/{session_id}/accept"))
        .bearer_auth(&key)
        .json(&json!({ "device_id": target }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let state = session_state(&base, &key, &session_id).await;
    assert_eq!(state["state"], "ready");
    assert_eq!(state["connect"]["relay"]["url"], "http://127.0.0.1:9102");
    assert_eq!(state["connect"]["relay"]["token"], "test-relay-token");
    assert_eq!(state["connect"]["endpoint_id"], "ep-台式机");
    assert_eq!(
        state["connect"]["direct_addresses"],
        json!(["192.168.1.5:27373", "1.2.3.4:27373"])
    );

    // 对照：不提供中继的部署无 relay 字段
    let dir = tempfile::tempdir().unwrap();
    let (base, key) = spawn_server(dir.path()).await;
    let target = register(&base, &key, "台式机", true).await;
    let source = register(&base, &key, "笔记本", true).await;
    assert_eq!(
        heartbeat(&base, &key, &target, full_addrs()).await.status(),
        200
    );
    let (_, body) = create_session(&base, &key, &source, &target).await;
    let session_id = body["data"]["session_id"].as_str().unwrap().to_string();
    client()
        .post(format!("http://{base}/api/v1/sessions/{session_id}/accept"))
        .bearer_auth(&key)
        .json(&json!({ "device_id": target }))
        .send()
        .await
        .unwrap();
    let state = session_state(&base, &key, &session_id).await;
    assert!(state["connect"].get("relay").is_none());
}

#[tokio::test]
async fn admin_endpoints_manage_tenants() {
    let dir = tempfile::tempdir().unwrap();
    let (base, _seed_key) = spawn_server(dir.path()).await;
    // 管理面路由挂在独立监听口（同进程，共享同一 KeyStore 实例语义——文件即事实源）
    let admin_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let admin_port = admin_listener.local_addr().unwrap().port();
    let admin_keys = Arc::new(KeyStore::open(dir.path()).unwrap());
    tokio::spawn(async move {
        axum::serve(admin_listener, hawk_server::admin::router(admin_keys))
            .await
            .unwrap();
    });
    let admin = format!("127.0.0.1:{admin_port}");

    // 创建租户
    let resp: Value = client()
        .post(format!("http://{admin}/api/v1/admin/tenants"))
        .json(&json!({ "label": "ops" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp["status"], "success", "创建失败: {resp}");
    let tenant_id = resp["data"]["id"].as_str().unwrap().to_string();
    let service_key = resp["data"]["service_key"].as_str().unwrap().to_string();
    assert!(service_key.starts_with("hs_"));

    // 创建的 key 立即可用于主服务（跨监听口共享注册表）
    let device = register(&base, &service_key, "管理面创建设备", true).await;
    assert!(device.starts_with("d_"));

    // 查询（不含密钥本体）
    let resp: Value = client()
        .get(format!("http://{admin}/api/v1/admin/tenants"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tenants = resp["data"]["tenants"].as_array().unwrap();
    let entry = tenants.iter().find(|t| t["id"] == tenant_id).unwrap();
    assert_eq!(entry["label"], "ops");
    assert!(entry.get("service_key").is_none(), "列表不得包含密钥本体");

    // 吊销 → 主服务侧立即 401
    let resp = client()
        .delete(format!("http://{admin}/api/v1/admin/tenants/{tenant_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let resp = client()
        .get(format!("http://{base}/api/v1/devices"))
        .bearer_auth(&service_key)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // 吊销不存在的租户 → 404
    let resp = client()
        .delete(format!("http://{admin}/api/v1/admin/tenants/{tenant_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn device_table_persists_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (base, key) = spawn_server(dir.path()).await;
    register(&base, &key, "台式机", true).await;

    // 模拟重启：同一数据目录重新加载（原实例仍在跑，load 只读不冲突）。
    // KeyStore 重新 open 共享同一 tenants.json，原 service-key 仍有效
    let (base2, _) = spawn_server(dir.path()).await;
    let resp: Value = client()
        .get(format!("http://{base2}/api/v1/devices"))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let devices = resp["data"]["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 1); // 本池已注册设备（网页版为全局设备，不在租户列表）
    assert_eq!(devices[0]["name"], "台式机");
    assert_eq!(devices[0]["online"], false);
}

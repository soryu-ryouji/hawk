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
    let (base, keys, _) = spawn_server_full(dir, tenants, false).await;
    (base, keys)
}

/// 拉起带打洞探测口（P3）的服务：返回（地址，service-key，探测口端口）
async fn spawn_server_with_probe(dir: &Path) -> (String, String, u16) {
    let (base, keys, probe_port) = spawn_server_full(dir, 1, true).await;
    (base, keys.into_iter().next().unwrap(), probe_port.unwrap())
}

async fn spawn_server_full(
    dir: &Path,
    tenants: usize,
    with_probe: bool,
) -> (String, Vec<String>, Option<u16>) {
    let keys = KeyStore::open(dir).unwrap();
    let service_keys: Vec<String> = (0..tenants)
        .map(|i| keys.tenant_add(&format!("pool-{i}")).unwrap().1)
        .collect();
    let mut state = AppState::new(Arc::new(keys), Store::load(dir).unwrap());
    // 打洞探测口：与生产一致的启动路径（绑定成功才启用编排）
    let probe_socket = if with_probe {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        state.probe_port = Some(socket.local_addr().unwrap().port());
        Some(socket)
    } else {
        None
    };
    let probe_port = state.probe_port;
    let state = Arc::new(state);
    if let Some(socket) = probe_socket {
        tokio::spawn(hawk_server::probe::run(state.clone(), socket));
    }
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
            api::router(state, viewer).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (format!("127.0.0.1:{port}"), service_keys, probe_port)
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
            "fingerprint": format!("fp-{name}"),
            "library_name": "我的素材库",
            "allow_remote": allow_remote,
            "allow_write": false,
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

async fn heartbeat(
    base: &str,
    key: &str,
    device_id: &str,
    reachability: Value,
) -> reqwest::Response {
    client()
        .post(format!("http://{base}/api/v1/devices/heartbeat"))
        .bearer_auth(key)
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

/// 读到指定 type 为止（打洞编排会穿插 pending 的 session.update）
async fn next_json_of(ws: &mut Ws, want: &str) -> Value {
    for _ in 0..8 {
        let msg = next_json(ws).await;
        if msg["type"] == want {
            return msg;
        }
    }
    panic!("未等到 {want} 消息")
}

/// 向探测口报到（P3 客户端行为：HAWKP + token + 零填充定长 64B），返回响应原文
async fn udp_probe(socket: &tokio::net::UdpSocket, probe_port: u16, token_hex: &str) -> Vec<u8> {
    let token: Vec<u8> = (0..token_hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&token_hex[i..i + 2], 16).unwrap())
        .collect();
    assert_eq!(token.len(), 16);
    let mut packet = [0u8; 64];
    packet[..5].copy_from_slice(b"HAWKP");
    packet[5..21].copy_from_slice(&token);
    socket
        .send_to(&packet, ("127.0.0.1", probe_port))
        .await
        .unwrap();
    let mut buf = [0u8; 128];
    let (len, _) = tokio::time::timeout(Duration::from_secs(2), socket.recv_from(&mut buf))
        .await
        .expect("探测响应超时")
        .unwrap();
    buf[..len].to_vec()
}

/// 轮询会话状态直到目标态（打洞编排水下 8s 上限，测试用短上限轮询）
async fn wait_session_state(base: &str, key: &str, session_id: &str, want: &str) -> Value {
    for _ in 0..50 {
        let state = session_state(base, key, session_id).await;
        if state["state"] == want {
            return state;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("会话未在预期时间内进入 {want}")
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
            "mode": "direct",
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

    let resp = heartbeat(&base, &key, &a, full_reachability()).await;
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
    assert_eq!(da["reachability"]["public_ip"], "127.0.0.1");
    assert_eq!(da["reachability"]["mapped_port"], 27373);
    assert_eq!(da["reachability"]["public_ipv6"], "240e::1");
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
            "reachability": full_reachability(),
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
        .json(&json!({ "device_id": c, "reachability": full_reachability() }))
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
        .json(&json!({ "device_id": d, "reachability": full_reachability() }))
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
    let resp = heartbeat(&base, &key_b, &device_a, full_reachability()).await;
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
            { "s": s1, "read": true, "write": false },
            { "s": s2, "read": true, "write": true },
        ] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // 非法条目整体拒绝（s 不是 sha256 十六进制）
    let resp = client()
        .put(format!("http://{base}/api/v1/devices/{device}/keys"))
        .bearer_auth(&key)
        .json(&json!({ "keys": [ { "s": "short", "read": true } ] }))
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

    let resp = heartbeat(&base, &key, &device, full_reachability()).await;
    assert_eq!(resp.status(), 200);
    // 协议限速：心跳最小间隔 10s
    let resp = heartbeat(&base, &key, &device, full_reachability()).await;
    assert_eq!(resp.status(), 429);
}

#[tokio::test]
async fn session_direct_flow_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let (base, key) = spawn_server(dir.path()).await;
    let target = register(&base, &key, "家里台式机", true).await;
    let source = register(&base, &key, "笔记本", true).await;
    assert_eq!(
        heartbeat(&base, &key, &target, full_reachability())
            .await
            .status(),
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
    assert_eq!(request["mode"], "direct");
    assert_eq!(request["credential"], Value::Null);

    // 目标接受 → 双方收到 session.update(ready)，候选 IPv4 映射优先、IPv6 次之
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
    let candidates = update["connect"]["candidates"].as_array().unwrap();
    assert_eq!(candidates[0]["transport"], "ipv4");
    assert_eq!(candidates[0]["host"], "127.0.0.1");
    assert_eq!(candidates[0]["port"], 27373);
    assert_eq!(candidates[1]["transport"], "ipv6");
    assert_eq!(candidates[1]["host"], "240e::1");
    assert_eq!(update["connect"]["fingerprint"], "fp-家里台式机");

    // 轮询兜底看到同一状态
    let state = session_state(&base, &key, &session_id).await;
    assert_eq!(state["state"], "ready");
    assert_eq!(state["connect"]["mode"], "direct");
}

#[tokio::test]
async fn lan_candidate_first_when_same_public_ip() {
    // 双方心跳来自同一连接源（loopback 同为 127.0.0.1，同公网 IP 的撮合信号）且目标
    // 上报 LAN 地址 → LAN 候选排最前（同局域网直连，绕开 hairpin）
    let dir = tempfile::tempdir().unwrap();
    let (base, key) = spawn_server(dir.path()).await;
    let target = register(&base, &key, "台式机", true).await;
    let source = register(&base, &key, "笔记本", true).await;
    let reachability = json!({
        "public_ip": null,
        "mapped_port": 27373,
        "mapping_type": "upnp",
        "public_ipv6": null,
        "ipv6_port": null,
        "lan_ipv4": "192.168.1.5",
    });
    assert_eq!(
        heartbeat(&base, &key, &target, reachability.clone())
            .await
            .status(),
        200
    );
    assert_eq!(
        heartbeat(&base, &key, &source, reachability).await.status(),
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
    let candidates = state["connect"]["candidates"].as_array().unwrap();
    assert_eq!(candidates[0]["transport"], "lan");
    assert_eq!(candidates[0]["host"], "192.168.1.5");
    assert_eq!(candidates[0]["port"], 27373); // ipv6_port 未上报 → 默认 QUIC 端口
    assert_eq!(candidates[1]["transport"], "ipv4");
}

#[tokio::test]
async fn session_rejected_when_allow_remote_false() {
    let dir = tempfile::tempdir().unwrap();
    let (base, key) = spawn_server(dir.path()).await;
    let target = register(&base, &key, "台式机", false).await;
    let source = register(&base, &key, "笔记本", true).await;
    assert_eq!(
        heartbeat(&base, &key, &target, full_reachability())
            .await
            .status(),
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
        heartbeat(&base, &key, &target, full_reachability())
            .await
            .status(),
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
        heartbeat(&base, &key, &target, empty_reachability())
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
async fn punch_orchestration_ready_when_both_probe() {
    let dir = tempfile::tempdir().unwrap();
    let (base, key, probe_port) = spawn_server_with_probe(dir.path()).await;
    let target = register(&base, &key, "台式机", true).await;
    let source = register(&base, &key, "笔记本", true).await;
    assert_eq!(
        heartbeat(&base, &key, &target, empty_reachability())
            .await
            .status(),
        200
    );
    // session.probe 走 WSS 推送，双方控制通道都需在线
    let mut target_ws = connect_ws(&base, &key, &target).await;
    let mut source_ws = connect_ws(&base, &key, &source).await;
    assert_eq!(next_json(&mut target_ws).await["type"], "hello");
    assert_eq!(next_json(&mut source_ws).await["type"], "hello");

    let (_, body) = create_session(&base, &key, &source, &target).await;
    let session_id = body["data"]["session_id"].as_str().unwrap().to_string();
    let request = next_json_of(&mut target_ws, "session.request").await;
    assert_eq!(request["session_id"], session_id);

    let resp = client()
        .post(format!("http://{base}/api/v1/sessions/{session_id}/accept"))
        .bearer_auth(&key)
        .json(&json!({ "device_id": target }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    // 打洞编排中：会话保持 pending，双方收到各自的 session.probe（token 按角色独立）
    assert_eq!(
        session_state(&base, &key, &session_id).await["state"],
        "pending"
    );
    let probe_a = next_json_of(&mut target_ws, "session.probe").await;
    let probe_b = next_json_of(&mut source_ws, "session.probe").await;
    assert_eq!(probe_a["session_id"], session_id);
    assert_eq!(probe_a["probe_port"], probe_port);
    assert_eq!(probe_b["probe_port"], probe_port);
    assert_ne!(probe_a["token"], probe_b["token"]);

    // 双方从各自 socket 报到：响应回告 observed endpoint（loopback 上即本机端口）
    let socket_a = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let socket_b = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let resp_a = udp_probe(&socket_a, probe_port, probe_a["token"].as_str().unwrap()).await;
    assert_eq!(&resp_a[..5], b"HAWKR");
    assert_eq!(resp_a[21], 4); // IPv4
    assert_eq!(
        u16::from_be_bytes([resp_a[26], resp_a[27]]),
        socket_a.local_addr().unwrap().port()
    );
    let _ = udp_probe(&socket_b, probe_port, probe_b["token"].as_str().unwrap()).await;

    // 到齐 → ready：punch 候选 = 目标 observed endpoint；punch_peer = 发起方 observed endpoint
    let update = next_json_of(&mut target_ws, "session.update").await;
    assert_eq!(update["state"], "ready");
    let candidates = update["connect"]["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0]["transport"], "punch");
    assert_eq!(candidates[0]["host"], "127.0.0.1");
    assert_eq!(candidates[0]["port"], socket_a.local_addr().unwrap().port());
    assert_eq!(
        update["connect"]["punch_peer"]["port"],
        socket_b.local_addr().unwrap().port()
    );
    assert_eq!(update["connect"]["fingerprint"], "fp-台式机");

    // 轮询兜底看到同一状态
    let state = session_state(&base, &key, &session_id).await;
    assert_eq!(state["state"], "ready");
    assert_eq!(state["connect"]["candidates"][0]["transport"], "punch");
}

#[tokio::test]
async fn punch_orchestration_timeout_unreachable() {
    // 探测永不到达（探测口被拦/UDP 丢失）：编排超时置 unreachable（与编排出现前同语义）
    std::env::set_var("HAWK_SERVER_PROBE_WAIT_MS", "800");
    let dir = tempfile::tempdir().unwrap();
    let (base, key, _probe_port) = spawn_server_with_probe(dir.path()).await;
    let target = register(&base, &key, "台式机", true).await;
    let source = register(&base, &key, "笔记本", true).await;
    assert_eq!(
        heartbeat(&base, &key, &target, empty_reachability())
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
    // 双方都不探测 → 超时置 unreachable
    let state = wait_session_state(&base, &key, &session_id, "unreachable").await;
    assert_eq!(state["state"], "unreachable");
}

#[tokio::test]
async fn punch_candidate_appended_when_direct_exists() {
    // 直连候选可能存在但不可用（CGNAT 内层 UPnP、被拦的 IPv6）：探测到齐后 punch 候选
    // 附加在队尾完底，punch_peer 照常下发
    let dir = tempfile::tempdir().unwrap();
    let (base, key, probe_port) = spawn_server_with_probe(dir.path()).await;
    let target = register(&base, &key, "台式机", true).await;
    let source = register(&base, &key, "笔记本", true).await;
    assert_eq!(
        heartbeat(&base, &key, &target, full_reachability())
            .await
            .status(),
        200
    );
    let mut target_ws = connect_ws(&base, &key, &target).await;
    let mut source_ws = connect_ws(&base, &key, &source).await;
    assert_eq!(next_json(&mut target_ws).await["type"], "hello");
    assert_eq!(next_json(&mut source_ws).await["type"], "hello");

    let (_, body) = create_session(&base, &key, &source, &target).await;
    let session_id = body["data"]["session_id"].as_str().unwrap().to_string();
    let _ = next_json_of(&mut target_ws, "session.request").await;
    let resp = client()
        .post(format!("http://{base}/api/v1/sessions/{session_id}/accept"))
        .bearer_auth(&key)
        .json(&json!({ "device_id": target }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let probe_a = next_json_of(&mut target_ws, "session.probe").await;
    let probe_b = next_json_of(&mut source_ws, "session.probe").await;
    let socket_a = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let socket_b = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let _ = udp_probe(&socket_a, probe_port, probe_a["token"].as_str().unwrap()).await;
    let _ = udp_probe(&socket_b, probe_port, probe_b["token"].as_str().unwrap()).await;

    let update = next_json_of(&mut target_ws, "session.update").await;
    assert_eq!(update["state"], "ready");
    let candidates = update["connect"]["candidates"].as_array().unwrap();
    // ipv4 → ipv6 → punch（协议约定顺序，punch 排最后）
    assert_eq!(candidates.len(), 3);
    assert_eq!(candidates[0]["transport"], "ipv4");
    assert_eq!(candidates[1]["transport"], "ipv6");
    assert_eq!(candidates[2]["transport"], "punch");
    assert_eq!(candidates[2]["port"], socket_a.local_addr().unwrap().port());
    assert_eq!(
        update["connect"]["punch_peer"]["port"],
        socket_b.local_addr().unwrap().port()
    );
}

#[tokio::test]
async fn direct_candidates_ready_when_probes_missing() {
    // 有直连候选而探测未到（旧客户端/UDP 被拦）：短等后按候选放行，不退化不阻塞
    std::env::set_var("HAWK_SERVER_PROBE_WAIT_SHORT_MS", "600");
    let dir = tempfile::tempdir().unwrap();
    let (base, key, _probe_port) = spawn_server_with_probe(dir.path()).await;
    let target = register(&base, &key, "台式机", true).await;
    let source = register(&base, &key, "笔记本", true).await;
    assert_eq!(
        heartbeat(&base, &key, &target, full_reachability())
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
    // 双方都不探测 → 短等超时 → ready 只含直连候选、无 punch_peer
    let state = wait_session_state(&base, &key, &session_id, "ready").await;
    let candidates = state["connect"]["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0]["transport"], "ipv4");
    assert!(state["connect"].get("punch_peer").is_none());
}

#[tokio::test]
async fn relay_path_attach_on_direct_session() {
    // P4：ready 的 direct 会话可附加中继路径；双方收 WSS 推送；relay WS 对该会话放行；
    // 附加路径断开不取消会话（UDP 路径可能仍在）
    use futures_util::{SinkExt, StreamExt};
    let dir = tempfile::tempdir().unwrap();
    let (base, key) = spawn_server(dir.path()).await;
    let target = register(&base, &key, "台式机", true).await;
    let source = register(&base, &key, "笔记本", true).await;
    assert_eq!(
        heartbeat(&base, &key, &target, full_reachability())
            .await
            .status(),
        200
    );
    let mut target_ws = connect_ws(&base, &key, &target).await;
    let mut source_ws = connect_ws(&base, &key, &source).await;
    assert_eq!(next_json(&mut target_ws).await["type"], "hello");
    assert_eq!(next_json(&mut source_ws).await["type"], "hello");

    let (_, body) = create_session(&base, &key, &source, &target).await;
    let session_id = body["data"]["session_id"].as_str().unwrap().to_string();
    let _ = next_json_of(&mut target_ws, "session.request").await;
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
        "ready"
    );

    // 附加前：relay WS 拒绝 direct 会话
    let resp = client()
        .get(format!(
            "http://{base}/api/v1/relay?device_id={target}&session_id={session_id}&role=a"
        ))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // 附加中继路径（幂等）
    for _ in 0..2 {
        let resp = client()
            .post(format!(
                "http://{base}/api/v1/sessions/{session_id}/relay-path"
            ))
            .bearer_auth(&key)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }
    // 双方收到 session.relay_path
    assert_eq!(
        next_json_of(&mut target_ws, "session.relay_path").await["session_id"],
        session_id
    );
    assert_eq!(
        next_json_of(&mut source_ws, "session.relay_path").await["session_id"],
        session_id
    );

    // relay WS 对已附加路径的 direct 会话放行，配对转发如常
    let mut ws_a = connect_ws_relay(&base, &key, &target, &session_id, "a").await;
    let mut ws_b = connect_ws_relay(&base, &key, &source, &session_id, "b").await;
    ws_a.send(tokio_tungstenite::tungstenite::Message::Binary(
        b"datagram".to_vec().into(),
    ))
    .await
    .unwrap();
    let echoed = tokio::time::timeout(Duration::from_secs(2), ws_b.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(echoed.into_data().as_ref(), b"datagram");

    // 附加路径断开：会话保持 ready（不取消）
    drop(ws_b);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        session_state(&base, &key, &session_id).await["state"],
        "ready"
    );

    // 终态会话附加被拒
    let resp = client()
        .post(format!("http://{base}/api/v1/sessions/{session_id}/cancel"))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let resp = client()
        .post(format!(
            "http://{base}/api/v1/sessions/{session_id}/relay-path"
        ))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);
}

#[tokio::test]
async fn session_cancel_notifies_peer() {
    let dir = tempfile::tempdir().unwrap();
    let (base, key) = spawn_server(dir.path()).await;
    let target = register(&base, &key, "台式机", true).await;
    let source = register(&base, &key, "笔记本", true).await;
    assert_eq!(
        heartbeat(&base, &key, &target, full_reachability())
            .await
            .status(),
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
async fn relay_session_ready_and_splice() {
    let dir = tempfile::tempdir().unwrap();
    let (base, key) = spawn_server(dir.path()).await;
    let target = register(&base, &key, "台式机", true).await;
    let source = register(&base, &key, "笔记本", true).await;
    // relay 模式不需要可达性候选
    assert_eq!(
        heartbeat(&base, &key, &target, empty_reachability())
            .await
            .status(),
        200
    );

    // mode=relay 发起 → 接受 → ready（connect 只带指纹、无候选）
    let resp = client()
        .post(format!("http://{base}/api/v1/sessions"))
        .bearer_auth(&key)
        .json(&json!({
            "source_device_id": source,
            "target_device_id": target,
            "mode": "relay",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
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
    assert_eq!(state["connect"]["mode"], "relay");
    assert_eq!(state["connect"]["fingerprint"], "fp-台式机");
    assert!(state["connect"]["candidates"].is_null());

    // 双方建立中继 WS（role=a 目标 / role=b 发起），二进制帧逐条转发
    let mut ws_a = connect_ws_relay(&base, &key, &target, &session_id, "a").await;
    let mut ws_b = connect_ws_relay(&base, &key, &source, &session_id, "b").await;

    use futures_util::{SinkExt, StreamExt};
    ws_b.send(tokio_tungstenite::tungstenite::Message::Binary(
        vec![1, 2, 3].into(),
    ))
    .await
    .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(2), ws_a.next())
        .await
        .expect("中继帧超时")
        .unwrap()
        .unwrap();
    assert_eq!(frame.into_data().as_ref(), &[1, 2, 3]);

    ws_a.send(tokio_tungstenite::tungstenite::Message::Binary(
        vec![4, 5].into(),
    ))
    .await
    .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(2), ws_b.next())
        .await
        .expect("中继帧超时")
        .unwrap()
        .unwrap();
    assert_eq!(frame.into_data().as_ref(), &[4, 5]);

    // 一方断开 → 会话置 cancelled
    drop(ws_a);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while session_state(&base, &key, &session_id).await["state"] != "cancelled" {
        assert!(
            tokio::time::Instant::now() < deadline,
            "会话未及时置 cancelled"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn connect_ws_relay(
    base: &str,
    key: &str,
    device_id: &str,
    session_id: &str,
    role: &str,
) -> Ws {
    let mut req = format!(
        "ws://{base}/api/v1/relay?device_id={device_id}&session_id={session_id}&role={role}"
    )
    .into_client_request()
    .unwrap();
    req.headers_mut()
        .insert("Authorization", format!("Bearer {key}").parse().unwrap());
    let (ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    ws
}

/// 管理面（环回监听口）：创建/查询/吊销租户全走 REST，与主服务共享同一注册表
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

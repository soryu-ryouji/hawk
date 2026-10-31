//! WSS 控制通道：服务端 → 设备的单向推送（hello/ping/session.*），设备 → 服务端只回
//! hello_ack/pong，业务动作一律走 REST（协议「控制通道」一节）。

use super::ApiError;
use crate::auth::DeviceAuth;
use crate::state::{now_ms, AppState};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// 保活：ping 30s 间隔，两次未回 pong 断开（协议约定）
const PING_INTERVAL: Duration = Duration::from_secs(30);
const PONG_TIMEOUT: Duration = Duration::from_secs(60);

/// `GET /api/v1/ws`（`Authorization: Bearer <device session>` 升级前鉴权）。
/// Device 身份从 Session 取得，不走 URL 查询串。
pub async fn control_channel(
    State(state): State<Arc<AppState>>,
    auth: DeviceAuth,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    Ok(ws.on_upgrade(move |socket| handle_socket(state, socket, auth.device_id)))
}

async fn handle_socket(state: Arc<AppState>, mut socket: WebSocket, device_id: String) {
    let conn_id = state.next_conn_id();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    // 注册发送端：同一设备的新连接顶替旧连接
    {
        let mut presence = state.presence.write().unwrap();
        let p = presence.entry(device_id.clone()).or_default();
        if let Some((_, old_tx)) = p.ws.take() {
            let _ = old_tx.send(Message::Close(None));
        }
        p.ws = Some((conn_id, tx));
    }

    let hello = json!({ "type": "hello", "device_id": device_id, "server_time_ms": now_ms() });
    if socket
        .send(Message::Text(hello.to_string().into()))
        .await
        .is_err()
    {
        remove_conn(&state, &device_id, conn_id);
        return;
    }

    let mut ping_tick = tokio::time::interval(PING_INTERVAL);
    ping_tick.tick().await; // interval 首次立即触发，跳过
    let mut last_pong = Instant::now();

    loop {
        tokio::select! {
            // 服务端推送（hello/ping/session.*）
            msg = rx.recv() => {
                let Some(msg) = msg else { break };
                let is_close = matches!(msg, Message::Close(_));
                if socket.send(msg).await.is_err() || is_close {
                    break;
                }
            }
            // 设备 → 服务端：只有 hello_ack / pong；未知 type 忽略（协议版本策略）
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(value) = serde_json::from_str::<Value>(&text) {
                            if value.get("type").and_then(|t| t.as_str()) == Some("pong") {
                                last_pong = Instant::now();
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
            _ = ping_tick.tick() => {
                if last_pong.elapsed() > PONG_TIMEOUT {
                    let _ = socket.send(Message::Close(None)).await;
                    break;
                }
                let ping = json!({ "type": "ping" });
                if socket.send(Message::Text(ping.to_string().into())).await.is_err() {
                    break;
                }
            }
        }
    }

    remove_conn(&state, &device_id, conn_id);
}

/// 断连清理：仅移除本代连接（可能已被新连接顶替）
fn remove_conn(state: &Arc<AppState>, device_id: &str, conn_id: u64) {
    let mut presence = state.presence.write().unwrap();
    if let Some(p) = presence.get_mut(device_id) {
        if p.ws.as_ref().is_some_and(|(id, _)| *id == conn_id) {
            p.ws = None;
        }
    }
}

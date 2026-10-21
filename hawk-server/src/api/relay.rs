//! 中继数据通道（协议「中继」一节）：双方各连一条 WS，配对后逐帧转发二进制帧
//! （一帧 = 一个 QUIC 数据报，不解析内容）；任一方断开 → 双方关闭，会话置 cancelled。

use super::ApiError;
use crate::auth::constant_time_eq;
use crate::state::{AppState, SessionState};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Deserialize)]
pub struct RelayQuery {
    key: String,
    device_id: String,
    session_id: String,
    role: String,
}

/// `GET /api/v1/relay?key&device_id&session_id&role=a|b`（升级前鉴权与角色校验）
pub async fn relay_channel(
    State(state): State<Arc<AppState>>,
    Query(query): Query<RelayQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    if !constant_time_eq(query.key.as_bytes(), state.access_key.as_bytes()) {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "访问密钥无效"));
    }
    if query.role != "a" && query.role != "b" {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "role 须为 a（目标设备）或 b（发起方）",
        ));
    }
    // 会话存在、ready、relay 模式、本机是该会话对应角色方
    {
        let sessions = state.sessions.read().unwrap();
        let Some(session) = sessions.get(&query.session_id) else {
            return Err(ApiError::not_found("会话不存在"));
        };
        if session.state != SessionState::Ready {
            return Err(ApiError::new(StatusCode::CONFLICT, "会话未就绪"));
        }
        if session.mode != "relay" {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "会话非中继模式"));
        }
        let matches = if query.role == "a" {
            session.target_device_id == query.device_id
        } else {
            session.source_device_id == query.device_id
        };
        if !matches {
            return Err(ApiError::new(StatusCode::FORBIDDEN, "设备与会话角色不符"));
        }
    }
    let role = query.role.clone();
    let session_id = query.session_id.clone();
    Ok(ws.on_upgrade(move |socket| handle_socket(state, socket, session_id, role)))
}

async fn handle_socket(
    state: Arc<AppState>,
    mut socket: WebSocket,
    session_id: String,
    role: String,
) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    // 注册发送端（同角色重连顶替旧连接）
    let paired = {
        let mut pairs = state.relay_pairs.write().unwrap();
        let pair = pairs.entry(session_id.clone()).or_default();
        let old = if role == "a" {
            pair.a.replace(tx.clone())
        } else {
            pair.b.replace(tx.clone())
        };
        if let Some(old) = old {
            let _ = old.send(Message::Close(None));
        }
        pair.paired()
    };
    if paired {
        tracing::info!("中继会话 {session_id} 配对完成，开始转发");
    }

    // 转发循环：出站（配对表 → 对端）与入站（本端 → 配对表对端）
    loop {
        tokio::select! {
            // 服务端推送/对端帧 → 本端 socket
            msg = rx.recv() => {
                let Some(msg) = msg else { break };
                let is_close = matches!(msg, Message::Close(_));
                if socket.send(msg).await.is_err() || is_close {
                    break;
                }
            }
            // 本端帧 → 写入配对表（由对端的出站分支取走）
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Binary(data))) => {
                        let peer = {
                            state
                                .relay_pairs
                                .read()
                                .unwrap()
                                .get(&session_id)
                                .and_then(|pair| pair.peer(&role).cloned())
                        };
                        match peer {
                            Some(tx) if tx.send(Message::Binary(data)).is_ok() => {}
                            // 对端未连接：丢弃（QUIC 重传会补上，不需要排队）
                            _ => {}
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    // 文本帧与协议层 ping/pong：忽略（协议约定）
                    _ => {}
                }
            }
        }
    }

    // 任一方断开：注销本端、关闭对端、会话置 cancelled 并通知双方控制通道
    let peer = {
        let mut pairs = state.relay_pairs.write().unwrap();
        if let Some(pair) = pairs.get_mut(&session_id) {
            pair.unregister(&role);
            pair.peer(&role).cloned()
        } else {
            None
        }
    };
    state.relay_pairs.write().unwrap().remove(&session_id);
    if let Some(peer_tx) = peer {
        let _ = peer_tx.send(Message::Close(None));
    }

    let (source, target) = {
        let mut sessions = state.sessions.write().unwrap();
        if let Some(session) = sessions.get_mut(&session_id) {
            if !session.state.is_terminal() {
                session.state = SessionState::Cancelled;
            }
            (
                session.source_device_id.clone(),
                session.target_device_id.clone(),
            )
        } else {
            return;
        }
    };
    let cancelled = json!({ "type": "session.cancelled", "session_id": session_id });
    state.push_ws(&source, &cancelled);
    state.push_ws(&target, &cancelled);
    tracing::info!("中继会话 {session_id} 结束（{role} 侧断开）");
}

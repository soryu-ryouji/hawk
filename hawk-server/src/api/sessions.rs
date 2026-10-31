//! 会话编排：发起、状态查询、接受/拒绝/取消。
//! Device 由 Session 认证；双方同租户，且读取/响应/取消必须是实际会话参与者。

use super::{ok, success, ApiError, JsonBody};
use crate::auth::DeviceAuth;
use crate::state::{now_ms, AppState, Role, Session, SessionState};
use axum::extract::{ConnectInfo, Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct CreateRequest {
    target_device_id: String,
    /// 请求的会话权限：read（缺省）/ write。app 间会话的写权限由目标设备的
    /// 「允许广域网保存素材」决定（单用户租户内设备互信），目标 daemon 会再本地执法一次
    #[serde(default)]
    role: Option<Role>,
}

fn new_session_id() -> String {
    format!("s_{:016x}", rand::random::<u64>())
}

/// `POST /api/v1/sessions`：发起 Device 间会话（Device Session，双方须同池）
pub async fn create(
    State(state): State<Arc<AppState>>,
    auth: DeviceAuth,
    JsonBody(req): JsonBody<CreateRequest>,
) -> Result<Json<Value>, ApiError> {
    for device_id in [&auth.device_id, &req.target_device_id] {
        match state.device(device_id) {
            Some(device) if device.tenant_id == auth.tenant_id => {}
            _ => return Err(ApiError::not_found("设备不存在")),
        }
    }
    let (session_id, initial) = create_session_internal(
        &state,
        &auth.device_id,
        &req.target_device_id,
        req.role.unwrap_or(Role::Viewer),
        false,
    )?;
    Ok(ok(
        json!({ "session_id": session_id, "state": initial.as_str() }),
    ))
}

/// 会话创建核心：校验双方与在线、插入会话、推送 session.request。
fn create_session_internal(
    state: &Arc<AppState>,
    source_device_id: &str,
    target_device_id: &str,
    role: Role,
    access_rendezvous: bool,
) -> Result<(String, SessionState), ApiError> {
    if source_device_id == target_device_id {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "不能与自身建立会话"));
    }
    if state.device(source_device_id).is_none() {
        return Err(ApiError::not_found("来源设备不存在"));
    }
    let Some(target) = state.device(target_device_id) else {
        return Err(ApiError::not_found("目标设备不存在"));
    };
    if !state.online(target_device_id) {
        return Err(ApiError::not_found("目标设备不在线"));
    }
    if role == Role::Importer && !target.allow_import {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "目标设备未开启广域网写入",
        ));
    }

    // allow_remote=false 直接置 rejected（协议约定，不打扰目标设备）
    let initial = if target.allow_remote {
        SessionState::Pending
    } else {
        SessionState::Rejected
    };
    let session = Session {
        session_id: new_session_id(),
        source_device_id: source_device_id.to_string(),
        target_device_id: target_device_id.to_string(),
        role,
        access_rendezvous,
        rendezvous_token: access_rendezvous.then(|| {
            format!(
                "rt_{:016x}{:016x}",
                rand::random::<u64>(),
                rand::random::<u64>()
            )
        }),
        state: initial,
        created_at_ms: now_ms(),
        connect: None,
    };
    let session_id = session.session_id.clone();
    state
        .sessions
        .write()
        .unwrap()
        .insert(session_id.clone(), session);

    if initial == SessionState::Pending {
        let source = state.device(source_device_id);
        state.push_ws(
            target_device_id,
            &json!({
                "type": "session.request",
                "session_id": session_id,
                "from": {
                    "device_id": source_device_id,
                    "name": source.as_ref().map(|s| s.name.as_str()).unwrap_or_default(),
                    "library_name": source.as_ref().map(|s| s.library_name.as_str()).unwrap_or_default(),
                    // 对端指纹随请求下发（QUIC 放行依据），省一次设备列表往返；
                    "endpoint_id": source.as_ref().map(|s| s.endpoint_id.as_str()).unwrap_or_default(),
                },
                "role": role.as_str(),
                "access_rendezvous": access_rendezvous,
            }),
        );
    }
    Ok((session_id, initial))
}

/// `GET /api/v1/sessions/{session_id}`：会话状态即全部结果语义（发起方轮询兜底，1s 间隔上限 30s）
pub async fn get(
    State(state): State<Arc<AppState>>,
    auth: DeviceAuth,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let mut sessions = state.sessions.write().unwrap();
    let Some(session) = sessions.get_mut(&session_id) else {
        return Err(ApiError::not_found("会话不存在"));
    };
    if session.source_device_id != auth.device_id && session.target_device_id != auth.device_id {
        return Err(ApiError::not_found("会话不存在"));
    }
    session.apply_expiry(now_ms());
    Ok(ok(session_view(session)))
}

/// `POST /api/v1/sessions/{session_id}/accept`：目标设备接受会话，服务端组装候选列表
pub async fn accept(
    State(state): State<Arc<AppState>>,
    auth: DeviceAuth,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    respond(state, auth, session_id, true)
}

/// `POST /api/v1/sessions/{session_id}/reject`：目标设备拒绝会话
pub async fn reject(
    State(state): State<Arc<AppState>>,
    auth: DeviceAuth,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    respond(state, auth, session_id, false)
}

fn respond(
    state: Arc<AppState>,
    auth: DeviceAuth,
    session_id: String,
    accepted: bool,
) -> Result<Json<Value>, ApiError> {
    let device_id = auth.device_id;
    // 目标设备须属调用者租户（跨池响应同回 404）
    match state.device(&device_id) {
        Some(device) if device.tenant_id == auth.tenant_id => {}
        _ => return Err(ApiError::not_found("设备不存在")),
    }
    let mut sessions = state.sessions.write().unwrap();
    let Some(session) = sessions.get_mut(&session_id) else {
        return Err(ApiError::not_found("会话不存在"));
    };
    session.apply_expiry(now_ms());
    if session.state != SessionState::Pending {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("会话已结束（{}）", session.state.as_str()),
        ));
    }
    if session.target_device_id != device_id {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "仅目标设备可响应会话"));
    }

    if !accepted {
        session.state = SessionState::Rejected;
    } else {
        // 连接信息 = iroh EndpointAddr 的控制面表达：endpoint_id + 直连地址 +（可选）中继。
        // 服务端不验证地址可达性（拨号方的连接尝试即最终校验）；
        // 无中继且无直连地址 → unreachable
        let endpoint_id = state
            .device(&session.target_device_id)
            .map(|d| d.endpoint_id)
            .unwrap_or_default();
        let direct_addresses = state
            .presence
            .read()
            .unwrap()
            .get(&session.target_device_id)
            .and_then(|p| p.addrs.as_ref())
            .map(|a| a.list.clone())
            .unwrap_or_default();
        if state.relay_info.is_none() && direct_addresses.is_empty() {
            session.state = SessionState::Unreachable;
        } else {
            let mut connect = json!({
                "endpoint_id": endpoint_id,
                "direct_addresses": direct_addresses,
            });
            if let Some(relay) = &state.relay_info {
                connect["relay"] = relay.clone();
            }
            session.connect = Some(connect);
            session.state = SessionState::Ready;
        }
    }

    let update = session_update(session);
    let source = session.source_device_id.clone();
    let target = session.target_device_id.clone();
    drop(sessions);
    // 状态变更推送双方（协议约定；轮询为兜底）
    state.push_ws(&source, &update);
    state.push_ws(&target, &update);
    Ok(success())
}

/// `POST /api/v1/sessions/{session_id}/cancel`：任一方取消会话
pub async fn cancel(
    State(state): State<Arc<AppState>>,
    auth: DeviceAuth,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    // 仅会话参与者可读取或取消，不能仅凭同租户身份操作。
    {
        let sessions = state.sessions.read().unwrap();
        let participant = sessions.get(&session_id).is_some_and(|session| {
            session.source_device_id == auth.device_id || session.target_device_id == auth.device_id
        });
        if !participant {
            return Err(ApiError::not_found("会话不存在"));
        }
    }
    let mut sessions = state.sessions.write().unwrap();
    let Some(session) = sessions.get_mut(&session_id) else {
        return Err(ApiError::not_found("会话不存在"));
    };
    session.apply_expiry(now_ms());
    match session.state {
        SessionState::Pending | SessionState::Ready => {
            session.state = SessionState::Cancelled;
            let source = session.source_device_id.clone();
            let target = session.target_device_id.clone();
            drop(sessions);
            state.close_relay(&session_id);
            let cancelled = json!({ "type": "session.cancelled", "session_id": session_id });
            state.push_ws(&source, &cancelled);
            state.push_ws(&target, &cancelled);
            Ok(success())
        }
        SessionState::Cancelled => Ok(success()),
        _ => Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("会话已结束（{}）", session.state.as_str()),
        )),
    }
}

#[derive(Deserialize)]
pub struct RendezvousRequest {
    target_device_id: String,
    source_endpoint_id: String,
    source_name: Option<String>,
}

/// AccessKey 原生客户端的匿名编排入口；只转发连接候选，不接收素材凭证。
pub async fn rendezvous(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    JsonBody(req): JsonBody<RendezvousRequest>,
) -> Result<Json<Value>, ApiError> {
    if let Some(secs) = state.ratelimit.check_ip(peer.ip()) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!("请求过于频繁，请 {secs} 秒后再试"),
        ));
    }
    if state
        .sessions
        .read()
        .unwrap()
        .values()
        .filter(|session| session.access_rendezvous)
        .count()
        >= 32
    {
        state.ratelimit.fail_ip(peer.ip());
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rendezvous 会话数已达上限",
        ));
    }
    if req.source_endpoint_id.parse::<iroh::EndpointId>().is_err() {
        state.ratelimit.fail_ip(peer.ip());
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "source_endpoint_id 必须是有效的 iroh 公钥",
        ));
    }
    let source_device_id = format!("d_guest_{:016x}", rand::random::<u64>());
    let source = crate::store::DeviceRecord {
        device_id: source_device_id.clone(),
        tenant_id: state
            .device(&req.target_device_id)
            .map(|device| device.tenant_id)
            .unwrap_or_default(),
        name: req
            .source_name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "AccessKey 客户端".to_string()),
        platform: "native".to_string(),
        endpoint_id: req.source_endpoint_id,
        library_name: String::new(),
        allow_remote: false,
        allow_import: false,
        registered_at_ms: now_ms(),
    };
    state
        .store
        .lock()
        .unwrap()
        .insert(source)
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let (session_id, initial) = match create_session_internal(
        &state,
        &source_device_id,
        &req.target_device_id,
        Role::Viewer,
        true,
    ) {
        Ok(result) => result,
        Err(error) => {
            let _ = state.store.lock().unwrap().remove(&source_device_id);
            state.ratelimit.fail_ip(peer.ip());
            return Err(error);
        }
    };
    if initial == SessionState::Rejected {
        let _ = state.store.lock().unwrap().remove(&source_device_id);
    }
    let token = state
        .sessions
        .read()
        .unwrap()
        .get(&session_id)
        .and_then(|session| session.rendezvous_token.clone())
        .ok_or_else(|| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "rendezvous 票据缺失"))?;
    Ok(ok(json!({
        "session_id": session_id,
        "state": initial.as_str(),
        "rendezvous_token": token,
    })))
}

fn rendezvous_authorized(state: &AppState, session_id: &str, token: &str) -> bool {
    state
        .sessions
        .read()
        .unwrap()
        .get(session_id)
        .is_some_and(|session| {
            session.access_rendezvous
                && session.rendezvous_token.as_deref().is_some_and(|expected| {
                    crate::auth::constant_time_eq(expected.as_bytes(), token.as_bytes())
                })
        })
}

pub async fn rendezvous_get(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let token = bearer_token(&headers)
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "rendezvous 票据无效"))?;
    if !rendezvous_authorized(&state, &session_id, &token) {
        return Err(ApiError::not_found("会话不存在"));
    }
    let mut sessions = state.sessions.write().unwrap();
    let Some(session) = sessions.get_mut(&session_id) else {
        return Err(ApiError::not_found("会话不存在"));
    };
    session.apply_expiry(now_ms());
    Ok(ok(session_view(session)))
}

pub async fn rendezvous_cancel(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let token = bearer_token(&headers)
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "rendezvous 票据无效"))?;
    if !rendezvous_authorized(&state, &session_id, &token) {
        return Err(ApiError::not_found("会话不存在"));
    }
    let (source, target) = {
        let mut sessions = state.sessions.write().unwrap();
        let Some(session) = sessions.get_mut(&session_id) else {
            return Err(ApiError::not_found("会话不存在"));
        };
        session.state = SessionState::Cancelled;
        (
            session.source_device_id.clone(),
            session.target_device_id.clone(),
        )
    };
    let _ = state.store.lock().unwrap().remove(&source);
    state.close_relay(&session_id);
    let cancelled = json!({ "type": "session.cancelled", "session_id": session_id });
    state.push_ws(&target, &cancelled);
    Ok(success())
}

fn bearer_token(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(String::from)
}

fn session_view(session: &Session) -> Value {
    let mut view = json!({
        "session_id": session.session_id,
        "state": session.state.as_str(),
    });
    if session.state == SessionState::Ready {
        if let Some(connect) = &session.connect {
            view["connect"] = connect.clone();
        }
    }
    view
}

fn session_update(session: &Session) -> Value {
    let mut update = json!({
        "type": "session.update",
        "session_id": session.session_id,
        "state": session.state.as_str(),
    });
    if session.state == SessionState::Ready {
        if let Some(connect) = &session.connect {
            update["connect"] = connect.clone();
        }
    }
    update
}

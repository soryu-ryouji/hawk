//! 会话编排：发起、状态查询、接受/拒绝/取消（协议「会话编排」一节）。
//! app 间会话要求双方同池（同一租户）；网页版/插件会话由查看面经 create_session_internal
//! 发起（服务端「网页版」全局设备为源），携带查看者凭证供目标 daemon 本地执法。

use super::{ok, success, ApiError, JsonBody};
use crate::auth::ServiceKey;
use crate::state::{now_ms, Access, AppState, Session, SessionState};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Deserialize)]
pub struct CreateRequest {
    source_device_id: String,
    target_device_id: String,
    mode: String,
}

#[derive(Deserialize)]
pub struct RespondRequest {
    device_id: String,
}

fn new_session_id() -> String {
    format!("s_{:016x}", rand::random::<u64>())
}

/// `POST /api/v1/sessions`：发起 app 间会话（service-key 鉴权，双方须同池）
pub async fn create(
    State(state): State<Arc<AppState>>,
    key: ServiceKey,
    JsonBody(req): JsonBody<CreateRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.mode != "direct" && req.mode != "relay" {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "不支持的会话模式（direct / relay）",
        ));
    }
    for device_id in [&req.source_device_id, &req.target_device_id] {
        match state.device(device_id) {
            Some(device) if device.tenant_id == key.tenant_id => {}
            _ => return Err(ApiError::not_found("设备不存在")),
        }
    }
    let (session_id, initial) = create_session_internal(
        &state,
        &req.source_device_id,
        &req.target_device_id,
        &req.mode,
        Access::Read,
        None,
    )?;
    Ok(ok(
        json!({ "session_id": session_id, "state": initial.as_str() }),
    ))
}

/// 会话创建核心（REST handler 与网页版宿主共用）：校验双方与在线、插入会话、推送 session.request。
/// credential：查看者会话携带的 hawk-app key 验证子（sha256），透传给目标 daemon 本地执法；
/// app 间会话为 None
pub fn create_session_internal(
    state: &Arc<AppState>,
    source_device_id: &str,
    target_device_id: &str,
    mode: &str,
    access: Access,
    credential: Option<String>,
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
    if access == Access::Write && !target.allow_write {
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
        mode: mode.to_string(),
        access,
        credential,
        state: initial,
        created_at_ms: now_ms(),
        connect: None,
    };
    let session_id = session.session_id.clone();
    let credential = session.credential.clone();
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
                    // 网页版全局设备不在租户列表中，不走列表本来也查不到
                    "fingerprint": source.as_ref().map(|s| s.fingerprint.as_str()).unwrap_or_default(),
                },
                "mode": mode,
                "access": access.as_str(),
                // 查看者会话的 key 验证子；app 间会话无此字段
                "credential": credential,
            }),
        );
    }
    Ok((session_id, initial))
}

/// `GET /api/v1/sessions/{session_id}`：会话状态即全部结果语义（发起方轮询兜底，1s 间隔上限 30s）
pub async fn get(
    State(state): State<Arc<AppState>>,
    _: ServiceKey,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let mut sessions = state.sessions.write().unwrap();
    let Some(session) = sessions.get_mut(&session_id) else {
        return Err(ApiError::not_found("会话不存在"));
    };
    session.apply_expiry(now_ms());
    Ok(ok(session_view(session)))
}

/// `POST /api/v1/sessions/{session_id}/accept`：目标设备接受会话，服务端组装候选列表
pub async fn accept(
    State(state): State<Arc<AppState>>,
    key: ServiceKey,
    Path(session_id): Path<String>,
    JsonBody(req): JsonBody<RespondRequest>,
) -> Result<Json<Value>, ApiError> {
    respond(state, key, session_id, req.device_id, true)
}

/// `POST /api/v1/sessions/{session_id}/reject`：目标设备拒绝会话
pub async fn reject(
    State(state): State<Arc<AppState>>,
    key: ServiceKey,
    Path(session_id): Path<String>,
    JsonBody(req): JsonBody<RespondRequest>,
) -> Result<Json<Value>, ApiError> {
    respond(state, key, session_id, req.device_id, false)
}

fn respond(
    state: Arc<AppState>,
    key: ServiceKey,
    session_id: String,
    device_id: String,
    accepted: bool,
) -> Result<Json<Value>, ApiError> {
    // 目标设备须属调用者租户（跨池响应同回 404）
    match state.device(&device_id) {
        Some(device) if device.tenant_id == key.tenant_id => {}
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
    } else if session.mode == "relay" {
        // 中继：无需可达性候选，connect 只携带目标指纹（QUIC 端到端钉扎语义不变）
        let fingerprint = state
            .device(&session.target_device_id)
            .map(|d| d.fingerprint)
            .unwrap_or_default();
        session.connect = Some(json!({
            "mode": "relay",
            "fingerprint": fingerprint,
        }));
        session.state = SessionState::Ready;
    } else {
        // 取目标最近心跳的可达性组装候选；无任何候选置 unreachable（协议约定）
        let candidates = state
            .presence
            .read()
            .unwrap()
            .get(&session.target_device_id)
            .and_then(|p| p.reachability.as_ref())
            .map(|r| r.candidates())
            .unwrap_or_default();
        if candidates.is_empty() {
            session.state = SessionState::Unreachable;
        } else {
            let fingerprint = state
                .device(&session.target_device_id)
                .map(|d| d.fingerprint)
                .unwrap_or_default();
            session.connect = Some(json!({
                "mode": "direct",
                "candidates": candidates,
                "fingerprint": fingerprint,
            }));
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
    key: ServiceKey,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    // 会话双方之一须属调用者租户
    {
        let sessions = state.sessions.read().unwrap();
        let in_pool = sessions.get(&session_id).is_some_and(|s| {
            [s.source_device_id.as_str(), s.target_device_id.as_str()]
                .iter()
                .any(|id| {
                    state
                        .device(id)
                        .is_some_and(|d| d.tenant_id == key.tenant_id)
                })
        });
        if !in_pool {
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

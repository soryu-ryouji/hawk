//! 会话编排：发起、状态查询、接受/拒绝/取消（协议「会话编排」一节，本期仅直连模式）。

use super::{ok, success, ApiError, JsonBody};
use crate::auth::AccessKey;
use crate::state::{now_ms, AppState, Session, SessionState};
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

/// `POST /api/v1/sessions`：发起会话（本期仅 mode=direct）
pub async fn create(
    State(state): State<Arc<AppState>>,
    _: AccessKey,
    JsonBody(req): JsonBody<CreateRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.mode != "direct" {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "不支持的会话模式，本期仅支持 direct",
        ));
    }
    if req.source_device_id == req.target_device_id {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "不能与自身建立会话"));
    }
    if state.device(&req.source_device_id).is_none() {
        return Err(ApiError::not_found("来源设备不存在"));
    }
    let Some(target) = state.device(&req.target_device_id) else {
        return Err(ApiError::not_found("目标设备不存在"));
    };
    if !state.online(&req.target_device_id) {
        return Err(ApiError::not_found("目标设备不在线"));
    }

    // allow_remote=false 直接置 rejected（协议约定，不打扰目标设备）
    let initial = if target.allow_remote {
        SessionState::Pending
    } else {
        SessionState::Rejected
    };
    let session = Session {
        session_id: new_session_id(),
        source_device_id: req.source_device_id.clone(),
        target_device_id: req.target_device_id.clone(),
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
        let source = state.device(&req.source_device_id);
        state.push_ws(
            &req.target_device_id,
            &json!({
                "type": "session.request",
                "session_id": session_id,
                "from": {
                    "device_id": req.source_device_id,
                    "name": source.as_ref().map(|s| s.name.as_str()).unwrap_or_default(),
                    "library_name": source.as_ref().map(|s| s.library_name.as_str()).unwrap_or_default(),
                },
                "mode": "direct",
            }),
        );
    }
    Ok(ok(
        json!({ "session_id": session_id, "state": initial.as_str() }),
    ))
}

/// `GET /api/v1/sessions/{session_id}`：会话状态即全部结果语义（发起方轮询兜底，1s 间隔上限 30s）
pub async fn get(
    State(state): State<Arc<AppState>>,
    _: AccessKey,
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
    _: AccessKey,
    Path(session_id): Path<String>,
    JsonBody(req): JsonBody<RespondRequest>,
) -> Result<Json<Value>, ApiError> {
    respond(state, session_id, req.device_id, true)
}

/// `POST /api/v1/sessions/{session_id}/reject`：目标设备拒绝会话
pub async fn reject(
    State(state): State<Arc<AppState>>,
    _: AccessKey,
    Path(session_id): Path<String>,
    JsonBody(req): JsonBody<RespondRequest>,
) -> Result<Json<Value>, ApiError> {
    respond(state, session_id, req.device_id, false)
}

fn respond(
    state: Arc<AppState>,
    session_id: String,
    device_id: String,
    accepted: bool,
) -> Result<Json<Value>, ApiError> {
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
    _: AccessKey,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
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

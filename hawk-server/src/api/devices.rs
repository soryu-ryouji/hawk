//! 设备管理：注册、心跳、列表、更新、注销（协议「设备管理」一节）。

use super::{ok, success, ApiError, JsonBody};
use crate::auth::AccessKey;
use crate::state::{
    now_ms, AppState, Presence, Reachability, SessionState, HEARTBEAT_MIN_INTERVAL_MS,
    HEARTBEAT_OFFLINE_MS,
};
use crate::store::DeviceRecord;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct RegisterRequest {
    name: String,
    platform: String,
    fingerprint: String,
    library_name: String,
    allow_remote: bool,
}

#[derive(Deserialize)]
pub struct HeartbeatRequest {
    device_id: String,
    reachability: HeartbeatReachability,
}

/// 心跳上报的可达性；public_ip 以服务端看到的连接源地址为准（自报值仅作占位不采用）
#[derive(Deserialize)]
pub struct HeartbeatReachability {
    #[serde(default)]
    #[allow(dead_code)]
    public_ip: Option<String>,
    mapped_port: Option<u16>,
    #[serde(default = "default_mapping_type")]
    mapping_type: String,
    public_ipv6: Option<String>,
    ipv6_port: Option<u16>,
    /// P3 打洞预留，阶段 1 服务端不使用
    #[serde(default)]
    #[allow(dead_code)]
    nat_type: Option<String>,
}

fn default_mapping_type() -> String {
    "none".to_string()
}

#[derive(Deserialize)]
pub struct UpdateRequest {
    name: Option<String>,
    allow_remote: Option<bool>,
    library_name: Option<String>,
    fingerprint: Option<String>,
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}_{:016x}", rand::random::<u64>())
}

/// `POST /api/v1/devices`：注册设备，返回 device_id
pub async fn register(
    State(state): State<Arc<AppState>>,
    _: AccessKey,
    JsonBody(req): JsonBody<RegisterRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.name.trim().is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "name 不能为空"));
    }
    if req.fingerprint.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "fingerprint 不能为空",
        ));
    }
    let record = DeviceRecord {
        device_id: new_id("d"),
        name: req.name,
        platform: req.platform,
        fingerprint: req.fingerprint,
        library_name: req.library_name,
        allow_remote: req.allow_remote,
        registered_at_ms: now_ms(),
    };
    let device_id = record.device_id.clone();
    state.store.lock().unwrap().insert(record).map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("设备注册表写入失败: {e}"),
        )
    })?;
    Ok(ok(json!({ "device_id": device_id })))
}

/// `POST /api/v1/devices/heartbeat`：在线心跳（间隔 30s，最小间隔 10s），携带可达性信息
pub async fn heartbeat(
    State(state): State<Arc<AppState>>,
    _: AccessKey,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    JsonBody(req): JsonBody<HeartbeatRequest>,
) -> Result<Json<Value>, ApiError> {
    if state.device(&req.device_id).is_none() {
        return Err(ApiError::not_found("设备不存在"));
    }
    let now = now_ms();
    let public_ip_seen = peer.ip().to_string();
    let mut presence = state.presence.write().unwrap();
    let p = presence.entry(req.device_id.clone()).or_default();
    if let Some(last) = p.last_heartbeat_ms {
        if now.saturating_sub(last) < HEARTBEAT_MIN_INTERVAL_MS {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "心跳过于频繁，最小间隔 10s",
            ));
        }
    }
    p.last_heartbeat_ms = Some(now);
    p.reachability = Some(Reachability {
        public_ip: public_ip_seen.clone(),
        mapped_port: req.reachability.mapped_port,
        mapping_type: req.reachability.mapping_type,
        public_ipv6: req.reachability.public_ipv6,
        ipv6_port: req.reachability.ipv6_port,
    });
    Ok(ok(
        json!({ "server_time_ms": now, "public_ip_seen": public_ip_seen }),
    ))
}

/// `GET /api/v1/devices`：账号下设备列表（单用户即全部已注册设备）
pub async fn list(
    State(state): State<Arc<AppState>>,
    _: AccessKey,
) -> Result<Json<Value>, ApiError> {
    let now = now_ms();
    let devices = state.store.lock().unwrap().list();
    let presence = state.presence.read().unwrap();
    let views: Vec<Value> = devices
        .iter()
        .map(|d| device_view(d, presence.get(&d.device_id), now))
        .collect();
    Ok(ok(json!({ "devices": views })))
}

/// `PATCH /api/v1/devices/{device_id}`：更新设备（fingerprint 更新即证书轮换）
pub async fn update(
    State(state): State<Arc<AppState>>,
    _: AccessKey,
    Path(device_id): Path<String>,
    JsonBody(req): JsonBody<UpdateRequest>,
) -> Result<Json<Value>, ApiError> {
    let updated = {
        let mut store = state.store.lock().unwrap();
        store
            .update(&device_id, |record| {
                if let Some(name) = req.name {
                    record.name = name;
                }
                if let Some(allow_remote) = req.allow_remote {
                    record.allow_remote = allow_remote;
                }
                if let Some(library_name) = req.library_name {
                    record.library_name = library_name;
                }
                if let Some(fingerprint) = req.fingerprint {
                    record.fingerprint = fingerprint;
                }
            })
            .map_err(|e| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("设备注册表写入失败: {e}"),
                )
            })?
    };
    let Some(record) = updated else {
        return Err(ApiError::not_found("设备不存在"));
    };
    let presence = state.presence.read().unwrap();
    Ok(ok(device_view(&record, presence.get(&device_id), now_ms())))
}

/// `DELETE /api/v1/devices/{device_id}`：注销设备；其进行中的会话一并取消
pub async fn remove(
    State(state): State<Arc<AppState>>,
    _: AccessKey,
    Path(device_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let removed = state
        .store
        .lock()
        .unwrap()
        .remove(&device_id)
        .map_err(|e| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("设备注册表写入失败: {e}"),
            )
        })?;
    if removed.is_none() {
        return Err(ApiError::not_found("设备不存在"));
    }
    state.presence.write().unwrap().remove(&device_id);
    let notifications = cancel_device_sessions(&state, &device_id);
    for (peer, session_id) in notifications {
        state.push_ws(
            &peer,
            &json!({ "type": "session.cancelled", "session_id": session_id }),
        );
    }
    Ok(success())
}

/// 注销设备时取消其进行中的会话，返回需要通知的对端（设备, 会话）列表
fn cancel_device_sessions(state: &Arc<AppState>, device_id: &str) -> Vec<(String, String)> {
    let mut notifications = Vec::new();
    let mut sessions = state.sessions.write().unwrap();
    for session in sessions.values_mut() {
        let involves =
            session.source_device_id == device_id || session.target_device_id == device_id;
        if involves && !session.state.is_terminal() {
            session.state = SessionState::Cancelled;
            let peer = if session.source_device_id == device_id {
                session.target_device_id.clone()
            } else {
                session.source_device_id.clone()
            };
            notifications.push((peer, session.session_id.clone()));
        }
    }
    notifications
}

fn device_view(record: &DeviceRecord, presence: Option<&Presence>, now: u64) -> Value {
    let last_seen = presence.and_then(|p| p.last_heartbeat_ms);
    let online = last_seen.is_some_and(|last| now.saturating_sub(last) <= HEARTBEAT_OFFLINE_MS);
    let reachability = presence.and_then(|p| p.reachability.as_ref()).map(|r| {
        json!({
            "public_ip": r.public_ip,
            "mapped_port": r.mapped_port,
            "mapping_type": r.mapping_type,
            "public_ipv6": r.public_ipv6,
            "ipv6_port": r.ipv6_port,
        })
    });
    json!({
        "device_id": record.device_id,
        "name": record.name,
        "platform": record.platform,
        "library_name": record.library_name,
        "online": online,
        "last_seen_at_ms": last_seen,
        "fingerprint": record.fingerprint,
        "allow_remote": record.allow_remote,
        "reachability": reachability,
    })
}

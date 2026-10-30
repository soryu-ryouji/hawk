//! 设备管理：注册、心跳、列表、更新、注销、key 映射同步（协议「设备管理」一节）。
//! 全部端点按 service-key 解析出的租户隔离：设备只能操作本池内的记录。

use super::{ok, success, ApiError, JsonBody};
use crate::auth::ServiceKey;
use crate::state::{
    now_ms, AppState, Presence, SessionState, HEARTBEAT_MIN_INTERVAL_MS, HEARTBEAT_OFFLINE_MS,
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
    endpoint_id: String,
    library_name: String,
    allow_remote: bool,
    /// 允许广域网写会话（目标设备 [remote].writable）
    allow_import: bool,
}

#[derive(Deserialize)]
pub struct HeartbeatRequest {
    device_id: String,
    /// 目标设备当前是否允许广域网写入；变化时更新设备表，供查看面筛选
    #[serde(default)]
    allow_import: Option<bool>,
    /// 当前打开的素材库名（设备身份跨库共享，切换库后靠心跳收敛设备表）
    #[serde(default)]
    library_name: Option<String>,
    /// iroh endpoint 自报的直连地址集（服务端不验证可达性，拨号方的连接尝试即最终校验）
    #[serde(default)]
    addrs: Vec<String>,
}

#[derive(Deserialize)]
pub struct UpdateRequest {
    name: Option<String>,
    allow_remote: Option<bool>,
    library_name: Option<String>,
    endpoint_id: Option<String>,
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}_{:016x}", rand::random::<u64>())
}

/// `POST /api/v1/devices`：注册设备（入调用者的租户池），返回 device_id
pub async fn register(
    State(state): State<Arc<AppState>>,
    key: ServiceKey,
    JsonBody(req): JsonBody<RegisterRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.name.trim().is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "name 不能为空"));
    }
    if req.endpoint_id.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "fingerprint 不能为空",
        ));
    }
    let record = DeviceRecord {
        device_id: new_id("d"),
        tenant_id: key.tenant_id,
        name: req.name,
        platform: req.platform,
        endpoint_id: req.endpoint_id,
        library_name: req.library_name,
        allow_remote: req.allow_remote,
        allow_import: req.allow_import,
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
    key: ServiceKey,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
    JsonBody(req): JsonBody<HeartbeatRequest>,
) -> Result<Json<Value>, ApiError> {
    let device = tenant_device(&state, &key.tenant_id, &req.device_id)?;
    let library_name = req
        .library_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty() && *name != device.library_name);
    if req.allow_import.is_some_and(|w| w != device.allow_import) || library_name.is_some() {
        state
            .store
            .lock()
            .unwrap()
            .update(&req.device_id, |record| {
                if let Some(allow_import) = req.allow_import {
                    record.allow_import = allow_import;
                }
                if let Some(library_name) = library_name {
                    record.library_name = library_name.to_string();
                }
            })
            .map_err(|e| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("设备注册表写入失败: {e}"),
                )
            })?;
    }
    let now = now_ms();
    let public_ip_seen = client_ip(&headers, peer);
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
    p.addrs = Some(crate::state::Addrs {
        public_ip: public_ip_seen.clone(),
        list: req.addrs,
    });
    Ok(ok(
        // relay：内嵌中继的公网入口与准入 token（None = 本部署不提供中继；
        // 变化时客户端重建数据面，见协议「中继」一节）
        json!({ "server_time_ms": now, "public_ip_seen": public_ip_seen, "relay": state.relay_info.clone().unwrap_or(Value::Null) }),
    ))
}

/// 本池设备校验：存在且属于调用者租户（不存在/跨池同回 404，不泄露他池设备存在性）
fn tenant_device(
    state: &Arc<AppState>,
    tenant_id: &str,
    device_id: &str,
) -> Result<DeviceRecord, ApiError> {
    match state.device(device_id) {
        Some(device) if device.tenant_id == tenant_id => Ok(device),
        _ => Err(ApiError::not_found("设备不存在")),
    }
}

#[derive(Deserialize)]
pub struct SyncKeysRequest {
    /// 该设备的 hawk-app key 全集（覆盖式同步；s = sha256(key)，server 不见明文）
    keys: Vec<crate::keystore::AppKeySync>,
}

/// `PUT /api/v1/devices/{device_id}/keys`：全量同步本设备的 hawk-app key 映射
/// （daemon 在 key 变更与每次接入时推送）。条目不合法整体拒绝
pub async fn sync_keys(
    State(state): State<Arc<AppState>>,
    key: ServiceKey,
    Path(device_id): Path<String>,
    JsonBody(req): JsonBody<SyncKeysRequest>,
) -> Result<Json<Value>, ApiError> {
    tenant_device(&state, &key.tenant_id, &device_id)?;
    for entry in &req.keys {
        let valid = entry.s.len() == 64
            && entry
                .s
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase());
        if !valid {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "keys[].s 须为 64 位小写十六进制（sha256）",
            ));
        }
    }
    state
        .keys
        .sync_device_keys(&key.tenant_id, &device_id, &req.keys)
        .map_err(|e| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("key 注册表写入失败: {e}"),
            )
        })?;
    Ok(ok(json!({ "synced": req.keys.len() })))
}

/// 权威客户端地址：反代部署（nginx/Caddy）时 TCP 对端是反代的环回地址，直连候选依赖
/// 真实公网 IP——优先 X-Real-IP，其次 X-Forwarded-For 首跳，回退 TCP 对端。
/// 信任前提：明文端口只绑环回（部署文档要求），全部流量必经反代，头不被伪造
fn client_ip(headers: &axum::http::HeaderMap, peer: SocketAddr) -> String {
    if let Some(real_ip) = headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        return real_ip.to_string();
    }
    if let Some(first) = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        return first.to_string();
    }
    peer.ip().to_string()
}

/// `GET /api/v1/devices`：本租户池设备列表（不含服务端的「网页版」全局设备）
pub async fn list(
    State(state): State<Arc<AppState>>,
    key: ServiceKey,
) -> Result<Json<Value>, ApiError> {
    let now = now_ms();
    let devices = state.store.lock().unwrap().list_tenant(&key.tenant_id);
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
    key: ServiceKey,
    Path(device_id): Path<String>,
    JsonBody(req): JsonBody<UpdateRequest>,
) -> Result<Json<Value>, ApiError> {
    tenant_device(&state, &key.tenant_id, &device_id)?;
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
                if let Some(endpoint_id) = req.endpoint_id {
                    record.endpoint_id = endpoint_id;
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

/// `DELETE /api/v1/devices/{device_id}`：注销设备；其进行中的会话与 key 映射一并清理
pub async fn remove(
    State(state): State<Arc<AppState>>,
    key: ServiceKey,
    Path(device_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    tenant_device(&state, &key.tenant_id, &device_id)?;
    state
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
    state.keys.remove_device_keys(&device_id).map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("key 注册表写入失败: {e}"),
        )
    })?;
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

/// 设备视图构建（控制面列表与网页版查看面共用）
pub(crate) fn device_view_pub(
    record: &DeviceRecord,
    presence: Option<&Presence>,
    now: u64,
) -> Value {
    device_view(record, presence, now)
}

fn device_view(record: &DeviceRecord, presence: Option<&Presence>, now: u64) -> Value {
    let last_seen = presence.and_then(|p| p.last_heartbeat_ms);
    let online = last_seen.is_some_and(|last| now.saturating_sub(last) <= HEARTBEAT_OFFLINE_MS);
    let addrs: Vec<String> = presence
        .and_then(|p| p.addrs.as_ref())
        .map(|a| a.list.clone())
        .unwrap_or_default();
    json!({
        "device_id": record.device_id,
        "name": record.name,
        "platform": record.platform,
        "library_name": record.library_name,
        "online": online,
        "last_seen_at_ms": last_seen,
        "endpoint_id": record.endpoint_id,
        "allow_remote": record.allow_remote,
        "allow_import": record.allow_import,
        "addrs": addrs,
    })
}

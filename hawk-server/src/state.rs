//! 运行时状态：持久化设备表之外的全部内存状态（在线心跳、WSS 连接、会话）。

use crate::store::{DeviceRecord, Store};
use axum::extract::ws::Message;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

/// 心跳离线判定：90s 未收到心跳判离线（协议约定）
pub const HEARTBEAT_OFFLINE_MS: u64 = 90_000;
/// 心跳最小间隔 10s（协议限速）
pub const HEARTBEAT_MIN_INTERVAL_MS: u64 = 10_000;
/// 会话建立超时 30s（协议约定）
pub const SESSION_TIMEOUT_MS: u64 = 30_000;
/// QUIC 默认端口：目标心跳未上报 ipv6_port 时的回退值
pub const DEFAULT_QUIC_PORT: u16 = 27373;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// 目标设备的直连可达性（来自最近一次心跳；public_ip 以服务端看到的连接源地址为准）
#[derive(Clone)]
pub struct Reachability {
    pub public_ip: String,
    pub mapped_port: Option<u16>,
    pub mapping_type: String,
    pub public_ipv6: Option<String>,
    pub ipv6_port: Option<u16>,
}

impl Reachability {
    /// 组装直连候选列表：IPv4 映射优先、IPv6 次之（协议约定）
    pub fn candidates(&self) -> Vec<Value> {
        let mut candidates = Vec::new();
        if let Some(port) = self.mapped_port {
            candidates.push(json!({ "transport": "ipv4", "host": self.public_ip, "port": port }));
        }
        if let Some(ipv6) = &self.public_ipv6 {
            candidates.push(json!({
                "transport": "ipv6",
                "host": ipv6,
                "port": self.ipv6_port.unwrap_or(DEFAULT_QUIC_PORT),
            }));
        }
        candidates
    }
}

/// 设备在线状态：心跳 + WSS 连接（新连接顶替旧连接，conn_id 区分代际）
#[derive(Default)]
pub struct Presence {
    pub last_heartbeat_ms: Option<u64>,
    pub reachability: Option<Reachability>,
    pub ws: Option<(u64, mpsc::UnboundedSender<Message>)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Pending,
    Ready,
    Rejected,
    Unreachable,
    Expired,
    Cancelled,
}

impl SessionState {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionState::Pending => "pending",
            SessionState::Ready => "ready",
            SessionState::Rejected => "rejected",
            SessionState::Unreachable => "unreachable",
            SessionState::Expired => "expired",
            SessionState::Cancelled => "cancelled",
        }
    }

    /// 终态：不再接受 accept/reject/cancel 之外的状态迁移
    pub fn is_terminal(self) -> bool {
        !matches!(self, SessionState::Pending | SessionState::Ready)
    }
}

pub struct Session {
    pub session_id: String,
    pub source_device_id: String,
    pub target_device_id: String,
    pub state: SessionState,
    pub created_at_ms: u64,
    /// ready 时组装的连接信息（候选列表 + 目标证书指纹）
    pub connect: Option<Value>,
}

impl Session {
    /// 惰性过期：pending 超 30s 转为 expired；返回是否发生了迁移
    pub fn apply_expiry(&mut self, now: u64) -> bool {
        if self.state == SessionState::Pending
            && now.saturating_sub(self.created_at_ms) > SESSION_TIMEOUT_MS
        {
            self.state = SessionState::Expired;
            return true;
        }
        false
    }
}

pub struct AppState {
    pub access_key: String,
    pub store: Mutex<Store>,
    pub presence: RwLock<HashMap<String, Presence>>,
    pub sessions: RwLock<HashMap<String, Session>>,
    conn_seq: AtomicU64,
}

impl AppState {
    pub fn new(access_key: String, store: Store) -> AppState {
        AppState {
            access_key,
            store: Mutex::new(store),
            presence: RwLock::new(HashMap::new()),
            sessions: RwLock::new(HashMap::new()),
            conn_seq: AtomicU64::new(1),
        }
    }

    pub fn device(&self, device_id: &str) -> Option<DeviceRecord> {
        self.store.lock().unwrap().get(device_id).cloned()
    }

    /// 在线 = 90s 内有心跳（协议以心跳为在线依据，WSS 只是推送通道）
    pub fn online(&self, device_id: &str) -> bool {
        let now = now_ms();
        self.presence
            .read()
            .unwrap()
            .get(device_id)
            .and_then(|p| p.last_heartbeat_ms)
            .is_some_and(|last| now.saturating_sub(last) <= HEARTBEAT_OFFLINE_MS)
    }

    pub fn next_conn_id(&self) -> u64 {
        self.conn_seq.fetch_add(1, Ordering::Relaxed)
    }

    /// 经 WSS 控制通道向设备推送消息；设备未连接时静默丢弃（推送是增强，轮询为兜底）
    pub fn push_ws(&self, device_id: &str, payload: &Value) {
        let text = payload.to_string();
        let presence = self.presence.read().unwrap();
        if let Some((_, tx)) = presence.get(device_id).and_then(|p| p.ws.as_ref()) {
            let _ = tx.send(Message::Text(text.into()));
        }
    }
}

//! 运行时状态：持久化设备表之外的全部内存状态（在线心跳、WSS 连接、会话）。

use crate::store::{DeviceRecord, Store};
use axum::extract::ws::Message;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

/// 心跳离线判定：90s 未收到心跳判离线（协议约定）
pub const HEARTBEAT_OFFLINE_MS: u64 = 90_000;
/// 心跳最小间隔 10s（协议限速）
pub const HEARTBEAT_MIN_INTERVAL_MS: u64 = 10_000;
/// 会话建立超时 30s（协议约定）
pub const SESSION_TIMEOUT_MS: u64 = 30_000;

/// 查看面令牌/会话的角色：viewer 只读浏览；importer 仅限导入。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    #[default]
    Viewer,
    Importer,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Importer => "importer",
        }
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// 设备的寻址信息（来自最近一次心跳；iroh endpoint 自报的直连地址集。
/// 服务端不验证可达性——拨号方的连接尝试即最终校验（协议约定）
#[derive(Clone, Default)]
pub struct Addrs {
    pub public_ip: String,
    pub list: Vec<String>,
}

/// 设备在线状态：心跳 + WSS 连接（新连接顶替旧连接，conn_id 区分代际）
#[derive(Default)]
pub struct Presence {
    pub last_heartbeat_ms: Option<u64>,
    pub addrs: Option<Addrs>,
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
    /// viewer（只读查看）/ importer（受限导入）
    pub role: Role,
    /// AccessKey 原生客户端的匿名编排会话；不包含任何 AccessKey 或验证子。
    pub access_rendezvous: bool,
    /// rendezvous 状态查询/取消的短期票据，不下发给目标 daemon。
    pub rendezvous_token: Option<String>,
    pub state: SessionState,
    pub created_at_ms: u64,
    /// ready 时组装的连接信息（endpoint_id + 直连地址 + 可选中继）
    pub connect: Option<Value>,
}

/// 中继配对：会话双方各自的中继 WS 发送端（role=a 目标设备 / role=b 发起方）
#[derive(Default)]
pub struct RelayPair {
    pub a: Option<mpsc::UnboundedSender<Message>>,
    pub b: Option<mpsc::UnboundedSender<Message>>,
}

impl RelayPair {
    pub fn register(&mut self, role: &str, sender: mpsc::UnboundedSender<Message>) {
        match role {
            "a" => self.a = Some(sender),
            _ => self.b = Some(sender),
        }
    }

    pub fn unregister(&mut self, role: &str) {
        match role {
            "a" => self.a = None,
            _ => self.b = None,
        }
    }

    /// 对端发送端（转发目标）
    pub fn peer(&self, role: &str) -> Option<&mpsc::UnboundedSender<Message>> {
        match role {
            "a" => self.b.as_ref(),
            _ => self.a.as_ref(),
        }
    }

    pub fn paired(&self) -> bool {
        self.a.is_some() && self.b.is_some()
    }
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
    /// 租户密钥注册表（管理面监听口共享同一实例）
    pub keys: Arc<crate::keystore::KeyStore>,
    /// TenantAdmin/Device Session 与设备签名 nonce
    pub auth: crate::auth::AuthState,
    /// 凭证验证面风控（按 IP + 按租户）
    pub ratelimit: crate::ratelimit::RateLimiter,
    pub store: Mutex<Store>,
    pub presence: RwLock<HashMap<String, Presence>>,
    pub sessions: RwLock<HashMap<String, Session>>,
    /// 中继会话的 WS 配对（session_id → 双方发送端）
    pub relay_pairs: RwLock<HashMap<String, RelayPair>>,
    /// 配对表变更通知（进程内中继 socket 的 poller 等待源）
    pub relay_changed: Arc<tokio::sync::Notify>,
    /// 内嵌中继的公网入口（url + 准入 token；None = 本部署不提供中继，纯直连）
    pub relay_info: Option<Value>,
    conn_seq: AtomicU64,
}

impl AppState {
    pub fn new(keys: Arc<crate::keystore::KeyStore>, store: Store) -> AppState {
        AppState {
            keys,
            auth: crate::auth::AuthState::default(),
            ratelimit: crate::ratelimit::RateLimiter::new(),
            store: Mutex::new(store),
            presence: RwLock::new(HashMap::new()),
            sessions: RwLock::new(HashMap::new()),
            relay_pairs: RwLock::new(HashMap::new()),
            relay_changed: Arc::new(tokio::sync::Notify::new()),
            relay_info: None,
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

    /// 关闭中继配对（会话取消/一方断开）：双方中继 WS 收到 Close 并移除配对
    pub fn close_relay(&self, session_id: &str) {
        if let Some(pair) = self.relay_pairs.write().unwrap().remove(session_id) {
            for sender in [pair.a, pair.b].into_iter().flatten() {
                let _ = sender.send(Message::Close(None));
            }
        }
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

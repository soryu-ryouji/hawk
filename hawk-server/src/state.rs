//! 运行时状态：持久化设备表之外的全部内存状态（在线心跳、WSS 连接、会话）。

use crate::store::{DeviceRecord, Store};
use axum::extract::ws::Message;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
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
/// QUIC 默认端口：目标心跳未上报 ipv6_port 时的回退值
pub const DEFAULT_QUIC_PORT: u16 = 27373;

/// 查看面令牌/会话的访问级别：read 只读浏览；write 仅限插件导入（目标 daemon 仍做端点白名单）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    Read,
    Write,
}

impl Access {
    pub fn as_str(self) -> &'static str {
        match self {
            Access::Read => "read",
            Access::Write => "write",
        }
    }
}

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
    /// direct（直连撮合）/ relay（服务端中继转发）
    pub mode: String,
    /// read（只读查看）/ write（广域网受限写）
    pub access: Access,
    /// 查看者凭证（hawk-app key 的 sha256）：仅网页版/插件发起的会话携带，
    /// 随 session.request 下发给目标 daemon 本地执法（server 声称的权限不可信）
    pub credential: Option<String>,
    pub state: SessionState,
    pub created_at_ms: u64,
    /// ready 时组装的连接信息（direct: 候选列表 + 指纹；relay: 仅指纹）
    pub connect: Option<Value>,
    /// 打洞编排状态（仅 direct 且目标无直连候选时创建；探测口记录 observed endpoint）
    pub probe: Option<ProbeState>,
}

/// 打洞编排（P3）：双方探测 token 与探测口观测到的 observed endpoint。
/// token 按角色签发并经 WSS session.probe 下发；探测包归属判定走 AppState.probe_index
#[derive(Clone, Default)]
pub struct ProbeState {
    /// 目标设备（被连侧）探测 token
    pub token_a: [u8; 16],
    /// 发起方探测 token
    pub token_b: [u8; 16],
    pub addr_a: Option<SocketAddr>,
    pub addr_b: Option<SocketAddr>,
}

/// 探测票据：token → 会话与角色（探测口收包时的归属判定）
#[derive(Clone)]
pub struct ProbeTicket {
    pub session_id: String,
    /// a = 目标设备（被连侧）/ b = 发起方
    pub role: String,
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
    /// 凭证存储（租户 service-key / hawk-app key 映射 / 门令牌；管理面监听口共享同一实例）
    pub keys: Arc<crate::keystore::KeyStore>,
    /// 凭证验证面风控（按 IP + 按租户）
    pub ratelimit: crate::ratelimit::RateLimiter,
    pub store: Mutex<Store>,
    pub presence: RwLock<HashMap<String, Presence>>,
    pub sessions: RwLock<HashMap<String, Session>>,
    /// 中继会话的 WS 配对（session_id → 双方发送端）
    pub relay_pairs: RwLock<HashMap<String, RelayPair>>,
    /// 配对表变更通知（进程内中继 socket 的 poller 等待源）
    pub relay_changed: Arc<tokio::sync::Notify>,
    /// 打洞探测口端口（None = 打洞编排未启用：无直连候选的会话直接 unreachable）；
    /// 由启动流程在绑定探测 socket 成功后写入
    pub probe_port: Option<u16>,
    /// 探测 token 索引（探测口收包 → 会话归属）；编排结束（到齐/超时/终态）即清除
    pub probe_index: RwLock<HashMap<[u8; 16], ProbeTicket>>,
    conn_seq: AtomicU64,
}

impl AppState {
    pub fn new(keys: Arc<crate::keystore::KeyStore>, store: Store) -> AppState {
        AppState {
            keys,
            ratelimit: crate::ratelimit::RateLimiter::new(),
            store: Mutex::new(store),
            presence: RwLock::new(HashMap::new()),
            sessions: RwLock::new(HashMap::new()),
            relay_pairs: RwLock::new(HashMap::new()),
            relay_changed: Arc::new(tokio::sync::Notify::new()),
            probe_port: None,
            probe_index: RwLock::new(HashMap::new()),
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

    /// 探测口收包归属：token 命中进行中会话的打洞编排则记录 observed endpoint 并返回
    /// （会话 id, 角色）（探测口记日志用）；未命中返回 None（静默丢弃不回包——防反射放大）
    pub fn record_probe(&self, token: &[u8; 16], from: SocketAddr) -> Option<(String, String)> {
        let ticket = self.probe_index.read().unwrap().get(token).cloned()?;
        let mut sessions = self.sessions.write().unwrap();
        let session = sessions.get_mut(&ticket.session_id)?;
        let probe = session.probe.as_mut()?;
        match ticket.role.as_str() {
            "a" => probe.addr_a = Some(from),
            _ => probe.addr_b = Some(from),
        }
        Some((ticket.session_id, ticket.role))
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

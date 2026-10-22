//! 网页版宿主：hawk-server 作为 B 侧（协议中的查看方），任意浏览器经域名查看素材库；
//! 写作用域令牌可供插件通过查看面代理导入素材（目标设备仍需开启广域网写入）。
//! 隧道在服务器终结（素材明文过服务器，自托管单用户信任前提）；浏览器令牌（bt）与
//! access_key / write_key 分离：登录换发 read/write 作用域令牌，服务端只存哈希与作用域。

pub mod api;
pub mod page;
pub mod proxy;
mod tokens;

/// 隧道 ALPN（协议常量；两侧零代码共享，各自硬编码同一值）
pub const HAWK_ALPN: &[u8] = b"hawk/1";

use crate::api::sessions::create_session_internal;
use crate::identity::Identity;
use crate::state::{Access, AppState, SessionState};
use crate::store::DeviceRecord;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 会话 ready 轮询（A 侧经 WSS 异步接受）
const READY_POLL: Duration = Duration::from_secs(1);
const READY_TIMEOUT: Duration = Duration::from_secs(30);
/// 中继隧道 QUIC 握手超时：中继比直连多一串串行链路（A 收 WSS 推送 → accept RTT →
/// A 经反代连 relay WS → 配对 → WS 上的 QUIC 往返），5s 在真实跨网环境下太紧
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

pub struct ViewerSession {
    pub session_token: String,
    pub hawk_session_id: String,
    pub access: Access,
    pub target_device_id: String,
    pub target_name: String,
    pub target_library_name: String,
    pub conn: iroh::endpoint::Connection,
}

struct ViewerInner {
    /// 有效浏览器令牌哈希（sha256 hex）→ 授权（级别 + 绑定设备 + 查看者凭证）
    token_hashes: HashMap<String, tokens::Grant>,
    /// session_token → 会话
    sessions: HashMap<String, Arc<ViewerSession>>,
    /// bt 哈希 → session_token（当前绑定）
    bindings: HashMap<String, String>,
}

pub struct ViewerManager {
    state: Arc<AppState>,
    identity: Identity,
    /// 「网页版」宿主的长驻 iroh endpoint（B 侧拨号方）
    endpoint: iroh::Endpoint,
    inner: Mutex<ViewerInner>,
    tokens_file: PathBuf,
}

impl ViewerManager {
    pub fn new(
        state: Arc<AppState>,
        identity: Identity,
        endpoint: iroh::Endpoint,
        data_dir: &std::path::Path,
    ) -> Arc<ViewerManager> {
        let token_hashes = tokens::load(&data_dir.join("viewer_tokens.json"));
        Arc::new(ViewerManager {
            state,
            identity,
            endpoint,
            inner: Mutex::new(ViewerInner {
                token_hashes,
                sessions: HashMap::new(),
                bindings: HashMap::new(),
            }),
            tokens_file: data_dir.join("viewer_tokens.json"),
        })
    }

    pub fn app_state(&self) -> &Arc<AppState> {
        &self.state
    }

    // ---------- 浏览器令牌（两级串联验证：service-key → 门令牌 → hawk-app key → bt） ----------

    /// 第一级：service-key 换门令牌（短时效 + 有限尝试预算）。验证失败返回 None
    pub fn gate(&self, service_key: &str) -> Option<String> {
        let tenant = self.state.keys.resolve_tenant(service_key)?;
        Some(self.state.keys.issue_gate(&tenant.id))
    }

    /// 门令牌所属租户（登录前的池冷却检查用；不消耗尝试预算）
    pub fn gate_tenant(&self, gate_token: &str) -> Option<String> {
        self.state.keys.gate_tenant(gate_token)
    }

    /// 第二级：门令牌 + hawk-app key 换发 bt（消耗门令牌一次尝试）。
    /// bt 绑定目标设备与权限；服务端只存哈希
    pub fn login(&self, gate_token: &str, key: &str) -> Option<(String, tokens::Grant)> {
        let tenant_id = self.state.keys.consume_gate_attempt(gate_token)?;
        let entry = self.state.keys.resolve_app_key(key)?;
        // key 必须落在门令牌绑定的池里（跨池 key 同按无效处理，不泄露存在性）
        if entry.tenant_id != tenant_id {
            return None;
        }
        // 设备可能已被注销：key 映射随之失效（设备注销时服务端已清理映射，此处兑底）
        self.state.device(&entry.device_id)?;
        let token = format!(
            "hb_{:016x}{:016x}",
            rand::random::<u64>(),
            rand::random::<u64>()
        );
        let grant = tokens::Grant {
            access: if entry.write {
                Access::Write
            } else {
                Access::Read
            },
            device_id: entry.device_id,
            credential: entry.s,
        };
        let hash = tokens::hash(&token);
        let mut inner = self.inner.lock().unwrap();
        inner.token_hashes.insert(hash, grant.clone());
        tokens::save(&self.tokens_file, &inner.token_hashes).ok()?;
        Some((token, grant))
    }

    pub fn logout(&self, token: &str) {
        let hash = tokens::hash(token);
        let mut inner = self.inner.lock().unwrap();
        inner.token_hashes.remove(&hash);
        inner.bindings.remove(&hash);
        let _ = tokens::save(&self.tokens_file, &inner.token_hashes);
    }

    /// 校验浏览器令牌：有效返回授权（级别 + 绑定设备 + 凭证）
    pub fn validate(&self, token: &str) -> Option<tokens::Grant> {
        let hash = tokens::hash(token);
        self.inner.lock().unwrap().token_hashes.get(&hash).cloned()
    }

    // ---------- 会话 ----------

    /// 当前绑定的会话（代理消费）
    pub fn session_for(&self, token: &str) -> Option<Arc<ViewerSession>> {
        let hash = tokens::hash(token);
        let inner = self.inner.lock().unwrap();
        let session_token = inner.bindings.get(&hash)?;
        inner.sessions.get(session_token).cloned()
    }

    /// 当前绑定信息（API 展示）
    pub fn current(&self, token: &str) -> Option<Value> {
        self.session_for(token).map(|s| session_view(&s))
    }

    /// 打开（或复用）到绑定设备的查看会话：建会话 → 轮询 ready → iroh 拨号（身份即认证）。
    /// bt 在建令牌时已绑定唯一目标设备，target 只能等于它
    pub async fn open_session(
        self: &Arc<Self>,
        token: &str,
        target_device_id: &str,
        access: Access,
    ) -> Result<Value, String> {
        let Some(grant) = self.validate(token) else {
            return Err("未登录或令牌无效".to_string());
        };
        if target_device_id != grant.device_id {
            return Err("该令牌无权访问此设备".to_string());
        }
        if access == Access::Write && grant.access != Access::Write {
            return Err("当前令牌没有写入权限".to_string());
        }
        let bt_hash = tokens::hash(token);
        // 复用存活绑定（访问级别也必须一致）
        if let Some(existing) = self.session_for(token) {
            if existing.target_device_id == target_device_id
                && existing.access == access
                && existing.conn.close_reason().is_none()
            {
                return Ok(session_view(&existing));
            }
        }

        let source = self
            .identity
            .device_id
            .clone()
            .ok_or_else(|| "服务端设备身份未初始化".to_string())?;
        let (hawk_session_id, initial) = create_session_internal(
            &self.state,
            &source,
            target_device_id,
            access,
            Some(grant.credential.clone()),
        )
        .map_err(|e| e.message)?;
        if initial == SessionState::Rejected {
            return Err("对方设备已关闭远程访问".to_string());
        }

        // 轮询 ready（A 侧经 WSS 异步 accept）
        let connect = match poll_ready(&self.state, &hawk_session_id).await {
            Some(connect) => connect,
            None => {
                let state = self
                    .state
                    .sessions
                    .read()
                    .unwrap()
                    .get(&hawk_session_id)
                    .map(|session| session.state.as_str().to_string());
                let message = match state.as_deref() {
                    Some("rejected") => "对方设备拒绝会话（可能未开启广域网写入）".to_string(),
                    Some("unreachable") => "目标设备不可达".to_string(),
                    Some("expired") => "会话建立超时".to_string(),
                    Some("cancelled") => "会话已取消".to_string(),
                    _ => "会话建立超时或失败".to_string(),
                };
                // 带 session_id 落日志：与中继配对日志（同 id）可对齐时间线
                tracing::warn!(
                    "查看会话 {hawk_session_id} 建立失败（目标 {target_device_id}）: {message}"
                );
                return Err(message);
            }
        };

        // iroh 拨号（endpoint_id 即认证；直连地址与中继由传输层自治竞速）
        let addr = build_endpoint_addr(&connect)?;
        let conn = tokio::time::timeout(
            CONNECT_TIMEOUT,
            self.endpoint.connect(addr, crate::viewer::HAWK_ALPN),
        )
        .await
        .map_err(|_| {
            tracing::warn!("查看会话 {hawk_session_id} 隧道握手超时（{CONNECT_TIMEOUT:?}，目标 {target_device_id}）");
            "隧道握手超时".to_string()
        })?
        .map_err(|e| {
            tracing::warn!("查看会话 {hawk_session_id} 隧道握手失败: {e}");
            format!("隧道握手失败: {e}")
        })?;

        let (target_name, target_library_name) = self
            .state
            .device(target_device_id)
            .map(|d| (d.name, d.library_name))
            .unwrap_or_else(|| ("远端设备".to_string(), "素材库".to_string()));
        let session = Arc::new(ViewerSession {
            session_token: format!(
                "vs_{:016x}{:016x}",
                rand::random::<u64>(),
                rand::random::<u64>()
            ),
            hawk_session_id: hawk_session_id.clone(),
            access,
            target_device_id: target_device_id.to_string(),
            target_name,
            target_library_name,
            conn: conn.clone(),
        });

        // 新会话顶替旧绑定（旧会话尽力取消）
        let old = {
            let mut inner = self.inner.lock().unwrap();
            let old_token = inner
                .bindings
                .insert(bt_hash, session.session_token.clone());
            inner
                .sessions
                .insert(session.session_token.clone(), session.clone());
            old_token.and_then(|t| inner.sessions.remove(&t))
        };
        if let Some(old) = old {
            Self::cancel_hawk_session(&self.state, &old.hawk_session_id);
        }

        // 断线清理：隧道关闭 → 解除绑定、移除会话、收服务端会话
        {
            let manager = self.clone();
            let session_token = session.session_token.clone();
            let hawk_session_id = session.hawk_session_id.clone();
            tokio::spawn(async move {
                conn.closed().await;
                let removed = {
                    let mut inner = manager.inner.lock().unwrap();
                    inner.bindings.retain(|_, v| v != &session_token);
                    inner.sessions.remove(&session_token).is_some()
                };
                if removed {
                    tracing::info!("网页查看会话 {hawk_session_id} 隧道已断开");
                    Self::cancel_hawk_session(&manager.state, &hawk_session_id);
                }
            });
        }

        tracing::info!("网页查看会话 {hawk_session_id} 已建立（目标 {target_device_id}）");
        Ok(session_view(&session))
    }

    /// 断开当前绑定（幂等）：取消服务端会话并解除绑定
    pub async fn disconnect(&self, token: &str) {
        let hash = tokens::hash(token);
        let old = {
            let mut inner = self.inner.lock().unwrap();
            inner
                .bindings
                .remove(&hash)
                .and_then(|t| inner.sessions.remove(&t))
        };
        if let Some(old) = old {
            old.conn.close(0u32.into(), b"viewer disconnect");
            Self::cancel_hawk_session(&self.state, &old.hawk_session_id);
        }
    }

    /// 会话取消（服务端状态 + 中继关闭 + 通知），供顶替/断开/断线共用
    fn cancel_hawk_session(state: &Arc<AppState>, session_id: &str) {
        let (source, target) = {
            let mut sessions = state.sessions.write().unwrap();
            let Some(session) = sessions.get_mut(session_id) else {
                return;
            };
            if !session.state.is_terminal() {
                session.state = SessionState::Cancelled;
            }
            (
                session.source_device_id.clone(),
                session.target_device_id.clone(),
            )
        };
        state.close_relay(session_id);
        let cancelled = json!({ "type": "session.cancelled", "session_id": session_id });
        state.push_ws(&source, &cancelled);
        state.push_ws(&target, &cancelled);
    }
}

fn session_view(session: &Arc<ViewerSession>) -> Value {
    json!({
        "session_token": session.session_token,
        "access": session.access.as_str(),
        "target_device_id": session.target_device_id,
        "target_name": session.target_name,
        "target_library_name": session.target_library_name,
    })
}

/// 启动时确保「网页版」设备行存在且指纹最新（服务端以设备身份走既有协议发起会话）。
/// 返回使用的 device_id（identity 未持久化过时由调用方回填）
pub fn ensure_viewer_device(state: &Arc<AppState>, identity: &Identity) -> Result<String, String> {
    let mut store = state.store.lock().unwrap();
    let device_id = identity
        .device_id
        .clone()
        .unwrap_or_else(|| format!("d_{:016x}", rand::random::<u64>()));
    let exists = store.get(&device_id).is_some();
    if exists {
        store
            .update(&device_id, |rec| {
                // 「网页版」是全局设备（不属于任何租户池）：它作为 B 侧为全部租户撮合会话
                rec.tenant_id = String::new();
                rec.name = crate::identity::VIEWER_DEVICE_NAME.to_string();
                rec.platform = "server".to_string();
                rec.library_name = "网页查看器".to_string();
                rec.endpoint_id = identity.endpoint_id.clone();
                rec.allow_remote = false;
                rec.allow_write = false;
            })
            .map_err(|e| format!("设备注册表写入失败: {e}"))?;
    } else {
        store
            .insert(DeviceRecord {
                device_id: device_id.clone(),
                tenant_id: String::new(),
                name: crate::identity::VIEWER_DEVICE_NAME.to_string(),
                platform: "server".to_string(),
                endpoint_id: identity.endpoint_id.clone(),
                library_name: "网页查看器".to_string(),
                allow_remote: false,
                allow_write: false,
                registered_at_ms: crate::state::now_ms(),
            })
            .map_err(|e| format!("设备注册表写入失败: {e}"))?;
    }
    Ok(device_id)
}

/// 轮询会话 ready 并取出连接信息（A 侧 accept 后 connect 携带）
async fn poll_ready(state: &Arc<AppState>, session_id: &str) -> Option<Value> {
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    loop {
        let (state_str, connect) = {
            let sessions = state.sessions.read().unwrap();
            sessions
                .get(session_id)
                .map(|s| (s.state.as_str().to_string(), s.connect.clone()))
                .unwrap_or_default()
        };
        match state_str.as_str() {
            "ready" => return connect,
            "" => return None, // 会话不存在
            "rejected" | "unreachable" | "expired" | "cancelled" => return None,
            _ => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(READY_POLL).await;
    }
}

/// connect 对象 → iroh EndpointAddr（endpoint_id + 直连地址 + 可选中继）
fn build_endpoint_addr(connect: &Value) -> Result<iroh::EndpointAddr, String> {
    let endpoint_id: iroh::EndpointId = connect["endpoint_id"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| "连接信息缺少 endpoint_id".to_string())?;
    let mut addr = iroh::EndpointAddr::new(endpoint_id);
    if let Some(addrs) = connect["direct_addresses"].as_array() {
        for a in addrs {
            if let Some(sock) = a.as_str().and_then(|s| s.parse().ok()) {
                addr = addr.with_ip_addr(sock);
            }
        }
    }
    if let Some(url) = connect["relay"]["url"].as_str() {
        if let Ok(relay_url) = url.parse() {
            addr = addr.with_relay_url(relay_url);
        }
    }
    Ok(addr)
}

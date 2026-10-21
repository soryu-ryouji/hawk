//! 网页版宿主：hawk-server 作为 B 侧（协议中的查看方），任意浏览器经域名只读浏览素材库。
//! 隧道在服务器终结（素材明文过服务器，自托管单用户信任前提）；浏览器令牌（bt）与
//! access_key 分离：登录换发、仅查看权限、服务端存哈希。

pub mod api;
pub mod page;
pub mod proxy;
mod tokens;

use crate::api::sessions::create_session_internal;
use crate::identity::Identity;
use crate::relay_socket::RelaySocket;
use crate::state::{AppState, SessionState};
use crate::store::DeviceRecord;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 会话 ready 轮询（A 侧经 WSS 异步接受）
const READY_POLL: Duration = Duration::from_secs(1);
const READY_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub struct ViewerSession {
    pub session_token: String,
    pub hawk_session_id: String,
    pub target_device_id: String,
    pub target_name: String,
    pub target_library_name: String,
    pub conn: quinn::Connection,
    _endpoint: quinn::Endpoint,
}

struct ViewerInner {
    /// 有效浏览器令牌哈希（sha256 hex）
    token_hashes: HashSet<String>,
    /// session_token → 会话
    sessions: HashMap<String, Arc<ViewerSession>>,
    /// bt 哈希 → session_token（当前绑定）
    bindings: HashMap<String, String>,
}

pub struct ViewerManager {
    state: Arc<AppState>,
    identity: Identity,
    inner: Mutex<ViewerInner>,
    tokens_file: PathBuf,
}

impl ViewerManager {
    pub fn new(
        state: Arc<AppState>,
        identity: Identity,
        data_dir: &std::path::Path,
    ) -> Arc<ViewerManager> {
        let token_hashes = tokens::load(&data_dir.join("viewer_tokens.json"));
        Arc::new(ViewerManager {
            state,
            identity,
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

    // ---------- 浏览器令牌 ----------

    /// 登录：access_key 正确则换发浏览器令牌（bt），服务端存哈希
    pub fn login(&self, access_key: &str) -> Option<String> {
        if !crate::auth::constant_time_eq(access_key.as_bytes(), self.state.access_key.as_bytes()) {
            return None;
        }
        let token = format!(
            "hb_{:016x}{:016x}",
            rand::random::<u64>(),
            rand::random::<u64>()
        );
        let hash = tokens::hash(&token);
        let mut inner = self.inner.lock().unwrap();
        inner.token_hashes.insert(hash);
        tokens::save(&self.tokens_file, &inner.token_hashes).ok()?;
        Some(token)
    }

    pub fn logout(&self, token: &str) {
        let hash = tokens::hash(token);
        let mut inner = self.inner.lock().unwrap();
        inner.token_hashes.remove(&hash);
        inner.bindings.remove(&hash);
        let _ = tokens::save(&self.tokens_file, &inner.token_hashes);
    }

    /// 校验浏览器令牌：有效返回其哈希
    pub fn validate(&self, token: &str) -> Option<String> {
        let hash = tokens::hash(token);
        self.inner
            .lock()
            .unwrap()
            .token_hashes
            .contains(&hash)
            .then_some(hash)
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

    /// 打开（或复用）到目标设备的查看会话：建会话 → 轮询 ready → 进程内中继 → QUIC 钉扎连接
    pub async fn open_session(
        self: &Arc<Self>,
        token: &str,
        target_device_id: &str,
    ) -> Result<Value, String> {
        let Some(bt_hash) = self.validate(token) else {
            return Err("未登录或令牌无效".to_string());
        };
        // 复用存活绑定
        if let Some(existing) = self.session_for(token) {
            if existing.target_device_id == target_device_id
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
        let (hawk_session_id, initial) =
            create_session_internal(&self.state, &source, target_device_id, "relay")
                .map_err(|e| e.message)?;
        if initial == SessionState::Rejected {
            return Err("对方设备已关闭远程访问".to_string());
        }

        // 轮询 ready（A 侧经 WSS 异步 accept）
        let fingerprint = poll_ready(&self.state, &hawk_session_id)
            .await
            .ok_or("会话建立超时或失败".to_string())?;

        // 进程内中继 + QUIC 钉扎连接
        let socket = RelaySocket::register(self.state.clone(), &hawk_session_id);
        let client_config = crate::tls_client::client_config(&self.identity, fingerprint)?;
        let mut endpoint = quinn::Endpoint::new_with_abstract_socket(
            quinn::EndpointConfig::default(),
            None,
            Arc::new(socket),
            quinn::default_runtime().ok_or_else(|| "tokio runtime 不可用".to_string())?,
        )
        .map_err(|e| format!("中继端点创建失败: {e}"))?;
        endpoint.set_default_client_config(client_config);
        let connecting = endpoint
            .connect(crate::relay_socket::RELAY_ADDR, "hawk-remote")
            .map_err(|e| format!("中继隧道发起失败: {e}"))?;
        let conn = tokio::time::timeout(CONNECT_TIMEOUT, connecting)
            .await
            .map_err(|_| "中继隧道握手超时".to_string())?
            .map_err(|e| format!("中继隧道握手失败: {e}"))?;

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
            target_device_id: target_device_id.to_string(),
            target_name,
            target_library_name,
            conn: conn.clone(),
            _endpoint: endpoint,
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
                rec.name = crate::identity::VIEWER_DEVICE_NAME.to_string();
                rec.platform = "server".to_string();
                rec.library_name = "网页查看器".to_string();
                rec.fingerprint = identity.fingerprint.clone();
                rec.allow_remote = false;
            })
            .map_err(|e| format!("设备注册表写入失败: {e}"))?;
    } else {
        store
            .insert(DeviceRecord {
                device_id: device_id.clone(),
                name: crate::identity::VIEWER_DEVICE_NAME.to_string(),
                platform: "server".to_string(),
                fingerprint: identity.fingerprint.clone(),
                library_name: "网页查看器".to_string(),
                allow_remote: false,
                registered_at_ms: crate::state::now_ms(),
            })
            .map_err(|e| format!("设备注册表写入失败: {e}"))?;
    }
    Ok(device_id)
}

/// 轮询会话 ready 并取出目标指纹（A 侧 accept 后 connect 携带）
async fn poll_ready(state: &Arc<AppState>, session_id: &str) -> Option<[u8; 32]> {
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
            "ready" => {
                let hex_str = connect?["fingerprint"].as_str()?.to_string();
                return parse_fingerprint(&hex_str);
            }
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

fn parse_fingerprint(hex_str: &str) -> Option<[u8; 32]> {
    let bytes: Vec<u8> = (0..hex_str.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex_str.get(i..i + 2)?, 16).ok())
        .collect::<Option<_>>()?;
    bytes.try_into().ok()
}

//! 信令客户端：设备注册、周期心跳（30s）、WSS 控制通道（自动重连）。
//! 瞬时错误（网络抖动/服务端暂不可达）在模块内退避重试闭环；
//! 致命错误（401 key 无效、url 非法）返回给 supervisor 置错误状态，等配置变更重启。
//! 协议契约见 docs/backend/remote-protocol.md。

use crate::core::config::{LibraryConfig, RemoteSettings};
use crate::core::paths::LibraryPaths;
use crate::remote::identity::Identity;
use crate::remote::{RemoteState, RemoteStatus, Supervisor};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const RETRY_MIN: Duration = Duration::from_secs(5);
const RETRY_MAX: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// 信令错误：致命 = 重试无意义（等配置变更重启）；瞬时 = 退避重连
#[derive(Debug)]
enum SignalError {
    Fatal(String),
    Transient(String),
}

type Result<T> = std::result::Result<T, SignalError>;

/// 信令主任务（由 supervisor 拉起；返回即终态：致命错误已写入状态）
pub async fn run(
    paths: LibraryPaths,
    config: Arc<LibraryConfig>,
    cfg: RemoteSettings,
    supervisor: Arc<Supervisor>,
) {
    if let Err(SignalError::Fatal(msg)) = run_inner(&paths, &config, &cfg, &supervisor).await {
        tracing::error!("远程访问信令终止（等配置变更重启）: {msg}");
        supervisor.set_status(RemoteStatus {
            state: RemoteState::Error,
            server_url: Some(cfg.url.clone()),
            device_id: None,
            error: Some(msg),
        });
    }
}

async fn run_inner(
    paths: &LibraryPaths,
    config: &Arc<LibraryConfig>,
    cfg: &RemoteSettings,
    supervisor: &Arc<Supervisor>,
) -> Result<()> {
    let base = normalize_base(&cfg.url)?;
    let http = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| SignalError::Fatal(format!("HTTP 客户端构建失败: {e}")))?;
    let mut identity = Identity::load_or_create(&paths.remote_dir).map_err(SignalError::Fatal)?;

    // 注册：无 device_id 时（瞬时失败退避重试，直到成功或致命）
    let mut retry = RETRY_MIN;
    while identity.device_id.is_none() {
        match register(&http, &base, &cfg.key, paths, config, &identity).await {
            Ok(device_id) => {
                identity.device_id = Some(device_id);
                identity
                    .persist(&paths.remote_dir)
                    .map_err(SignalError::Fatal)?;
            }
            Err(SignalError::Fatal(msg)) => return Err(SignalError::Fatal(msg)),
            Err(SignalError::Transient(msg)) => {
                tracing::warn!("设备注册失败（{retry:?} 后重试）: {msg}");
                tokio::time::sleep(retry).await;
                retry = (retry * 2).min(RETRY_MAX);
            }
        }
    }
    let device_id = identity.device_id.clone().unwrap();
    tracing::info!("远程访问设备已注册: {device_id}（{base}）");

    // 主循环：WSS 控制通道 + 周期心跳；断线退避重连
    let mut retry = RETRY_MIN;
    loop {
        match channel_loop(&http, &base, &cfg.key, &device_id, supervisor).await {
            Ok(()) => retry = RETRY_MIN, // 对端正常关闭：立即重连
            Err(SignalError::Fatal(msg)) => return Err(SignalError::Fatal(msg)),
            Err(SignalError::Transient(msg)) => {
                tracing::warn!("控制通道断开（{retry:?} 后重连）: {msg}");
                supervisor.set_status(RemoteStatus {
                    state: RemoteState::Connecting,
                    server_url: Some(cfg.url.clone()),
                    device_id: Some(device_id.clone()),
                    error: None,
                });
            }
        }
        tokio::time::sleep(retry).await;
        retry = (retry * 2).min(RETRY_MAX);
    }
}

/// 一轮控制通道：WSS 连接 → hello/hello_ack → 心跳与推送处理，直到断开
async fn channel_loop(
    http: &reqwest::Client,
    base: &str,
    key: &str,
    device_id: &str,
    supervisor: &Arc<Supervisor>,
) -> Result<()> {
    let url = ws_url(base, key, device_id)?;
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| SignalError::Transient(format!("控制通道连接失败: {e}")))?;

    // 协议握手：服务端先 hello，客户端回 hello_ack
    let hello = tokio::time::timeout(REQUEST_TIMEOUT, ws.next())
        .await
        .map_err(|_| SignalError::Transient("等待服务端 hello 超时".to_string()))?
        .ok_or_else(|| SignalError::Transient("控制通道在握手前关闭".to_string()))?
        .map_err(|e| SignalError::Transient(format!("控制通道读取失败: {e}")))?;
    let hello: Value = serde_json::from_str(hello.to_text().unwrap_or_default())
        .map_err(|e| SignalError::Transient(format!("hello 消息解析失败: {e}")))?;
    if hello.get("type").and_then(|t| t.as_str()) != Some("hello") {
        return Err(SignalError::Transient(
            "控制通道首条消息不是 hello".to_string(),
        ));
    }
    ws.send(Message::Text(r#"{"type":"hello_ack"}"#.into()))
        .await
        .map_err(|e| SignalError::Transient(format!("hello_ack 发送失败: {e}")))?;

    supervisor.set_status(RemoteStatus {
        state: RemoteState::Online,
        server_url: Some(base.to_string()),
        device_id: Some(device_id.to_string()),
        error: None,
    });
    tracing::info!("远程访问控制通道已连接: {device_id}");

    // 连接建立即心跳一次（让自己在线），随后按周期间隔
    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                send_heartbeat(http, base, key, device_id).await?;
            }
            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_message(&mut ws, http, base, key, device_id, &text).await?;
                    }
                    Some(Ok(Message::Close(_))) => return Ok(()),
                    Some(Err(e)) => {
                        return Err(SignalError::Transient(format!("控制通道读取失败: {e}")));
                    }
                    None => return Err(SignalError::Transient("控制通道已关闭".to_string())),
                    _ => {}
                }
            }
        }
    }
}

/// 服务端推送处理：ping → pong；session.request → 自动接受（allow_remote 由服务端在登记时裁决）；
/// 未知 type 忽略（协议版本策略）
async fn handle_message(
    ws: &mut (impl SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin),
    http: &reqwest::Client,
    base: &str,
    key: &str,
    device_id: &str,
    text: &str,
) -> Result<()> {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Ok(()); // 无法解析的消息按未知处理（协议：未知忽略）
    };
    match value.get("type").and_then(|t| t.as_str()) {
        Some("ping") => {
            ws.send(Message::Text(r#"{"type":"pong"}"#.into()))
                .await
                .map_err(|e| SignalError::Transient(format!("pong 发送失败: {e}")))?;
        }
        Some("session.request") => {
            if let Some(session_id) = value.get("session_id").and_then(|s| s.as_str()) {
                accept_session(http, base, key, session_id, device_id).await?;
            }
        }
        _ => {}
    }
    Ok(())
}

async fn send_heartbeat(
    http: &reqwest::Client,
    base: &str,
    key: &str,
    device_id: &str,
) -> Result<()> {
    // 可达性上报为占位（none/null）：UPnP/IPv6 探测属阶段 1b，此前会话候选为空（unreachable）
    let body = json!({
        "device_id": device_id,
        "reachability": {
            "public_ip": null,
            "mapped_port": null,
            "mapping_type": "none",
            "public_ipv6": null,
            "ipv6_port": null,
            "nat_type": "unknown",
        }
    });
    let resp = http
        .post(format!("{base}/api/v1/devices/heartbeat"))
        .bearer_auth(key)
        .json(&body)
        .send()
        .await
        .map_err(|e| SignalError::Transient(format!("心跳发送失败: {e}")))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(response_error(resp, "心跳").await)
    }
}

async fn accept_session(
    http: &reqwest::Client,
    base: &str,
    key: &str,
    session_id: &str,
    device_id: &str,
) -> Result<()> {
    let resp = http
        .post(format!("{base}/api/v1/sessions/{session_id}/accept"))
        .bearer_auth(key)
        .json(&json!({ "device_id": device_id }))
        .send()
        .await
        .map_err(|e| SignalError::Transient(format!("会话接受失败: {e}")))?;
    // 409（会话已结束/过期）属正常竞态：不致命、不重连
    if resp.status() == reqwest::StatusCode::CONFLICT {
        tracing::debug!("会话 {session_id} 接受时已结束");
        return Ok(());
    }
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(response_error(resp, "会话接受").await)
    }
}

async fn register(
    http: &reqwest::Client,
    base: &str,
    key: &str,
    paths: &LibraryPaths,
    config: &Arc<LibraryConfig>,
    identity: &Identity,
) -> Result<String> {
    let hostname = gethostname::gethostname();
    let hostname = hostname.to_string_lossy();
    let name = if hostname.trim().is_empty() {
        "未命名设备".to_string()
    } else {
        hostname.trim().to_string()
    };
    let library_name = config.current().name.unwrap_or_else(|| {
        std::path::Path::new(&paths.root)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "素材库".to_string())
    });
    let body = json!({
        "name": name,
        "platform": std::env::consts::OS,
        "fingerprint": identity.fingerprint,
        "library_name": library_name,
        "allow_remote": true,
    });
    let resp = http
        .post(format!("{base}/api/v1/devices"))
        .bearer_auth(key)
        .json(&body)
        .send()
        .await
        .map_err(|e| SignalError::Transient(format!("注册请求失败: {e}")))?;
    if !resp.status().is_success() {
        return Err(response_error(resp, "注册").await);
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| SignalError::Transient(format!("注册响应解析失败: {e}")))?;
    body["data"]["device_id"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| SignalError::Transient("注册响应缺少 device_id".to_string()))
}

/// 非成功响应归一为信令错误：401 致命（key 无效）；其余瞬时（message 直接展示，协议约定）
async fn response_error(resp: reqwest::Response, what: &str) -> SignalError {
    let status = resp.status();
    let message = resp
        .json::<Value>()
        .await
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(String::from))
        .unwrap_or_else(|| status.to_string());
    if status == reqwest::StatusCode::UNAUTHORIZED {
        SignalError::Fatal(format!("{what}被服务端拒绝: {message}"))
    } else {
        SignalError::Transient(format!("{what}失败（{status}）: {message}"))
    }
}

/// base url 归一：去尾斜杠，强制 http(s):// 前缀
fn normalize_base(url: &str) -> Result<String> {
    let base = url.trim().trim_end_matches('/');
    if base.starts_with("http://") || base.starts_with("https://") {
        Ok(base.to_string())
    } else {
        Err(SignalError::Fatal(
            "remote url 需以 http:// 或 https:// 开头".to_string(),
        ))
    }
}

/// REST base → WSS 控制通道地址（查询参数编码经 Url 处理）
fn ws_url(base: &str, key: &str, device_id: &str) -> Result<String> {
    let mut url = reqwest::Url::parse(base)
        .map_err(|e| SignalError::Fatal(format!("remote url 解析失败: {e}")))?;
    url.set_path("/api/v1/ws");
    url.query_pairs_mut()
        .append_pair("key", key)
        .append_pair("device_id", device_id);
    let scheme = if base.starts_with("https://") {
        "wss"
    } else {
        "ws"
    };
    url.set_scheme(scheme)
        .map_err(|_| SignalError::Fatal("remote url scheme 非法".to_string()))?;
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_base_requires_http_scheme() {
        assert!(normalize_base("https://hawk.example.com/").is_ok());
        assert_eq!(normalize_base("http://a:27370/").unwrap(), "http://a:27370");
        assert!(normalize_base("hawk.example.com").is_err());
    }

    #[test]
    fn ws_url_switches_scheme_and_encodes_query() {
        let url = ws_url("https://hawk.example.com", "k+e/y=", "d_1").unwrap();
        assert_eq!(
            url,
            "wss://hawk.example.com/api/v1/ws?key=k%2Be%2Fy%3D&device_id=d_1"
        );
        let url = ws_url("http://127.0.0.1:27370", "abc", "d_1").unwrap();
        assert_eq!(url, "ws://127.0.0.1:27370/api/v1/ws?key=abc&device_id=d_1");
    }
}

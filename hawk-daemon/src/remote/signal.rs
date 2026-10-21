//! 信令客户端：设备注册、周期心跳（30s）、WSS 控制通道（自动重连）。
//! 瞬时错误（网络抖动/服务端暂不可达）在模块内退避重试闭环；
//! 致命错误（401 key 无效、url 非法）返回给 supervisor 置错误状态，等配置变更重启。
//! 协议契约见 docs/backend/remote-protocol.md。

use crate::core::config::LibraryConfig;
use crate::core::paths::LibraryPaths;
use crate::remote::config::{RemoteConfigStore, RemoteSettings};
use crate::remote::data::DataPlane;
use crate::remote::identity::Identity;
use crate::remote::reachability::{self, SharedUpnpReport, UpnpReport};
use crate::remote::{RemoteState, RemoteStatus, Supervisor};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
/// 通道重连退避：首次 1s（快恢复优先），指数增长至 60s
const CHANNEL_RETRY_MIN: Duration = Duration::from_secs(1);

/// 心跳间隔：测试可经 HAWK_REMOTE_HEARTBEAT_MS 覆盖（协议约定 30s，此处仅为测试提速）
fn heartbeat_interval() -> Duration {
    std::env::var("HAWK_REMOTE_HEARTBEAT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(HEARTBEAT_INTERVAL)
}
const RETRY_MIN: Duration = Duration::from_secs(5);
const RETRY_MAX: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// 接受会话后等待对端连接的上限：超时未连收回放行（防放行残留）
const ACCEPT_PEER_TIMEOUT: Duration = Duration::from_secs(60);

/// 信令错误：致命 = 重试无意义（等配置变更重启）；瞬时 = 退避重连
#[derive(Debug)]
enum SignalError {
    Fatal(String),
    Transient(String),
    /// 服务端已不再认识本设备（设备表丢失/被注销）→ 清除 device_id 重注册
    Unregistered,
}

type Result<T> = std::result::Result<T, SignalError>;

/// 信令主任务（由 supervisor 拉起；返回即终态：致命错误已写入状态）。
/// router 为 daemon API 路由克隆：A 侧隧道端进程内转发的目标
pub async fn run(
    paths: LibraryPaths,
    library: Arc<LibraryConfig>,
    remote: Arc<RemoteConfigStore>,
    cfg: RemoteSettings,
    supervisor: Arc<Supervisor>,
    router: axum::Router,
) {
    if let Err(SignalError::Fatal(msg)) =
        run_inner(&paths, &library, &remote, &cfg, &supervisor, router).await
    {
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
    library: &Arc<LibraryConfig>,
    remote: &Arc<RemoteConfigStore>,
    cfg: &RemoteSettings,
    supervisor: &Arc<Supervisor>,
    router: axum::Router,
) -> Result<()> {
    let base = normalize_base(&cfg.url)?;
    let http = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| SignalError::Fatal(format!("HTTP 客户端构建失败: {e}")))?;
    let mut identity =
        Identity::load_or_create(remote.identity_dir()).map_err(SignalError::Fatal)?;

    // 数据面：绑定 QUIC 端口（A 侧 accept 循环），拉起 UPnP 映射维护（重注册循环复用，不重复绑定）。
    // 两者都装在守卫里：热重启（supervisor abort）或停用时必须随任务一起收尾，
    // 否则 UDP 端口被本进程泄漏，下次激活直接绑定失败
    let data_plane = PlaneGuard(bind_data_plane(&identity, cfg, router.clone()).await?);
    let upnp_report: SharedUpnpReport = Arc::new(RwLock::new(UpnpReport::default()));
    let _upnp = TaskGuard(tokio::spawn(reachability::maintain_upnp(
        cfg.quic_port,
        upnp_report.clone(),
    )));

    let mut retry = RETRY_MIN;
    let mut chan_retry = CHANNEL_RETRY_MIN;
    loop {
        // 注册（含服务端设备表丢失后的重注册）：瞬时失败退避重试，直到成功或致命
        while identity.device_id.is_none() {
            match register(
                &http,
                &base,
                &cfg.service_key,
                paths,
                library,
                remote,
                &identity,
            )
            .await
            {
                Ok(device_id) => {
                    tracing::info!("远程访问设备已注册: {device_id}（{base}）");
                    identity.device_id = Some(device_id);
                    identity
                        .persist(remote.identity_dir())
                        .map_err(SignalError::Fatal)?;
                }
                Err(SignalError::Fatal(msg)) => return Err(SignalError::Fatal(msg)),
                Err(SignalError::Transient(msg)) => {
                    tracing::warn!("设备注册失败（{retry:?} 后重试）: {msg}");
                    tokio::time::sleep(retry).await;
                    retry = (retry * 2).min(RETRY_MAX);
                }
                Err(SignalError::Unregistered) => {
                    unreachable!("注册阶段不产生 Unregistered")
                }
            }
        }
        let device_id = identity.device_id.clone().unwrap();

        let ctx = ChannelContext {
            http: &http,
            base: &base,
            key: &cfg.service_key,
            device_id: &device_id,
            identity: &identity,
            data_plane: &data_plane.0,
            upnp_report: &upnp_report,
            quic_port: cfg.quic_port,
            router: &router,
            library,
            library_root: &paths.root,
            remote,
            keys: &supervisor.keys,
        };
        match channel_loop(&ctx, supervisor).await {
            // 对端正常关闭：立即重连（无退避、不闪「接入中」）
            Ok(()) => {
                chan_retry = CHANNEL_RETRY_MIN;
                continue;
            }
            Err(SignalError::Unregistered) => {
                // 服务端设备表丢失/被注销：清除 device_id，回到注册阶段
                tracing::warn!("服务端已不再认识本设备（{device_id}），重新注册");
                identity.device_id = None;
                identity
                    .persist(remote.identity_dir())
                    .map_err(SignalError::Fatal)?;
                retry = RETRY_MIN;
                chan_retry = CHANNEL_RETRY_MIN;
                continue;
            }
            Err(SignalError::Fatal(msg)) => return Err(SignalError::Fatal(msg)),
            Err(SignalError::Transient(msg)) => {
                tracing::warn!("控制通道断开（{chan_retry:?} 后重连）: {msg}");
                // 「接入中」带上断开原因，设置面板可直接看到发生了什么
                supervisor.set_status(RemoteStatus {
                    state: RemoteState::Connecting,
                    server_url: Some(cfg.url.clone()),
                    device_id: Some(device_id.clone()),
                    error: Some(msg),
                });
                tokio::time::sleep(chan_retry).await;
                chan_retry = (chan_retry * 2).min(RETRY_MAX);
            }
        }
    }
}

/// QUIC 端口绑定：重试次数 × 间隔（上一轮热重启的 socket 可能仍在关闭中）
const BIND_ATTEMPTS: u32 = 100;
const BIND_RETRY: Duration = Duration::from_millis(20);

/// 数据面守卫：无论正常返回还是被 abort（配置变更热重启），都关停 QUIC 端点释放端口
struct PlaneGuard(Arc<DataPlane>);

impl Drop for PlaneGuard {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

/// 附属任务守卫：随信令任务一起收尾（UPnP 维护循环是无限循环，不主动中止会随热重启累积）
struct TaskGuard(tokio::task::JoinHandle<()>);

impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// 绑定 QUIC 端口（带短暂重试）：端口可能刚由上一轮任务释放，立即重绑会瞬时 EADDRINUSE
async fn bind_data_plane(
    identity: &Identity,
    cfg: &RemoteSettings,
    router: axum::Router,
) -> Result<Arc<DataPlane>> {
    let mut last = String::new();
    for attempt in 0..BIND_ATTEMPTS {
        match DataPlane::bind(identity, cfg.quic_port, router.clone()) {
            Ok(plane) => return Ok(plane),
            Err(e) => last = e,
        }
        if attempt + 1 < BIND_ATTEMPTS {
            tokio::time::sleep(BIND_RETRY).await;
        }
    }
    Err(SignalError::Fatal(last))
}

/// 控制通道上下文（收敛 handler 参数）
struct ChannelContext<'a> {
    http: &'a reqwest::Client,
    base: &'a str,
    /// service-key（池门禁凭证）
    key: &'a str,
    device_id: &'a str,
    identity: &'a Identity,
    data_plane: &'a Arc<DataPlane>,
    upnp_report: &'a SharedUpnpReport,
    quic_port: u16,
    router: &'a axum::Router,
    library: &'a Arc<LibraryConfig>,
    library_root: &'a str,
    remote: &'a Arc<RemoteConfigStore>,
    /// 访问密钥本地存储（查看者会话执法 + 接入后推送 server）
    keys: &'a Arc<crate::remote::keys::KeyStore>,
}

/// 一轮控制通道：WSS 连接 → hello/hello_ack → 心跳与推送处理，直到断开
async fn channel_loop(ctx: &ChannelContext<'_>, supervisor: &Arc<Supervisor>) -> Result<()> {
    let (base, key, device_id) = (ctx.base, ctx.key, ctx.device_id);
    let request = ws_request(base, key, device_id)?;
    let (mut ws, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(ws_connect_error)?;

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

    // 接入成功：全量推送本机访问密钥映射到 server（覆盖式幂等；失败等下次变更/重连重试）
    {
        let keys = ctx.keys.clone();
        let http = ctx.http.clone();
        let base = base.to_string();
        let service_key = key.to_string();
        let device_id = device_id.to_string();
        tokio::spawn(async move {
            if let Err(e) = keys
                .sync_to_server(&http, &base, &service_key, &device_id)
                .await
            {
                tracing::warn!("访问密钥同步失败（下次变更或重连时重试）: {e}");
            }
        });
    }

    // 通道（重）建立：上一轮会话的放行与连接全部失效（B 侧会感知断开自行收尾）
    ctx.data_plane.reset_peers();
    // 本会话周期内接受的对端：session_id → 对端指纹（cancelled 时收回放行）
    let mut accepted_peers: std::collections::HashMap<String, [u8; 32]> =
        std::collections::HashMap::new();

    // 连接建立即心跳一次（让自己在线），随后按周期间隔
    let mut heartbeat = tokio::time::interval(heartbeat_interval());
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                send_heartbeat(ctx).await?;
            }
            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_message(&mut ws, ctx, &mut accepted_peers, &text).await?;
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

/// 服务端推送处理：ping → pong；session.request → 放行对端指纹并自动接受
/// （allow_remote 由服务端在登记时裁决）；session.cancelled → 收回放行；未知 type 忽略
async fn handle_message(
    ws: &mut (impl SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin),
    ctx: &ChannelContext<'_>,
    accepted_peers: &mut std::collections::HashMap<String, [u8; 32]>,
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
                let requested_write = value.get("access").and_then(|a| a.as_str()) == Some("write");
                // 本地执法（权限以本机 key 映射为准，server 声称的 access 只是参考）：
                // - 查看者会话（携带 key 验证子）：key 必须存在且权限覆盖请求；
                //   写会话另需本机 writable 总开关
                // - app 间会话（无验证子）：不开放写（写只对 key 持有者开放）
                let credential = value.get("credential").and_then(|c| c.as_str());
                let writable = match credential {
                    Some(hash) => match ctx.keys.resolve(hash) {
                        Some(entry) => {
                            if requested_write && !(entry.write && ctx.remote.current().writable) {
                                tracing::info!(
                                    "拒绝广域网写会话 {session_id}：key 无写权限或未开启 writable"
                                );
                                reject_session(
                                    ctx.http,
                                    ctx.base,
                                    ctx.key,
                                    session_id,
                                    ctx.device_id,
                                )
                                .await?;
                                return Ok(());
                            }
                            requested_write
                        }
                        None => {
                            tracing::info!("拒绝查看者会话 {session_id}：访问密钥已吊销");
                            reject_session(ctx.http, ctx.base, ctx.key, session_id, ctx.device_id)
                                .await?;
                            return Ok(());
                        }
                    },
                    None => {
                        if requested_write {
                            tracing::info!("拒绝广域网写会话 {session_id}：写会话必须携带访问密钥");
                            reject_session(ctx.http, ctx.base, ctx.key, session_id, ctx.device_id)
                                .await?;
                            return Ok(());
                        }
                        false
                    }
                };

                // 对端指纹随 session.request 下发（服务端持有双方设备记录），
                // 先放行（B 在 ready 后可能立即连接）再回 accept
                let fingerprint = value["from"]["fingerprint"]
                    .as_str()
                    .and_then(crate::remote::tls::parse_fingerprint);
                match fingerprint {
                    Some(fingerprint) => {
                        ctx.data_plane
                            .allow_session(session_id, fingerprint, writable);
                        accepted_peers.insert(session_id.to_string(), fingerprint);
                        // 接受后超时清理：60s 内对端未连接 → 收回放行（幂等，
                        // cancelled/重连时的重复收回无害）
                        let plane = ctx.data_plane.clone();
                        let session_id = session_id.to_string();
                        tokio::spawn(async move {
                            if !plane
                                .await_first_conn(&session_id, ACCEPT_PEER_TIMEOUT)
                                .await
                            {
                                plane.disallow_session(&session_id);
                            }
                        });
                    }
                    None => {
                        if writable {
                            reject_session(ctx.http, ctx.base, ctx.key, session_id, ctx.device_id)
                                .await?;
                            return Ok(());
                        }
                        // 只读会话指纹缺失不阻断：接受后对端连不上会自行超时
                        tracing::warn!("会话对端指纹缺失（接受但不放行）");
                    }
                }
                if let Err(e) =
                    accept_session(ctx.http, ctx.base, ctx.key, session_id, ctx.device_id).await
                {
                    ctx.data_plane.disallow_session(session_id);
                    accepted_peers.remove(session_id);
                    return Err(e);
                }

                // 中继模式：accept 后经服务端中继 WS 建立 QUIC（role=a 目标设备）
                if value.get("mode").and_then(|m| m.as_str()) == Some("relay") {
                    match crate::remote::relay::relay_ws_request(
                        ctx.base,
                        ctx.key,
                        ctx.device_id,
                        session_id,
                        "a",
                    ) {
                        Ok(request) => {
                            let identity = ctx.identity.clone();
                            let plane = ctx.data_plane.clone();
                            let router = ctx.router.clone();
                            let session_id = session_id.to_string();
                            tokio::spawn(async move {
                                match tokio_tungstenite::connect_async(request).await {
                                    Ok((ws, _)) => {
                                        tracing::info!("中继会话 {session_id}：A 侧通道已连接");
                                        if let Err(e) = crate::remote::relay::serve_over_ws(
                                            &identity, &plane, ws, router,
                                        )
                                        .await
                                        {
                                            tracing::warn!(
                                                "中继会话 {session_id} 服务端点失败: {e}"
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("中继会话 {session_id} 通道连接失败: {e}");
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            tracing::warn!("中继地址构建失败: {e}");
                        }
                    }
                }
            }
        }
        Some("session.cancelled") => {
            if let Some(session_id) = value.get("session_id").and_then(|s| s.as_str()) {
                accepted_peers.remove(session_id);
                ctx.data_plane.disallow_session(session_id);
            }
        }
        _ => {}
    }
    Ok(())
}

/// 当前库显示名：库内配置的 name，缺失回退目录名。注册与心跳共用，
/// 设备身份跨库共享后由心跳保持服务端设备表的库名最新
fn library_name(config: &Arc<LibraryConfig>, root: &str) -> String {
    config.current().name.unwrap_or_else(|| {
        std::path::Path::new(root)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "素材库".to_string())
    })
}

async fn send_heartbeat(ctx: &ChannelContext<'_>) -> Result<()> {
    // 可达性：UPnP 映射（维护任务异步更新）+ 全局 IPv6（每次心跳探测，零成本）。
    // public_ip 不自报——以服务端 public_ip_seen 为准
    let mapped_port = ctx.upnp_report.read().unwrap().mapped_port;
    let (public_ipv6, ipv6_port) = reachability::probe_ipv6(ctx.base, ctx.quic_port)
        .map(|(ip, port)| (Some(ip), Some(port)))
        .unwrap_or((None, None));
    let body = json!({
        "device_id": ctx.device_id,
        // 设备身份应用级：切换素材库后靠心跳把服务端设备表的库名收敛到当前库
        "library_name": library_name(ctx.library, ctx.library_root),
        "allow_write": ctx.remote.current().writable,
        "reachability": {
            "public_ip": null,
            "mapped_port": mapped_port,
            "mapping_type": if mapped_port.is_some() { "upnp" } else { "none" },
            "public_ipv6": public_ipv6,
            "ipv6_port": ipv6_port,
            "nat_type": "unknown",
        }
    });
    let resp = ctx
        .http
        .post(format!("{}/api/v1/devices/heartbeat", ctx.base))
        .bearer_auth(ctx.key)
        .json(&body)
        .send()
        .await
        .map_err(|e| SignalError::Transient(format!("心跳发送失败: {e}")))?;
    if resp.status().is_success() {
        Ok(())
    } else if resp.status() == reqwest::StatusCode::NOT_FOUND {
        // 设备不存在：服务端设备表丢失/被注销 → 触发重注册
        Err(SignalError::Unregistered)
    } else if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        // 429 是良性竞态：重连后的即时心跳可能落在上一跳 10s 最小间隔内。
        // 只记日志不拆通道——下一跳（30s 后）自然恢复，通道保持在线
        tracing::debug!("心跳被服务端限速（10s 最小间隔），跳过本跳");
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

async fn reject_session(
    http: &reqwest::Client,
    base: &str,
    key: &str,
    session_id: &str,
    device_id: &str,
) -> Result<()> {
    let resp = http
        .post(format!("{base}/api/v1/sessions/{session_id}/reject"))
        .bearer_auth(key)
        .json(&json!({ "device_id": device_id }))
        .send()
        .await
        .map_err(|e| SignalError::Transient(format!("会话拒绝失败: {e}")))?;
    if resp.status() == reqwest::StatusCode::CONFLICT {
        tracing::debug!("会话 {session_id} 拒绝时已结束");
        return Ok(());
    }
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(response_error(resp, "会话拒绝").await)
    }
}

async fn register(
    http: &reqwest::Client,
    base: &str,
    key: &str,
    paths: &LibraryPaths,
    library: &Arc<LibraryConfig>,
    remote: &Arc<RemoteConfigStore>,
    identity: &Identity,
) -> Result<String> {
    let hostname = gethostname::gethostname();
    let hostname = hostname.to_string_lossy();
    let name = if hostname.trim().is_empty() {
        "未命名设备".to_string()
    } else {
        hostname.trim().to_string()
    };
    let body = json!({
        "name": name,
        "platform": std::env::consts::OS,
        "fingerprint": identity.fingerprint,
        "library_name": library_name(library, &paths.root),
        "allow_remote": true,
        "allow_write": remote.current().writable,
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

/// 控制通道握手失败归一：404 = 服务端不认识本设备（设备表丢失/服务端换了数据目录）→ 重注册；
/// 401/403 = key 失效 → 致命；其余瞬时重连。
/// 若不区分，404 会被当成网络抖动无限重连，状态永远停在「接入中」（心跳在通道内部发，
/// 没通道就不会发现 404，只能靠这里触发重注册）
fn ws_connect_error(e: tokio_tungstenite::tungstenite::Error) -> SignalError {
    if let tokio_tungstenite::tungstenite::Error::Http(resp) = &e {
        match resp.status().as_u16() {
            404 => return SignalError::Unregistered,
            401 | 403 => return SignalError::Fatal(format!("控制通道鉴权失败: {e}")),
            _ => {}
        }
    }
    SignalError::Transient(format!("控制通道连接失败: {e}"))
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

/// REST base → WSS 控制通道请求。service-key 走 Authorization 头而非 URL 查询串
/// （反代 access log 会记录完整 URL，查询串带 key 等于明文落日志）
fn ws_request(
    base: &str,
    key: &str,
    device_id: &str,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut url = reqwest::Url::parse(base)
        .map_err(|e| SignalError::Fatal(format!("remote url 解析失败: {e}")))?;
    url.set_path("/api/v1/ws");
    url.query_pairs_mut().append_pair("device_id", device_id);
    let scheme = if base.starts_with("https://") {
        "wss"
    } else {
        "ws"
    };
    url.set_scheme(scheme)
        .map_err(|_| SignalError::Fatal("remote url scheme 非法".to_string()))?;
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|e| SignalError::Fatal(format!("控制通道请求构建失败: {e}")))?;
    request.headers_mut().insert(
        tokio_tungstenite::tungstenite::http::header::AUTHORIZATION,
        format!("Bearer {key}")
            .parse()
            .map_err(|_| SignalError::Fatal("service-key 含非法字符".to_string()))?,
    );
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_base_requires_http_scheme() {
        assert!(normalize_base("https://hawk.example.com/").is_ok());
        assert_eq!(normalize_base("http://a:9100/").unwrap(), "http://a:9100");
        assert!(normalize_base("hawk.example.com").is_err());
    }

    #[test]
    fn ws_request_switches_scheme_and_carries_bearer_header() {
        let req = ws_request("https://hawk.example.com", "k+e/y=", "d_1").unwrap();
        assert_eq!(
            req.uri().to_string(),
            "wss://hawk.example.com/api/v1/ws?device_id=d_1"
        );
        assert_eq!(req.headers().get("authorization").unwrap(), "Bearer k+e/y=");
        let req = ws_request("http://127.0.0.1:9100", "abc", "d_1").unwrap();
        assert_eq!(
            req.uri().to_string(),
            "ws://127.0.0.1:9100/api/v1/ws?device_id=d_1"
        );
    }
}

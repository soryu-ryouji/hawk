//! 信令客户端：设备注册、周期心跳（30s）、WSS 控制通道（自动重连）。
//! 瞬时错误（网络抖动/服务端暂不可达）在模块内退避重试闭环；
//! 致命错误（401 key 无效、url 非法）返回给 supervisor 置错误状态，等配置变更重启。
//! 协议契约见 docs/backend/remote-protocol.md。

use crate::core::access::Role;
use crate::remote::config::{RemoteConfigStore, RemoteSettings};
use crate::remote::data::DataPlane;
use crate::remote::identity::Identity;
use crate::remote::{LibraryContext, RemoteState, RemoteStatus, Supervisor};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
/// 通道重连退避：首次 1s（快恢复优先），指数增长至 60s
const CHANNEL_RETRY_MIN: Duration = Duration::from_secs(1);
/// 通道活性看门狗：服务端保活 ping 周期 30s（协议约定），超过该时长收不到任何服务端帧
/// 即判定连接半开（反代切换/NAT 会话老化可致下行静默死亡、上行 REST 心跳照常），主动断开重连
const CHANNEL_SILENCE_TIMEOUT: Duration = Duration::from_secs(90);

/// 看门狗时长：测试可经 HAWK_REMOTE_CHANNEL_SILENCE_MS 覆盖（同 heartbeat_interval 的提速约定）
fn channel_silence_timeout() -> Duration {
    std::env::var("HAWK_REMOTE_CHANNEL_SILENCE_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(CHANNEL_SILENCE_TIMEOUT)
}

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
    /// 服务端中继配置变化（心跳下发的 relay 字段变更）→ 重建数据面后重连通道
    RelayChanged(Option<(String, String)>),
}

type Result<T> = std::result::Result<T, SignalError>;

/// 信令主任务（由 supervisor 拉起；返回即终态：致命错误已写入状态）。
/// router 为 daemon API 路由克隆：A 侧隧道端进程内转发的目标；
/// library 为本地库上下文（None = 远程查看模式：不可被访问、不上报写权限）
pub async fn run(
    library: Option<LibraryContext>,
    remote: Arc<RemoteConfigStore>,
    cfg: RemoteSettings,
    supervisor: Arc<Supervisor>,
    router: axum::Router,
) {
    if let Err(SignalError::Fatal(msg)) =
        run_inner(&library, &remote, &cfg, &supervisor, router).await
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
    library: &Option<LibraryContext>,
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

    // 数据面：绑定 iroh endpoint（A 侧接受与 B 侧拨号共用）。装在守卫里：热重启
    // （supervisor abort）或停用时必须随任务一起收尾并清出拨号槽位，
    // 否则 UDP 端口被本进程泄漏，下次激活直接绑定失败。
    // relay 配置来自服务端心跳下发；变化时经 RelayChanged 重建数据面
    let mut current_relay: Option<(String, String)> = None;
    let mut data_plane = PlaneGuard::new(
        bind_data_plane(&identity, cfg, None, router.clone()).await?,
        supervisor.endpoint_slot().clone(),
    );

    let mut retry = RETRY_MIN;
    let mut chan_retry = CHANNEL_RETRY_MIN;
    loop {
        // 注册（含服务端设备表丢失后的重注册）：瞬时失败退避重试，直到成功或致命
        while identity.device_id.is_none() {
            match register(&http, &base, &cfg.service_key, library, remote, &identity).await {
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
                Err(SignalError::Unregistered) | Err(SignalError::RelayChanged(_)) => {
                    unreachable!("注册阶段不产生 Unregistered/RelayChanged")
                }
            }
        }
        let device_id = identity.device_id.clone().unwrap();

        let ctx = ChannelContext {
            current_relay: &mut current_relay,
            http: &http,
            base: &base,
            key: &cfg.service_key,
            device_id: &device_id,
            data_plane: &data_plane.plane,
            library,
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
            Err(SignalError::RelayChanged(relay)) => {
                // 服务端中继配置变化：重建数据面（旧连接断开由 B 侧重拨兜底）后立刻重连
                tracing::info!("服务端中继配置变化，重建数据面");
                data_plane
                    .rebuild(&identity, cfg, relay.clone(), router.clone())
                    .await?;
                current_relay = relay;
                chan_retry = CHANNEL_RETRY_MIN;
                continue;
            }
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

/// 数据面守卫：随信令任务收尾（关停 endpoint 并清出拨号槽位），热重启 abort 同理
struct PlaneGuard {
    plane: Arc<DataPlane>,
    slot: crate::remote::session::EndpointSlot,
}

impl PlaneGuard {
    fn new(plane: Arc<DataPlane>, slot: crate::remote::session::EndpointSlot) -> PlaneGuard {
        *slot.write().unwrap() = Some(plane.clone());
        PlaneGuard { plane, slot }
    }

    /// 中继配置变化重建：关停旧数据面（旧连接断开由 B 侧重拨兜底），绑定新配置并重新发布槽位
    async fn rebuild(
        &mut self,
        identity: &Identity,
        cfg: &RemoteSettings,
        relay: Option<(String, String)>,
        router: axum::Router,
    ) -> Result<()> {
        self.plane.shutdown();
        *self.slot.write().unwrap() = None;
        self.plane = bind_data_plane(identity, cfg, relay, router).await?;
        *self.slot.write().unwrap() = Some(self.plane.clone());
        // 旧会话放行随数据面重建失效（进行中的对端连接死亡，B 侧重拨兜底）
        Ok(())
    }
}

impl Drop for PlaneGuard {
    fn drop(&mut self) {
        self.plane.shutdown();
        *self.slot.write().unwrap() = None;
    }
}

/// 绑定隧道端口（带短暂重试）：端口可能刚由上一轮任务释放，立即重绑会瞬时 EADDRINUSE
async fn bind_data_plane(
    identity: &Identity,
    cfg: &RemoteSettings,
    relay: Option<(String, String)>,
    router: axum::Router,
) -> Result<Arc<DataPlane>> {
    let mut last = String::new();
    for attempt in 0..BIND_ATTEMPTS {
        match DataPlane::bind(identity, cfg.quic_port, relay.clone(), router.clone()).await {
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
    /// 当前生效的中继配置（心跳下发；变化触发 RelayChanged 重建数据面）
    current_relay: &'a mut Option<(String, String)>,
    http: &'a reqwest::Client,
    base: &'a str,
    /// service-key（池门禁凭证）
    key: &'a str,
    device_id: &'a str,
    data_plane: &'a Arc<DataPlane>,
    /// 本地库上下文（None = 远程查看模式）
    library: &'a Option<LibraryContext>,
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
    let mut accepted_peers: std::collections::HashMap<String, iroh::EndpointId> =
        std::collections::HashMap::new();

    // 连接建立即心跳一次（让自己在线），随后按周期间隔
    let mut heartbeat = tokio::time::interval(heartbeat_interval());
    // 活性看门狗：任何服务端帧（含保活 ping）都刷新活性；静默超时即半开，主动重连。
    // 没有它，半开连接会让设备永远「假在线」：REST 心跳照常，推送（session.request 等）全丢
    let silence = channel_silence_timeout();
    let mut watchdog = tokio::time::interval((silence / 4).max(Duration::from_millis(100)));
    let mut last_recv = std::time::Instant::now();
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if let HeartbeatReply::Ok(relay) = send_heartbeat(ctx).await? {
                    if relay != *ctx.current_relay {
                        return Err(SignalError::RelayChanged(relay));
                    }
                }
            }
            msg = ws.next() => {
                match msg {
                    Some(Ok(frame)) => {
                        last_recv = std::time::Instant::now();
                        match frame {
                            Message::Text(text) => {
                                handle_message(&mut ws, ctx, &mut accepted_peers, &text).await?;
                            }
                            Message::Close(_) => return Ok(()),
                            _ => {}
                        }
                    }
                    Some(Err(e)) => {
                        return Err(SignalError::Transient(format!("控制通道读取失败: {e}")));
                    }
                    None => return Err(SignalError::Transient("控制通道已关闭".to_string())),
                }
            }
            _ = watchdog.tick() => {
                if last_recv.elapsed() > silence {
                    return Err(SignalError::Transient(format!(
                        "控制通道超过 {silence:?} 未收到服务端帧（连接疑似半开）"
                    )));
                }
            }
        }
    }
}

/// 服务端推送处理：ping → pong；session.request → 放行对端身份并自动接受
/// （allow_remote 由服务端在登记时裁决）；session.cancelled → 收回放行；未知 type 忽略
async fn handle_message(
    ws: &mut (impl SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin),
    ctx: &ChannelContext<'_>,
    accepted_peers: &mut std::collections::HashMap<String, iroh::EndpointId>,
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
                let requested_write =
                    value.get("role").and_then(|r| r.as_str()) == Some("importer");
                // 时间线日志：与查看方日志按 session_id 对齐
                tracing::info!(
                    "收到会话请求 {session_id}（access={}）",
                    value
                        .get("role")
                        .and_then(|a| a.as_str())
                        .unwrap_or_default()
                );
                // 本地执法（权限以本机 key 映射为准，server 声称的 access 只是参考）：
                // - 查看者会话（携带 key 验证子）：key 必须存在且权限覆盖请求；
                //   写会话另需本机 writable 总开关
                // - app 间会话（无验证子）：不开放写（写只对 key 持有者开放）
                let credential = value.get("credential").and_then(|c| c.as_str());
                let writable = match credential {
                    Some(hash) => match ctx.keys.resolve(hash) {
                        Some(entry) => {
                            if requested_write
                                && !(entry.role == Role::Importer
                                    && ctx.remote.current().allow_import)
                            {
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
                        // app 间会话（同租户设备互信）：写权限只看本机「允许广域网保存素材」总开关。
                        // 访问密钥是给第三方、可逐把吊销的访问用的，自己的设备走设备身份，两者不混用；
                        // 写端点仍受 WAN 通道的 Import 上限限制（仅导入与新建文件夹）
                        if requested_write {
                            if !ctx.remote.current().allow_import {
                                tracing::info!("拒绝写会话 {session_id}：本机未开启广域网写入");
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
                            tracing::info!(
                                "放行同租户设备写会话 {session_id}（来源 {}）",
                                value["from"]["device_id"].as_str().unwrap_or_default()
                            );
                        }
                        requested_write
                    }
                };

                // 对端身份（endpoint_id）随 session.request 下发（服务端持有双方设备记录），
                // 先放行（B 在 ready 后可能立即连接）再回 accept
                let endpoint_id = value["from"]["endpoint_id"]
                    .as_str()
                    .and_then(|s| s.parse::<iroh::EndpointId>().ok());
                match endpoint_id {
                    Some(endpoint_id) => {
                        ctx.data_plane.allow_session(
                            session_id,
                            endpoint_id,
                            if writable {
                                Role::Importer
                            } else {
                                Role::Viewer
                            },
                        );
                        accepted_peers.insert(session_id.to_string(), endpoint_id);
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
                        // 只读会话身份缺失不阻断：接受后对端连不上会自行超时
                        tracing::warn!("会话对端身份缺失（接受但不放行）");
                    }
                }
                if let Err(e) =
                    accept_session(ctx.http, ctx.base, ctx.key, session_id, ctx.device_id).await
                {
                    ctx.data_plane.disallow_session(session_id);
                    accepted_peers.remove(session_id);
                    return Err(e);
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

/// 当前库显示名：库内配置的 name，缺失回退目录名；远程查看模式（无本地库）恒为「远程查看」。
/// 注册与心跳共用，设备身份跨库共享后由心跳保持服务端设备表的库名最新
fn library_name(library: &Option<LibraryContext>) -> String {
    match library {
        Some(ctx) => ctx.config.current().name.clone().unwrap_or_else(|| {
            std::path::Path::new(&ctx.paths.root)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "素材库".to_string())
        }),
        None => "远程查看".to_string(),
    }
}

/// 心跳回执
enum HeartbeatReply {
    /// 心跳成功；内容为服务端下发的中继配置（url + token；None = 该部署不提供中继）
    Ok(Option<(String, String)>),
    /// 被限速跳过（本跳无下发，调用方保持现状）
    Skipped,
}

async fn send_heartbeat(ctx: &ChannelContext<'_>) -> Result<HeartbeatReply> {
    // 寻址：iroh endpoint 自报的直连地址集（映射/外部地址由 iroh 自己维护刷新）。
    // public_ip 不自报——以服务端 public_ip_seen 为准
    let body = json!({
        "device_id": ctx.device_id,
        // 设备身份应用级：切换素材库后靠心跳把服务端设备表的库名收敛到当前库
        "library_name": library_name(ctx.library),
        // 远程查看模式无本地库可供写入，不上报写权限
        "allow_import": ctx.library.is_some() && ctx.remote.current().allow_import,
        "addrs": ctx.data_plane.direct_addrs(),
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
        let body: Value = resp.json().await.unwrap_or_default();
        let relay = body["data"]["relay"]["url"]
            .as_str()
            .map(str::to_string)
            .zip(body["data"]["relay"]["token"].as_str().map(str::to_string));
        Ok(HeartbeatReply::Ok(relay))
    } else if resp.status() == reqwest::StatusCode::NOT_FOUND {
        Err(SignalError::Unregistered)
    } else if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        tracing::debug!("心跳被服务端限速（10s 最小间隔），跳过本跳");
        Ok(HeartbeatReply::Skipped)
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
    library: &Option<LibraryContext>,
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
        "endpoint_id": identity.endpoint_id,
        "library_name": library_name(library),
        // 远程查看模式无本地库：可被发现但不接受会话（没有素材可服务）
        "allow_remote": library.is_some(),
        "allow_import": library.is_some() && remote.current().allow_import,
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

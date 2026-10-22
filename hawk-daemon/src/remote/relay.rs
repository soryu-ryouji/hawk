//! 中继传输（P2）：QUIC over 服务端中继 WS（quinn AsyncUdpSocket 实现）。
//! 一帧 = 一个 QUIC 数据报；端到端加密与指纹钉扎语义同直连（服务端只见密文）。

use crate::remote::identity::Identity;
use crate::remote::tls;
use futures_util::{SinkExt, StreamExt};
use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use std::collections::VecDeque;
use std::fmt::{Debug, Formatter};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// 中继伪地址：无真实五元组，仅作 QUIC 连接匹配标签（两端同值即可）
pub const RELAY_ADDR: SocketAddr = SocketAddr::V4(std::net::SocketAddrV4::new(
    Ipv4Addr::new(127, 0, 0, 1),
    4433,
));

pub type RelayWs =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// 中继 WS 请求（role=a 目标设备 / role=b 发起方，协议「中继」一节）。
/// service-key 走 Authorization 头而非 URL 查询串（反代 access log 会记录完整 URL）
pub fn relay_ws_request(
    base: &str,
    key: &str,
    device_id: &str,
    session_id: &str,
    role: &str,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, String> {
    let mut url = reqwest::Url::parse(base).map_err(|e| format!("remote url 解析失败: {e}"))?;
    url.set_path("/api/v1/relay");
    url.query_pairs_mut()
        .append_pair("device_id", device_id)
        .append_pair("session_id", session_id)
        .append_pair("role", role);
    url.set_scheme(if base.starts_with("https://") {
        "wss"
    } else {
        "ws"
    })
    .map_err(|_| "remote url scheme 非法".to_string())?;
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|e| format!("中继请求构建失败: {e}"))?;
    request.headers_mut().insert(
        tokio_tungstenite::tungstenite::http::header::AUTHORIZATION,
        format!("Bearer {key}")
            .parse()
            .map_err(|_| "service-key 含非法字符".to_string())?,
    );
    Ok(request)
}

use crate::remote::data::runtime;

/// 中继隧道 QUIC 握手超时：15s。中继比直连多一串串行链路（accept RTT → 对端经反代
/// 连 relay WS → 配对 → WS 上的 QUIC 往返），直连候选的 5s 在跨网中继下过紧
pub const RELAY_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// B 侧：经中继连接 A 侧（钉扎指纹）。返回 (endpoint, connection, WS 驱动句柄)；
/// 驱动句柄须随会话持有，中止即关闭中继通道
pub async fn connect_over_ws(
    identity: &Identity,
    ws: RelayWs,
    expected: [u8; 32],
) -> Result<
    (
        quinn::Endpoint,
        quinn::Connection,
        tokio::task::JoinHandle<()>,
    ),
    String,
> {
    let (socket, driver) = WsDatagramSocket::new(ws);
    let mut endpoint = quinn::Endpoint::new_with_abstract_socket(
        quinn::EndpointConfig::default(),
        None,
        Arc::new(socket),
        runtime()?,
    )
    .map_err(|e| format!("中继端点创建失败: {e}"))?;
    endpoint.set_default_client_config(tls::client_config(identity, expected)?);
    let connecting = endpoint
        .connect(RELAY_ADDR, "hawk-remote")
        .map_err(|e| format!("中继隧道发起失败: {e}"))?;
    let conn = tokio::time::timeout(RELAY_CONNECT_TIMEOUT, connecting)
        .await
        .map_err(|_| "中继隧道握手超时".to_string())?
        .map_err(|e| format!("中继隧道握手失败: {e}"))?;
    Ok((endpoint, conn, driver))
}

/// A 侧：经中继接受 B 侧连接（与直连同一 accept 循环，QUIC 端点建在中继字节流上）
pub async fn serve_over_ws(
    identity: &Identity,
    plane: &Arc<crate::remote::data::DataPlane>,
    ws: RelayWs,
    router: axum::Router,
) -> Result<(), String> {
    let (socket, _driver) = WsDatagramSocket::new(ws);
    let server_config = tls::server_config(identity, plane.allowlist.clone())?;
    let endpoint = quinn::Endpoint::new_with_abstract_socket(
        quinn::EndpointConfig::default(),
        Some(server_config),
        Arc::new(socket),
        runtime()?,
    )
    .map_err(|e| format!("中继端点创建失败: {e}"))?;
    plane.accept_over(endpoint, router).await;
    Ok(())
}

// ---------- AsyncUdpSocket over 中继 WS ----------

struct State {
    inbound: Mutex<VecDeque<Vec<u8>>>,
    recv_waker: Mutex<Option<Waker>>,
    outbound: mpsc::UnboundedSender<Vec<u8>>,
    closed: AtomicBool,
}

/// quinn AsyncUdpSocket over 中继 WS（每帧一个数据报；地址为伪标签）
pub struct WsDatagramSocket {
    state: Arc<State>,
}

impl WsDatagramSocket {
    /// 创建 socket 并拉起 WS 驱动任务（驱动持有 WS 读写两侧；任务结束即通道关闭）
    pub fn new(ws: RelayWs) -> (WsDatagramSocket, tokio::task::JoinHandle<()>) {
        let (out_tx, out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let state = Arc::new(State {
            inbound: Mutex::new(VecDeque::new()),
            recv_waker: Mutex::new(None),
            outbound: out_tx,
            closed: AtomicBool::new(false),
        });
        let driver = tokio::spawn(drive_ws(ws, state.clone(), out_rx));
        (WsDatagramSocket { state }, driver)
    }
}

impl Debug for WsDatagramSocket {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WsDatagramSocket").finish_non_exhaustive()
    }
}

/// WS 驱动：出站通道 → 二进制帧发送；入站帧 → 入队并唤醒 QUIC 接收端
async fn drive_ws(ws: RelayWs, state: Arc<State>, mut out_rx: mpsc::UnboundedReceiver<Vec<u8>>) {
    let (mut sink, mut stream) = ws.split();
    loop {
        tokio::select! {
            data = out_rx.recv() => {
                let Some(data) = data else { break };
                if sink.send(Message::Binary(data.into())).await.is_err() {
                    break;
                }
            }
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        state.inbound.lock().unwrap().push_back(data.to_vec());
                        if let Some(waker) = state.recv_waker.lock().unwrap().take() {
                            waker.wake();
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
    state.closed.store(true, Ordering::Relaxed);
    if let Some(waker) = state.recv_waker.lock().unwrap().take() {
        waker.wake();
    }
}

impl AsyncUdpSocket for WsDatagramSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(ReadyPoller)
    }

    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        if self.state.closed.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "中继通道已关闭"));
        }
        // unbounded 发送：不会 WouldBlock，poller 恒 Ready
        self.state
            .outbound
            .send(transmit.contents.to_vec())
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "中继通道已关闭"))
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [io::IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        if let Some(data) = self.state.inbound.lock().unwrap().pop_front() {
            let len = data.len().min(bufs[0].len());
            bufs[0][..len].copy_from_slice(&data[..len]);
            meta[0] = RecvMeta {
                addr: RELAY_ADDR,
                len,
                stride: len,
                ecn: None,
                dst_ip: None,
            };
            return Poll::Ready(Ok(1));
        }
        if self.state.closed.load(Ordering::Relaxed) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "中继通道已关闭",
            )));
        }
        *self.state.recv_waker.lock().unwrap() = Some(cx.waker().clone());
        // 入队与注册 waker 的竞态兜底：注册后再查一次
        if !self.state.inbound.lock().unwrap().is_empty() {
            cx.waker().wake_by_ref();
        }
        Poll::Pending
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(RELAY_ADDR)
    }

    fn may_fragment(&self) -> bool {
        false
    }
}

/// 出站无界，写就绪恒成立
#[derive(Debug)]
struct ReadyPoller;

impl UdpPoller for ReadyPoller {
    fn poll_writable(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

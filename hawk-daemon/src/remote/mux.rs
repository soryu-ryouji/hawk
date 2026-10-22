//! 统一传输 socket（P4 路径迁移）：一个真实 UDP socket + 零或多条中继 WS 路径合并为
//! 单一 quinn endpoint，对 QUIC 呈现稳定地址——路径切换对连接不可见。
//! 对标 iroh magicsock 的最小自研等价，借助 quinn 原生能力减负：
//! - 握手不在 mux 层多路径：quinn 握手钉死在首个 Initial 的地址（is_handshaking 期丢弃
//!   异源包），候选竞速在握手层并行（每候选独立 mux endpoint，首个胜者赢，见 session.rs）
//! - B 侧（client 角色）：quinn 客户端丢弃未知来源地址的包，因此全部入站归一到固定伪地址
//!   MUX_PEER；出站按路径活性路由（UDP 静默超阈值且中继在场 → 中继，恢复即自动回切）
//! - A 侧（server 角色）：UDP 入站保持真实地址、中继通道按分配伪地址入站；B 换路径后
//!   由 quinn 服务端被动迁移（migration 默认开启）自动跟随，A 侧不做任何主动决策
//!
//! mux 自身不产任何控制帧：静默观察只是读入站时间戳，没有额外的探测协议。
//! 单路径发送错误一律吞掉（返回 Ok）：多路径 socket 的一条路径失效不应杀死连接，
//! 终局由 QUIC 自身的重传/空闲超时负责（quinn 对 try_send 返回 Err 的处理是关闭连接）。

use futures_util::{SinkExt, StreamExt};
use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use std::collections::{HashMap, VecDeque};
use std::fmt::{Debug, Formatter};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// B 侧对端伪地址（固定）：入站归一与 connect 目标都用它，路径切换对 QUIC 不可见
pub const MUX_PEER: SocketAddr = SocketAddr::V4(std::net::SocketAddrV4::new(
    Ipv4Addr::new(127, 0, 3, 1),
    4433,
));

/// UDP 静默降级阈值：保活 10s 的活性观察窗口（路径死亡的典型信号是一轮保活无应答），
/// 远小于 idle 超时 60s，为「检测（12s）→ 附加中继（~2s）→ quinn 路径验证（~1s）」留足余量
pub const MUX_SILENCE: Duration = Duration::from_secs(12);

/// 静默阈值：测试可经 HAWK_REMOTE_MUX_SILENCE_MS 覆盖（同 heartbeat_interval 的提速约定）
pub fn silence_threshold() -> Duration {
    std::env::var("HAWK_REMOTE_MUX_SILENCE_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(MUX_SILENCE)
}

struct MuxState {
    /// 入站队列：addr 已是呈现给 quinn 的地址（A 侧 UDP=真实地址、中继=通道伪地址；
    /// B 侧恒为 MUX_PEER）
    inbound: Mutex<VecDeque<(SocketAddr, Vec<u8>)>>,
    recv_waker: Mutex<Option<Waker>>,
    /// UDP 出站句柄（endpoint socket 的克隆；读归 mux 入站泵任务独占）
    udp: Arc<std::net::UdpSocket>,
    /// B 侧 UDP 对端（握手竞速胜出的候选地址）；None = 纯中继路径形态；A 侧恒 None
    udp_peer: Option<SocketAddr>,
    /// 最近一次 UDP 入站时间（B 侧静默降级的判据；初值为创建时间，新 mux 不算静默）
    last_udp_recv: Mutex<Instant>,
    /// 中继路径：伪地址 → WS 发送端（B 侧至多一条、键为 MUX_PEER；A 侧每通道一条）
    relay_paths: Mutex<HashMap<SocketAddr, mpsc::UnboundedSender<Vec<u8>>>>,
    relay_seq: AtomicU32,
    closed: AtomicBool,
    /// B 侧归一目标（MUX_PEER）；A 侧 None
    normalize: Option<SocketAddr>,
    /// 测试挂钩：屏蔽 UDP 入站（模拟路径死亡，触发静默降级）
    block_udp: AtomicBool,
}

/// 统一传输 socket。A 侧经 `new_server`、B 侧经 `new_client` 创建；
/// quinn 经 `Endpoint::new_with_abstract_socket` 使用
pub struct MuxSocket {
    state: Arc<MuxState>,
    /// UDP 入站泵（shutdown 时中止）
    pump_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl MuxSocket {
    /// A 侧：多 peer 服务端形态（UDP 真实地址 + 中继通道伪地址）
    pub fn new_server(socket: std::net::UdpSocket) -> Arc<MuxSocket> {
        Self::new(socket, None, None)
    }

    /// B 侧：单对端客户端形态（入站全部归一到 MUX_PEER；udp_peer 为竞速胜出的候选地址，
    /// None = 纯中继路径形态）
    pub fn new_client(socket: std::net::UdpSocket, udp_peer: Option<SocketAddr>) -> Arc<MuxSocket> {
        Self::new(socket, udp_peer, Some(MUX_PEER))
    }

    fn new(
        socket: std::net::UdpSocket,
        udp_peer: Option<SocketAddr>,
        normalize: Option<SocketAddr>,
    ) -> Arc<MuxSocket> {
        // 入站泵用 tokio socket（原 socket）；出站用克隆句柄（与 quinn endpoint 同端口）
        let send = socket.try_clone().expect("UDP socket 克隆失败").into();
        let recv = tokio::net::UdpSocket::from_std(socket).expect("UDP socket 转 tokio 失败");
        let mux = Arc::new(MuxSocket {
            state: Arc::new(MuxState {
                inbound: Mutex::new(VecDeque::new()),
                recv_waker: Mutex::new(None),
                udp: send,
                udp_peer,
                last_udp_recv: Mutex::new(Instant::now()),
                relay_paths: Mutex::new(HashMap::new()),
                relay_seq: AtomicU32::new(0),
                closed: AtomicBool::new(false),
                normalize,
                block_udp: AtomicBool::new(false),
            }),
            pump_task: Mutex::new(None),
        });
        let task = tokio::spawn(udp_pump(recv, mux.state.clone()));
        *mux.pump_task.lock().unwrap() = Some(task);
        mux
    }

    /// 附加中继路径（P4）：WS 驱动任务把入站数据报喂进 mux，出站经 mpsc 送达 WS。
    /// 返回本侧分配的通道伪地址（诊断用）
    pub fn add_relay_path(self: &Arc<Self>, ws: crate::remote::relay::RelayWs) -> SocketAddr {
        // B 侧键恒为 MUX_PEER（同一对端只有一条中继路径）；A 侧按通道序号分配
        let pseudo = match self.state.normalize {
            Some(peer) => peer,
            None => {
                let n = self.state.relay_seq.fetch_add(1, Ordering::Relaxed) + 1;
                SocketAddr::from((Ipv4Addr::new(127, 0, 2, 1), n as u16))
            }
        };
        let (tx, rx) = mpsc::unbounded_channel::<Vec<u8>>();
        self.state.relay_paths.lock().unwrap().insert(pseudo, tx);
        tokio::spawn(drive_relay_path(ws, self.state.clone(), pseudo, rx));
        pseudo
    }

    /// B 侧：UDP 路径静默时长（会话层据此判定降级时机）
    pub fn udp_silence(&self) -> Duration {
        self.state.last_udp_recv.lock().unwrap().elapsed()
    }

    /// B 侧：中继路径是否已附加
    pub fn relay_attached(&self) -> bool {
        self.state
            .relay_paths
            .lock()
            .unwrap()
            .contains_key(&MUX_PEER)
    }

    /// UDP socket 的本机地址（DataPlane::local_addr 透传）
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.state.udp.local_addr()
    }

    /// 关停：入站泵退出、中继路径清空（endpoint close 后 socket 随 drop 释放）
    pub fn close(&self) {
        self.state.closed.store(true, Ordering::Relaxed);
        if let Some(task) = self.pump_task.lock().unwrap().take() {
            task.abort();
        }
        self.state.relay_paths.lock().unwrap().clear();
        if let Some(waker) = self.state.recv_waker.lock().unwrap().take() {
            waker.wake();
        }
    }

    #[doc(hidden)]
    pub fn debug_block_udp(&self, block: bool) {
        self.state.block_udp.store(block, Ordering::Relaxed);
    }
}

impl MuxState {
    fn push_inbound(&self, addr: SocketAddr, data: Vec<u8>) {
        self.inbound.lock().unwrap().push_back((addr, data));
        if let Some(waker) = self.recv_waker.lock().unwrap().take() {
            waker.wake();
        }
    }
}

/// UDP 入站泵：独占读 endpoint socket（出站经克隆句柄，读只有这里）
async fn udp_pump(socket: tokio::net::UdpSocket, state: Arc<MuxState>) {
    let mut buf = vec![0u8; 65536];
    loop {
        let Ok((len, from)) = socket.recv_from(&mut buf).await else {
            return; // socket 关闭即退出
        };
        if state.closed.load(Ordering::Relaxed) {
            return;
        }
        if state.block_udp.load(Ordering::Relaxed) {
            continue; // 测试挂钩：模拟路径死亡
        }
        *state.last_udp_recv.lock().unwrap() = Instant::now();
        let addr = state.normalize.unwrap_or(from);
        state.push_inbound(addr, buf[..len].to_vec());
    }
}

/// 中继路径驱动：出站 mpsc → WS 二进制帧；入站帧 → mux 入站队列。
/// WS 关闭即摘除路径（对端重连经 relay-path 重新附加）
async fn drive_relay_path(
    ws: crate::remote::relay::RelayWs,
    state: Arc<MuxState>,
    pseudo: SocketAddr,
    mut rx: mpsc::UnboundedReceiver<Vec<u8>>,
) {
    let (mut sink, mut stream) = ws.split();
    loop {
        tokio::select! {
            data = rx.recv() => {
                let Some(data) = data else { break };
                if sink
                    .send(tokio_tungstenite::tungstenite::Message::Binary(data.into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            msg = stream.next() => {
                match msg {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(data))) => {
                        state.push_inbound(pseudo, data.to_vec());
                    }
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_)))
                    | None
                    | Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
    state.relay_paths.lock().unwrap().remove(&pseudo);
    tracing::info!("中继路径 {pseudo} 已摘除");
}

impl Debug for MuxSocket {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MuxSocket")
            .field("client", &self.state.normalize.is_some())
            .finish()
    }
}

/// 出站无界（UDP 发送缓冲 + WS mpsc），写就绪恒成立
#[derive(Debug)]
struct ReadyPoller;

impl UdpPoller for ReadyPoller {
    fn poll_writable(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncUdpSocket for MuxSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(ReadyPoller)
    }

    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        let state = &self.state;
        if state.closed.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "mux 已关闭"));
        }
        match state.normalize {
            Some(_) => state.send_client(transmit),
            None => state.send_server(transmit),
        }
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [io::IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        loop {
            if let Some((addr, data)) = self.state.inbound.lock().unwrap().pop_front() {
                let len = data.len().min(bufs[0].len());
                bufs[0][..len].copy_from_slice(&data[..len]);
                meta[0] = RecvMeta {
                    addr,
                    len,
                    stride: len,
                    ecn: None,
                    dst_ip: None,
                };
                return Poll::Ready(Ok(1));
            }
            if self.state.closed.load(Ordering::Relaxed) {
                return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "mux 已关闭")));
            }
            *self.state.recv_waker.lock().unwrap() = Some(cx.waker().clone());
            // 入队与注册竞态兜底：注册后再查一次
            if !self.state.inbound.lock().unwrap().is_empty() {
                continue;
            }
            return Poll::Pending;
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.state.udp.local_addr()
    }

    fn may_fragment(&self) -> bool {
        false
    }
}

impl MuxState {
    /// B 侧出站路由：UDP 静默超阈值且中继在场 → 中继；其余 → UDP 对端（无 UDP 对端
    /// 即纯中继形态）。UDP 恢复入站即自动回切（udp_alive 随入站刷新），无需显式升级逻辑。
    /// 发送失败一律吞掉（多路径单点失效不杀连接，终局由 QUIC 重传/空闲超时负责）
    fn send_client(&self, transmit: &Transmit) -> io::Result<()> {
        let relay = self.relay_paths.lock().unwrap().get(&MUX_PEER).cloned();
        let udp_alive = self.last_udp_recv.lock().unwrap().elapsed() < silence_threshold();
        if let (Some(sender), false) = (&relay, udp_alive) {
            return send_ws(sender, transmit);
        }
        if let Some(addr) = self.udp_peer {
            self.send_udp(transmit, addr);
            return Ok(());
        }
        if let Some(sender) = relay {
            return send_ws(&sender, transmit);
        }
        Ok(())
    }

    /// A 侧出站路由：中继伪地址 → 对应通道；其余 → UDP 实发（v4-mapped 换算沿用）。
    /// 发送失败同样吞掉（某条路径失效不影响其余路径上的连接）
    fn send_server(&self, transmit: &Transmit) -> io::Result<()> {
        if let Some(sender) = self.relay_paths.lock().unwrap().get(&transmit.destination) {
            return send_ws(sender, transmit);
        }
        self.send_udp(transmit, transmit.destination);
        Ok(())
    }

    fn send_udp(&self, transmit: &Transmit, addr: SocketAddr) {
        let target = crate::remote::data::punch_target(&self.udp, addr);
        if let Err(e) = self.udp.send_to(transmit.contents, target) {
            tracing::debug!("mux UDP 发送失败（{target}）: {e}");
        }
    }
}

fn send_ws(sender: &mpsc::UnboundedSender<Vec<u8>>, transmit: &Transmit) -> io::Result<()> {
    sender
        .send(transmit.contents.to_vec())
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "中继路径已断开"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loopback_socket() -> std::net::UdpSocket {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_nonblocking(true).unwrap();
        socket
    }

    fn transmit(payload: &[u8]) -> Transmit<'_> {
        Transmit {
            destination: MUX_PEER,
            ecn: None,
            contents: payload,
            segment_size: None,
            src_ip: None,
        }
    }

    /// 收包辅助：从 mux 拉一条入站（测试驱动 quinn 之外的旁路）
    async fn recv_inbound(mux: &MuxSocket) -> (SocketAddr, Vec<u8>) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let item = mux.state.inbound.lock().unwrap().pop_front();
                if let Some(item) = item {
                    return item;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("入站超时")
    }

    #[tokio::test]
    async fn client_udp_routing_and_normalize() {
        let receiver = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let peer = SocketAddr::from(([127, 0, 0, 1], receiver.local_addr().unwrap().port()));
        let mux = MuxSocket::new_client(loopback_socket(), Some(peer));

        // UDP 路径在场且活跃：发送直达对端
        mux.state.send_client(&transmit(b"hello")).unwrap();
        let mut buf = [0u8; 64];
        assert_eq!(receiver.recv(&mut buf).unwrap(), 5);

        // 对端回包：入站归一到伪地址（quinn 客户端丢未知来源包的硬约束）
        receiver
            .send_to(b"resp", mux.local_addr().unwrap())
            .unwrap();
        let (addr, data) = recv_inbound(&mux).await;
        assert_eq!(addr, MUX_PEER);
        assert_eq!(data, b"resp");

        // 单路径发送失败被吞掉（地址族不符：v4 socket 发 v6 目标）——不杀连接
        let v6_peer = SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1u16], 9));
        let mux_v4 = MuxSocket::new_client(loopback_socket(), Some(v6_peer));
        assert!(mux_v4.state.send_client(&transmit(b"x")).is_ok());
        mux_v4.close();
        mux.close();
    }

    #[tokio::test]
    async fn silence_switch_prefers_relay_when_attached() {
        std::env::set_var("HAWK_REMOTE_MUX_SILENCE_MS", "200");
        // UDP 对端无人监听（永不入站 → 静默）
        let dead_peer = SocketAddr::from(([127, 0, 0, 1], 9));
        let mux = MuxSocket::new_client(loopback_socket(), Some(dead_peer));
        // 静默阈值内：仍走 UDP（无入站但时限未到）
        mux.state.send_client(&transmit(b"udp-first")).unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
        mux.state.relay_paths.lock().unwrap().insert(MUX_PEER, tx);
        tokio::time::sleep(Duration::from_millis(250)).await;
        // 超阈值：切中继
        mux.state.send_client(&transmit(b"via-relay")).unwrap();
        assert_eq!(rx.recv().await.unwrap(), b"via-relay");
        mux.close();
        std::env::remove_var("HAWK_REMOTE_MUX_SILENCE_MS");
    }

    #[tokio::test]
    async fn pure_relay_form_sends_over_relay() {
        // 无 UDP 对端（纯中继形态，relay 模式会话）：中继在场即走中继
        let mux = MuxSocket::new_client(loopback_socket(), None);
        let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
        mux.state.relay_paths.lock().unwrap().insert(MUX_PEER, tx);
        mux.state.send_client(&transmit(b"dgram")).unwrap();
        assert_eq!(rx.recv().await.unwrap(), b"dgram");
        mux.close();
    }
}

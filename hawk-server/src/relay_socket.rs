//! 进程内中继 datagram socket：网页版宿主的 QUIC 传输——不写真实 WS，直接读写
//! 本进程 relay_pairs 配对表（role=b），A 侧中继 WS 转发的数据报经配对表流入本 socket。

use crate::state::AppState;
use axum::extract::ws::Message;
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

/// 中继伪地址（与 daemon 侧同值的标签即可；无真实五元组）
pub const RELAY_ADDR: SocketAddr = SocketAddr::V4(std::net::SocketAddrV4::new(
    Ipv4Addr::new(127, 0, 0, 1),
    4433,
));

/// 写就绪 poller：对端在场即 Ready（quinn 首次发送前也会调，不能盲等 Notify）；
/// 仅对端缺失时挂起到配对表变更
struct NotifyPoller {
    app: Arc<AppState>,
    session_id: String,
    fut: Mutex<Option<Pin<Box<dyn std::future::Future<Output = ()> + Send>>>>,
}

impl Debug for NotifyPoller {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotifyPoller").finish_non_exhaustive()
    }
}

impl UdpPoller for NotifyPoller {
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // 对端在场（或配对已被移除由 try_send 报错）→ 立即可写
        let peer_ready = self
            .app
            .relay_pairs
            .read()
            .unwrap()
            .get(&self.session_id)
            .is_none_or(|pair| pair.peer("b").is_some());
        if peer_ready {
            return Poll::Ready(Ok(()));
        }
        let mut guard = self.fut.lock().unwrap();
        if guard.is_none() {
            let notify = self.app.relay_changed.clone();
            *guard = Some(Box::pin(async move {
                notify.notified().await;
            }));
        }
        let fut = guard.as_mut().unwrap();
        match fut.as_mut().poll(cx) {
            Poll::Ready(()) => {
                *guard = None;
                Poll::Ready(Ok(()))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

struct State {
    inbound: Mutex<VecDeque<Vec<u8>>>,
    recv_waker: Mutex<Option<Waker>>,
    closed: AtomicBool,
}

/// role=b 的进程内 QUIC socket：try_send 经配对表发给 role=a；poll_recv 收 role=a 的转发
pub struct RelaySocket {
    app: Arc<AppState>,
    session_id: String,
    state: Arc<State>,
}

impl RelaySocket {
    /// 注册进 relay_pairs（role=b），返回 socket；A 侧（role=a）连接后配对即成
    pub fn register(app: Arc<AppState>, session_id: &str) -> RelaySocket {
        let state = Arc::new(State {
            inbound: Mutex::new(VecDeque::new()),
            recv_waker: Mutex::new(None),
            closed: AtomicBool::new(false),
        });
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
        app.relay_pairs
            .write()
            .unwrap()
            .entry(session_id.to_string())
            .or_default()
            .register("b", tx);
        app.relay_changed.notify_waiters();
        // 配对表 → 入站队列（Binary 入队，Close 关闭）
        let st = state.clone();
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                match msg {
                    Message::Binary(data) => {
                        st.inbound.lock().unwrap().push_back(data.to_vec());
                        if let Some(waker) = st.recv_waker.lock().unwrap().take() {
                            waker.wake();
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            st.closed.store(true, Ordering::Relaxed);
            if let Some(waker) = st.recv_waker.lock().unwrap().take() {
                waker.wake();
            }
        });
        RelaySocket {
            app,
            session_id: session_id.to_string(),
            state,
        }
    }
}

impl Debug for RelaySocket {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelaySocket")
            .field("session_id", &self.session_id)
            .finish()
    }
}

impl AsyncUdpSocket for RelaySocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(NotifyPoller {
            app: self.app.clone(),
            session_id: self.session_id.clone(),
            fut: Mutex::new(None),
        })
    }

    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        let broken = || io::Error::new(io::ErrorKind::BrokenPipe, "中继配对已关闭");
        let pairs = self.app.relay_pairs.read().unwrap();
        let Some(pair) = pairs.get(&self.session_id) else {
            return Err(broken());
        };
        let Some(peer) = pair.peer("b") else {
            // A 侧未连接：WouldBlock，poller 等 relay_changed 唤醒重试
            return Err(io::Error::new(io::ErrorKind::WouldBlock, "对端未连接"));
        };
        peer.send(Message::Binary(transmit.contents.to_vec().into()))
            .map_err(|_| broken())
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [io::IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        loop {
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
            // 入队与注册竞态兜底：注册后再查一次
            if !self.state.inbound.lock().unwrap().is_empty() {
                continue;
            }
            return Poll::Pending;
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(RELAY_ADDR)
    }

    fn may_fragment(&self) -> bool {
        false
    }
}

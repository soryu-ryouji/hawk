//! 打洞探测口（P3）：极简 STUN 式 UDP 应答——校验会话 token，记录并回告 observed endpoint。
//! 防滥用三件套：响应不大于请求（请求定长 64B）、无效 token 静默丢弃不回包、按来源 IP 限速。
//! 协议细节见 docs/backend/remote-protocol.md「NAT 打洞」一节。

use crate::state::AppState;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 探测请求定长（magic 5B + token 16B + 零填充）：响应恒不大于此长度
pub const REQUEST_LEN: usize = 64;
const MAGIC_REQUEST: &[u8; 5] = b"HAWKP";
const MAGIC_RESPONSE: &[u8; 5] = b"HAWKR";
/// 否定应答（无效 token）：让运维能区分「探测口不可达」与「token 错误」；
/// 21B 远小于 64B 请求，无反射放大风险（形状非法的包连应答都没有，仍静默丢弃）
const MAGIC_NAK: &[u8; 5] = b"HAWK?";

/// 单来源 IP 限速：10s 窗口 30 包（正常客户端每次会话每侧 3 包）
const RATE_WINDOW: Duration = Duration::from_secs(10);
const RATE_MAX: u32 = 30;
/// 限速表兜底容量（防分布式的低速探测把表撑大）
const RATE_TABLE_MAX: usize = 10_000;

/// 探测口主循环（绑定失败由启动流程降级处理，不启动本任务）
pub async fn run(state: Arc<AppState>, socket: tokio::net::UdpSocket) {
    let mut limiter = RateLimit::default();
    let mut buf = [0u8; 2048];
    loop {
        let (len, from) = match socket.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("打洞探测口接收失败: {e}");
                continue;
            }
        };
        let Some(response) = handle(&state, &mut limiter, &buf[..len], from) else {
            continue;
        };
        let _ = socket.send_to(&response, from).await;
    }
}

/// 单包处理：形状非法/超限静默丢弃；无效 token 回否定应答（HAWK?）；
/// 命中则记录 observed endpoint 并回告（HAWKR）
fn handle(
    state: &AppState,
    limiter: &mut RateLimit,
    packet: &[u8],
    from: SocketAddr,
) -> Option<Vec<u8>> {
    if packet.len() < REQUEST_LEN || &packet[..5] != MAGIC_REQUEST {
        return None;
    }
    if !limiter.allow(from.ip()) {
        return None;
    }
    let token: [u8; 16] = packet[5..21].try_into().ok()?;
    let Some((session_id, role)) = state.record_probe(&token, from) else {
        tracing::debug!("打洞探测 token 无效或编排已结束（{from}）");
        let mut nak = Vec::with_capacity(21);
        nak.extend_from_slice(MAGIC_NAK);
        nak.extend_from_slice(&token);
        return Some(nak);
    };
    tracing::info!("打洞探测报到 {session_id}（{role} 侧）：{from}");
    let mut response = Vec::with_capacity(40);
    response.extend_from_slice(MAGIC_RESPONSE);
    response.extend_from_slice(&token);
    match from {
        SocketAddr::V4(addr) => {
            response.push(4);
            response.extend_from_slice(&addr.ip().octets());
        }
        SocketAddr::V6(addr) => {
            response.push(6);
            response.extend_from_slice(&addr.ip().octets());
        }
    }
    response.extend_from_slice(&from.port().to_be_bytes());
    Some(response)
}

/// 探测口限速（进程内存、惰性过期；表超容量直接清零重建）
#[derive(Default)]
struct RateLimit {
    hits: Mutex<HashMap<IpAddr, (Instant, u32)>>,
}

impl RateLimit {
    fn allow(&mut self, ip: IpAddr) -> bool {
        let mut map = self.hits.lock().unwrap();
        if map.len() > RATE_TABLE_MAX {
            map.clear();
        }
        let (window_start, count) = map.entry(ip).or_insert((Instant::now(), 0));
        if window_start.elapsed() > RATE_WINDOW {
            *window_start = Instant::now();
            *count = 0;
        }
        *count += 1;
        *count <= RATE_MAX
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;

    fn test_state(dir: &std::path::Path) -> Arc<AppState> {
        let keys = Arc::new(crate::keystore::KeyStore::open(dir).unwrap());
        Arc::new(AppState::new(keys, crate::store::Store::load(dir).unwrap()))
    }

    fn probe_packet(token: &[u8; 16]) -> [u8; REQUEST_LEN] {
        let mut packet = [0u8; REQUEST_LEN];
        packet[..5].copy_from_slice(MAGIC_REQUEST);
        packet[5..21].copy_from_slice(token);
        packet
    }

    #[test]
    fn rate_limit_blocks_flood() {
        let mut limiter = RateLimit::default();
        let ip: IpAddr = "1.2.3.4".parse().unwrap();
        for _ in 0..RATE_MAX {
            assert!(limiter.allow(ip));
        }
        assert!(!limiter.allow(ip));
        // 其他来源不受影响
        assert!(limiter.allow("5.6.7.8".parse().unwrap()));
    }

    #[test]
    fn malformed_packet_silently_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let mut limiter = RateLimit::default();
        let from: SocketAddr = "1.2.3.4:5000".parse().unwrap();
        // 太短 / magic 不对：连否定应答都没有（防扫描探测服务存在性之外的表面）
        assert!(handle(&state, &mut limiter, b"test", from).is_none());
        let mut bad_magic = [0u8; REQUEST_LEN];
        bad_magic[..5].copy_from_slice(b"XXXXX");
        assert!(handle(&state, &mut limiter, &bad_magic, from).is_none());
    }

    #[test]
    fn invalid_token_gets_nak() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let mut limiter = RateLimit::default();
        let from: SocketAddr = "1.2.3.4:5000".parse().unwrap();
        let token = [0u8; 16];
        let resp = handle(&state, &mut limiter, &probe_packet(&token), from).unwrap();
        assert_eq!(&resp[..5], MAGIC_NAK);
        assert_eq!(&resp[5..21], &token);
        assert!(resp.len() <= REQUEST_LEN, "响应不得大于请求（防放大）");
    }
}

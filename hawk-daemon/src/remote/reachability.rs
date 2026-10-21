//! 可达性探测：全局 IPv6（UDP connect 出接口，零流量）+ UPnP UDP 端口映射维护。
//! v4 公网地址不本地自报——以心跳响应的 public_ip_seen（服务端权威值）为准；
//! 这里产出的候选只有「UPnP 映射端口」与「全局 IPv6」。

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// UPnP 租约 30 分钟，每 25 分钟续期；失败 60s 后重试
const LEASE: Duration = Duration::from_secs(30 * 60);
const RENEW: Duration = Duration::from_secs(25 * 60);
const RETRY: Duration = Duration::from_secs(60);

#[derive(Clone, Default)]
pub struct UpnpReport {
    pub mapped_port: Option<u16>,
}

pub type SharedUpnpReport = Arc<RwLock<UpnpReport>>;

/// 出接口地址：UDP connect 到服务端地址（仅路由决策，不产生流量），local_addr 即出接口
fn egress_addr(server_base: &str) -> Option<IpAddr> {
    let url = reqwest::Url::parse(server_base).ok()?;
    let host = url.host_str()?;
    let port = url.port_or_known_default()?;
    let socket = UdpSocket::bind("[::]:0")
        .or_else(|_| UdpSocket::bind("0.0.0.0:0"))
        .ok()?;
    socket.connect((host, port)).ok()?;
    Some(socket.local_addr().ok()?.ip())
}

/// 全局 IPv6：无 NAT 可直接入站 → 直连候选（UPnP 无关，防火墙放行即可）。
/// 每次心跳调用一次（出接口随网络切换变化，探测零成本）
pub fn probe_ipv6(server_base: &str, quic_port: u16) -> Option<(String, u16)> {
    match egress_addr(server_base) {
        Some(IpAddr::V6(v6)) if is_global_v6(&v6) => Some((v6.to_string(), quic_port)),
        _ => None,
    }
}

fn is_global_v6(ip: &std::net::Ipv6Addr) -> bool {
    // Ipv6Addr::is_global 尚未稳定：排除回环/链路本地/唯一本地/未指定即全局
    !(ip.is_loopback() || ip.is_unicast_link_local() || ip.is_unique_local() || ip.is_unspecified())
}

/// UPnP 映射维护任务（igd 为阻塞 I/O，整体进 spawn_blocking 循环）：
/// 发现网关 → 映射 quic_port/UDP → 租约期内睡眠续期；失败清除上报并 60s 重试
pub async fn maintain_upnp(quic_port: u16, report: SharedUpnpReport) {
    loop {
        let outcome = tokio::task::spawn_blocking(move || upnp_round(quic_port)).await;
        match outcome {
            Ok(Ok(())) => {
                report.write().unwrap().mapped_port = Some(quic_port);
                tokio::time::sleep(RENEW).await;
            }
            Ok(Err(e)) => {
                tracing::debug!("UPnP 映射失败（{RETRY:?} 后重试）: {e}");
                report.write().unwrap().mapped_port = None;
                tokio::time::sleep(RETRY).await;
            }
            Err(e) => {
                tracing::error!("UPnP 任务异常: {e}");
                report.write().unwrap().mapped_port = None;
                tokio::time::sleep(RETRY).await;
            }
        }
    }
}

/// 单轮：发现网关 → 以网关地址探出本机局域网 v4 → 添加 UDP 映射（续期即重加）
fn upnp_round(quic_port: u16) -> Result<(), String> {
    let mut options = igd_next::SearchOptions::default();
    options.timeout = Some(Duration::from_secs(5));
    let gateway = igd_next::search_gateway(options).map_err(|e| format!("网关发现失败: {e}"))?;
    let local = UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect(gateway.addr)?;
            s.local_addr()
        })
        .map_err(|e| format!("本机局域网地址探测失败: {e}"))?;
    let local = match local {
        SocketAddr::V4(v4) => v4,
        SocketAddr::V6(_) => return Err("本机局域网地址非 IPv4".to_string()),
    };
    gateway
        .add_port(
            igd_next::PortMappingProtocol::UDP,
            quic_port,
            SocketAddr::V4(std::net::SocketAddrV4::new(*local.ip(), quic_port)),
            LEASE.as_secs() as u32,
            "hawk-remote",
        )
        .map_err(|e| format!("端口映射失败: {e}"))?;
    tracing::info!("UPnP 已映射 UDP {quic_port}（网关 {}）", gateway.addr);
    Ok(())
}

/// Windows 防火墙入站规则（首次启用 QUIC 监听时添加，幂等）：无规则时入站 UDP 会被静默丢弃，
/// UPnP 映射成功也连不进来。规则名带端口（多库错开端口时各有一条）
#[cfg(windows)]
pub async fn ensure_firewall_rule_windows(quic_port: u16) {
    let rule_name = format!("hawk-remote QUIC {quic_port}");
    let rule = rule_name.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let exists = std::process::Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "show",
                "rule",
                &format!("name={rule}"),
            ])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if exists {
            return true;
        }
        std::process::Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "add",
                "rule",
                &format!("name={rule}"),
                "dir=in",
                "action=allow",
                "protocol=UDP",
                &format!("localport={quic_port}"),
            ])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
    .await;
    match outcome {
        Ok(true) => tracing::info!("Windows 防火墙入站规则就绪: {rule_name}"),
        _ => tracing::warn!(
            "Windows 防火墙入站规则添加失败（QUIC 入站可能被拦截，可手动放行 UDP {quic_port}）"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_v6_classification() {
        assert!(is_global_v6(&"240e::1".parse().unwrap()));
        assert!(is_global_v6(&"2001:4860:4860::8888".parse().unwrap()));
        assert!(!is_global_v6(&"::1".parse().unwrap()));
        assert!(!is_global_v6(&"fe80::1".parse().unwrap()));
        assert!(!is_global_v6(&"fd00::1".parse().unwrap()));
    }
}

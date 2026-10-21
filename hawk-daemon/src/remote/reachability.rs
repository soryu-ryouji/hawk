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

/// 全局 IPv6 探测目标（Cloudflare 公共 DNS；UDP connect 只做路由决策，不产生流量）。
/// 不能探服务端地址：服务端 IPv4-only 时出接口是 IPv4，本机的全球 IPv6 永远探不到
const IPV6_PROBE_TARGET: std::net::Ipv6Addr =
    std::net::Ipv6Addr::new(0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111);

/// 全局 IPv6：无 NAT 可直接入站 → 直连候选（UPnP 无关，防火墙放行即可）。
/// 每次心跳调用一次（出接口随网络切换变化，探测零成本）
pub fn probe_ipv6(quic_port: u16) -> Option<(String, u16)> {
    let socket = UdpSocket::bind("[::]:0").ok()?;
    socket.connect((IPV6_PROBE_TARGET, 53)).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    match ip {
        IpAddr::V6(v6) if is_global_v6(&v6) => Some((v6.to_string(), quic_port)),
        _ => None,
    }
}

fn is_global_v6(ip: &std::net::Ipv6Addr) -> bool {
    // Ipv6Addr::is_global 尚未稳定：排除回环/链路本地/唯一本地/未指定即全局
    !(ip.is_loopback() || ip.is_unicast_link_local() || ip.is_unique_local() || ip.is_unspecified())
}

/// UPnP 映射维护任务（igd 为阻塞 I/O，整体进 spawn_blocking 循环）：
/// 发现网关 → 映射 UDP 端口（27373 被占时退到任意外部端口）→ 租约期内睡眠续期；
/// 失败清除上报并 60s 重试。上报值为实际分到的**外部端口**
pub async fn maintain_upnp(quic_port: u16, report: SharedUpnpReport) {
    loop {
        let outcome = tokio::task::spawn_blocking(move || upnp_round(quic_port)).await;
        match outcome {
            Ok(Ok(external_port)) => {
                report.write().unwrap().mapped_port = Some(external_port);
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

/// 单轮：发现网关 → 以网关地址探出本机局域网 v4 → 添加 UDP 映射（续期即重加）。
/// 返回实际分到的外部端口：期望端口被占用（同局域网多台 hawk、他设备映射）时
/// 退到任意外部端口——candidate 播报的是外部端口，必须与路由器实际分配一致
fn upnp_round(quic_port: u16) -> Result<u16, String> {
    let mut options = igd_next::SearchOptions::default();
    options.timeout = Some(Duration::from_secs(5));
    let gateway = igd_next::search_gateway(options).map_err(|e| format!("网关发现失败: {e}"))?;
    let local = UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect(gateway.addr)?;
            s.local_addr()
        })
        .map_err(|e| format!("本机局域网地址探测失败: {e}"))?;
    let local_v4 = match local {
        SocketAddr::V4(v4) => v4,
        SocketAddr::V6(_) => return Err("本机局域网地址非 IPv4".to_string()),
    };
    let local_addr = SocketAddr::V4(std::net::SocketAddrV4::new(*local_v4.ip(), quic_port));
    match gateway.add_port(
        igd_next::PortMappingProtocol::UDP,
        quic_port,
        local_addr,
        LEASE.as_secs() as u32,
        "hawk-remote",
    ) {
        Ok(()) => {
            tracing::info!("UPnP 已映射 UDP {quic_port}（网关 {}）", gateway.addr);
            Ok(quic_port)
        }
        Err(e) => {
            // 期望端口被占：让网关分配任意外部端口（内部端口不变）
            let external = gateway
                .add_any_port(
                    igd_next::PortMappingProtocol::UDP,
                    local_addr,
                    LEASE.as_secs() as u32,
                    "hawk-remote",
                )
                .map_err(|e2| format!("端口映射失败（{e}；任意端口亦失败 {e2}）"))?;
            tracing::info!(
                "UPnP 已映射 UDP {external} → {quic_port}（网关 {}，期望端口被占）",
                gateway.addr
            );
            Ok(external)
        }
    }
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

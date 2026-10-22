//! 可达性探测：全局 IPv6（UDP connect 出接口，零流量）+ UPnP/NAT-PMP UDP 端口映射维护。
//! v4 公网地址不本地自报——以心跳响应的 public_ip_seen（服务端权威值）为准；
//! 这里产出的候选只有「映射端口」与「全局 IPv6」。
//! 映射协议为 Syncthing 同款分层：UPnP 优先，失败回退 NAT-PMP（RFC 6886，Apple 系路由器
//! 只讲此协议）；PCP 不投（运营商侧协议，家用场景无覆盖）。

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// UPnP 租约 30 分钟，每 25 分钟续期；失败 60s 后重试
const LEASE: Duration = Duration::from_secs(30 * 60);
const RENEW: Duration = Duration::from_secs(25 * 60);
const RETRY: Duration = Duration::from_secs(60);

#[derive(Clone, Default)]
pub struct MappingReport {
    pub mapped_port: Option<u16>,
    /// 映射协议：upnp / pmp（无映射时 None，心跳上报 "none"）
    pub mapping_type: Option<&'static str>,
}

pub type SharedMappingReport = Arc<RwLock<MappingReport>>;

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

/// LAN IPv4 探测（UDP connect 出接口，零流量，同 probe_ipv6 原理）：
/// 同公网 IP 的会话双方很可能在同一 NAT 后——LAN 候选让同局域网设备直接内网直连，
/// 绕开 hairpin/UPnP/CGNAT 全部问题。仅私网地址上报（机器直连公网时直连候选已覆盖）
pub fn probe_lan_ipv4() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket
        .connect((std::net::Ipv4Addr::new(1, 1, 1, 1), 53))
        .ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(v4) if v4.is_private() => Some(v4.to_string()),
        _ => None,
    }
}

fn is_global_v6(ip: &std::net::Ipv6Addr) -> bool {
    // Ipv6Addr::is_global 尚未稳定：排除回环/链路本地/唯一本地/未指定即全局
    !(ip.is_loopback() || ip.is_unicast_link_local() || ip.is_unique_local() || ip.is_unspecified())
}

/// 端口映射维护任务（igd/NAT-PMP 均为阻塞 I/O，整体进 spawn_blocking 循环）：
/// 映射 UDP 端口（27373 被占时退到网关分配的任意外部端口）→ 租约期内睡眠续期；
/// 失败清除上报并 60s 重试。上报值为实际分到的**外部端口**
pub async fn maintain_mapping(quic_port: u16, report: SharedMappingReport) {
    loop {
        let outcome = tokio::task::spawn_blocking(move || mapping_round(quic_port)).await;
        match outcome {
            Ok(Ok((external_port, mapping_type))) => {
                // 块作用域圈住写 guard：不得跨 await（非 Send）
                {
                    let mut guard = report.write().unwrap();
                    guard.mapped_port = Some(external_port);
                    guard.mapping_type = Some(mapping_type);
                }
                tokio::time::sleep(RENEW).await;
            }
            Ok(Err(e)) => {
                tracing::debug!("端口映射失败（{RETRY:?} 后重试）: {e}");
                *report.write().unwrap() = MappingReport::default();
                tokio::time::sleep(RETRY).await;
            }
            Err(e) => {
                tracing::error!("端口映射任务异常: {e}");
                *report.write().unwrap() = MappingReport::default();
                tokio::time::sleep(RETRY).await;
            }
        }
    }
}

/// 单轮映射：UPnP 优先，失败回退 NAT-PMP（协议分层见模块头注释）
fn mapping_round(quic_port: u16) -> Result<(u16, &'static str), String> {
    match upnp_round(quic_port) {
        Ok(port) => Ok((port, "upnp")),
        Err(upnp_err) => pmp_round(quic_port)
            .map(|port| (port, "pmp"))
            .map_err(|pmp_err| format!("UPnP（{upnp_err}）；NAT-PMP（{pmp_err}）")),
    }
}

// ---------- NAT-PMP（RFC 6886，极简客户端） ----------

/// NAT-PMP 网关端口
const PMP_PORT: u16 = 5351;
/// 单包收发超时（网关同局域网，正常 RTT <10ms）
const PMP_IO_TIMEOUT: Duration = Duration::from_millis(800);

/// UDP 映射请求（12B）：version=0, op=1(UDP), reserved=0, internal, suggested external, lifetime
fn pmp_mapping_request(internal: u16, external: u16, lifetime: u32) -> [u8; 12] {
    let mut request = [0u8; 12];
    request[1] = 1;
    request[4..6].copy_from_slice(&internal.to_be_bytes());
    request[6..8].copy_from_slice(&external.to_be_bytes());
    request[8..12].copy_from_slice(&lifetime.to_be_bytes());
    request
}

/// 映射响应（16B）：version=0, op=129, result(2), epoch(4), internal(2), assigned external(2), lifetime(4)。
/// 返回网关实际分配的外部端口（候选播报值必须与之口径一致，同 UPnP）
fn pmp_parse_mapping_response(buf: &[u8], internal: u16) -> Result<u16, String> {
    if buf.len() < 16 || buf[0] != 0 || buf[1] != 129 {
        return Err("响应形状非法".to_string());
    }
    let result = u16::from_be_bytes([buf[2], buf[3]]);
    if result != 0 {
        return Err(format!("网关拒绝（结果码 {result}）"));
    }
    let resp_internal = u16::from_be_bytes([buf[8], buf[9]]);
    if resp_internal != internal {
        return Err(format!("响应内部端口不符（{resp_internal}）"));
    }
    Ok(u16::from_be_bytes(buf[10..12].try_into().unwrap()))
}

/// NAT-PMP 映射一轮：查默认网关（路由表）→ UDP 映射请求（两次重发容忍局域网丢包）
fn pmp_round(quic_port: u16) -> Result<u16, String> {
    let gateway =
        default_net::get_default_gateway().map_err(|e| format!("默认网关查询失败: {e}"))?;
    let IpAddr::V4(gateway) = gateway.ip_addr else {
        return Err("默认网关非 IPv4".to_string());
    };
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("socket 绑定失败: {e}"))?;
    socket
        .set_read_timeout(Some(PMP_IO_TIMEOUT))
        .map_err(|e| format!("socket 超时设置失败: {e}"))?;
    let target = SocketAddr::from((gateway, PMP_PORT));
    let request = pmp_mapping_request(quic_port, quic_port, LEASE.as_secs() as u32);
    let mut last_err = String::new();
    for _ in 0..2 {
        if let Err(e) = socket.send_to(&request, target) {
            last_err = format!("发送失败: {e}");
            continue;
        }
        let mut buf = [0u8; 64];
        match socket.recv_from(&mut buf) {
            Ok((len, _)) => {
                let external = pmp_parse_mapping_response(&buf[..len], quic_port)?;
                tracing::info!("NAT-PMP 已映射 UDP {external} → {quic_port}（网关 {gateway}）");
                return Ok(external);
            }
            Err(e) => last_err = format!("接收失败: {e}"),
        }
    }
    Err(last_err)
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
    fn pmp_request_layout() {
        let req = pmp_mapping_request(27373, 27373, 1800);
        assert_eq!(req[0], 0); // version
        assert_eq!(req[1], 1); // op = UDP
        assert_eq!(&req[4..6], &27373u16.to_be_bytes());
        assert_eq!(&req[8..12], &1800u32.to_be_bytes());
    }

    #[test]
    fn pmp_response_parsing() {
        // 成功响应：op=129，结果码 0，内部端口回显，外部端口以网关分配为准
        let mut resp = [0u8; 16];
        resp[1] = 129;
        resp[8..10].copy_from_slice(&27373u16.to_be_bytes());
        resp[10..12].copy_from_slice(&40000u16.to_be_bytes());
        assert_eq!(pmp_parse_mapping_response(&resp, 27373).unwrap(), 40000);
        // 网关拒绝（结果码非 0）
        let mut denied = resp;
        denied[3] = 2;
        assert!(pmp_parse_mapping_response(&denied, 27373).is_err());
        // 内部端口回显不符（串台包）
        let mut mismatched = resp;
        mismatched[9] = 1;
        assert!(pmp_parse_mapping_response(&mismatched, 27373).is_err());
        // 形状非法
        assert!(pmp_parse_mapping_response(&resp[..8], 27373).is_err());
    }

    #[test]
    fn lan_ipv4_reports_private_or_nothing() {
        // 环境敏感（无默认路由的命名空间返回 None）：有值则必须是私网地址
        if let Some(ip) = probe_lan_ipv4() {
            let v4: std::net::Ipv4Addr = ip.parse().unwrap();
            assert!(v4.is_private(), "LAN 候选必须私网: {v4}");
        }
    }

    #[test]
    fn global_v6_classification() {
        assert!(is_global_v6(&"240e::1".parse().unwrap()));
        assert!(is_global_v6(&"2001:4860:4860::8888".parse().unwrap()));
        assert!(!is_global_v6(&"::1".parse().unwrap()));
        assert!(!is_global_v6(&"fe80::1".parse().unwrap()));
        assert!(!is_global_v6(&"fd00::1".parse().unwrap()));
    }
}

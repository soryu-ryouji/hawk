//! 中继通道（P2/P4）：与服务端中继 WS 的建立辅助。数据报传输已由 mux 接管
//! （中继是 mux 的一条路径，见 mux.rs）；这里只剩请求构建与带超时的连接。

use tokio_tungstenite::tungstenite::client::IntoClientRequest;

pub type RelayWs =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

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

/// 带超时连接中继 WS：connect_async 无内置超时（反代对 Upgrade 处理不当时会静默挂起，
/// 对端 15s 就超时放弃了），必须自己掐表
pub async fn connect_relay_ws(
    base: &str,
    key: &str,
    device_id: &str,
    session_id: &str,
    role: &str,
) -> Result<RelayWs, String> {
    let request = relay_ws_request(base, key, device_id, session_id, role)?;
    let (ws, _) = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .map_err(|_| "中继通道连接超时（10s，检查反代的 WebSocket Upgrade 配置）".to_string())?
    .map_err(|e| format!("中继通道连接失败: {e}"))?;
    Ok(ws)
}

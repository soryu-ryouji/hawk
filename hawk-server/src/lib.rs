//! hawk-server：hawk 远程访问服务端（单用户）。协议契约见 docs/backend/remote-protocol.md。

pub mod api;
pub mod auth;
pub mod settings;
pub mod state;
pub mod store;

use settings::Settings;
use std::net::SocketAddr;
use std::sync::Arc;

/// 启动 HTTP 服务。本进程监听明文 HTTP，TLS/WSS 由前置反代（Caddy 等）终结。
pub async fn run(settings: Settings) -> std::io::Result<()> {
    let store = store::Store::load(&settings.data_dir)?;
    let state = Arc::new(state::AppState::new(settings.access_key, store));
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", settings.port)).await?;
    tracing::info!(port = settings.port, "hawk-server listening");
    axum::serve(
        listener,
        api::router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
}

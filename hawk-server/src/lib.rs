//! hawk-server：hawk 远程访问服务端（单用户）。协议契约见 docs/backend/remote-protocol.md。

pub mod api;
pub mod auth;
pub mod identity;
pub mod relay_socket;
pub mod settings;
pub mod state;
pub mod store;
pub mod tls_client;
pub mod viewer;

use settings::Settings;
use std::net::SocketAddr;
use std::sync::Arc;

/// 启动 HTTP 服务。本进程监听明文 HTTP，TLS/WSS 由前置反代（Caddy 等）终结。
pub async fn run(settings: Settings) -> std::io::Result<()> {
    let store = store::Store::load(&settings.data_dir)?;
    let state = Arc::new(state::AppState::new(settings.access_key, store));

    // 网页版宿主：服务端设备身份 + 「网页版」设备行 upsert + 查看会话管理
    let mut identity =
        identity::Identity::load_or_create(&settings.data_dir).map_err(std::io::Error::other)?;
    let device_id =
        viewer::ensure_viewer_device(&state, &identity).map_err(std::io::Error::other)?;
    if identity.device_id.is_none() {
        identity.device_id = Some(device_id);
        identity
            .persist(&settings.data_dir)
            .map_err(std::io::Error::other)?;
    }
    let viewer = viewer::ViewerManager::new(state.clone(), identity, &settings.data_dir);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", settings.port)).await?;
    tracing::info!(port = settings.port, "hawk-server listening");
    axum::serve(
        listener,
        api::router(state, viewer, settings.web_dist)
            .into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
}

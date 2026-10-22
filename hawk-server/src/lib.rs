//! hawk-server：hawk 远程访问服务端（多租户：service-key 准入 + hawk-app key 映射）。
//! 协议契约见 docs/backend/remote-protocol.md。

pub mod admin;
pub mod api;
pub mod auth;
pub mod identity;
pub mod keystore;
pub mod probe;
pub mod ratelimit;
pub mod relay_socket;
pub mod settings;
pub mod state;
pub mod store;
pub mod tls_client;
pub mod viewer;

use settings::Cli;
use std::net::SocketAddr;
use std::sync::Arc;

/// 启动 HTTP 服务。主端口监听明文 HTTP（TLS/WSS 由前置反代终结）；
/// 管理面另起环回监听口（127.0.0.1:admin_port，默认 9110），仅本机可达。
pub async fn run(settings: &Cli) -> std::io::Result<()> {
    let data_dir = settings.data_dir();
    let keys = Arc::new(keystore::KeyStore::open(&data_dir)?);
    let store = store::Store::load(&data_dir)?;
    let mut state = state::AppState::new(keys.clone(), store);

    // 打洞探测口（P3，UDP）：绑定失败只降级不阻断启动——打洞编排关闭，
    // 无直连候选的会话退回 unreachable（同编排出现之前的行为）
    let probe_socket = if settings.probe_port == 0 {
        None
    } else {
        match tokio::net::UdpSocket::bind(("0.0.0.0", settings.probe_port)).await {
            Ok(socket) => Some(socket),
            Err(e) => {
                tracing::error!(
                    port = settings.probe_port,
                    "打洞探测口绑定失败（打洞编排关闭）: {e}"
                );
                None
            }
        }
    };
    state.probe_port = probe_socket.as_ref().map(|_| settings.probe_port);
    let state = Arc::new(state);
    if let Some(socket) = probe_socket {
        tracing::info!(
            port = settings.probe_port,
            "hawk-server probe listening (UDP)"
        );
        tokio::spawn(probe::run(state.clone(), socket));
    }

    // 网页版宿主：服务端设备身份 + 「网页版」设备行 upsert + 查看会话管理
    let mut identity =
        identity::Identity::load_or_create(&data_dir).map_err(std::io::Error::other)?;
    let device_id =
        viewer::ensure_viewer_device(&state, &identity).map_err(std::io::Error::other)?;
    if identity.device_id.is_none() {
        identity.device_id = Some(device_id);
        identity.persist(&data_dir).map_err(std::io::Error::other)?;
    }
    let viewer = viewer::ViewerManager::new(state.clone(), identity, &data_dir);

    if settings.admin_port == settings.port {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "管理面端口不能与主端口相同（--admin-port 须为独立端口，仅绑环回）",
        ));
    }
    let admin_port = settings.admin_port;
    let admin_listener = tokio::net::TcpListener::bind(("127.0.0.1", admin_port)).await?;
    let admin_router = admin::router(keys);
    tokio::spawn(async move {
        tracing::info!(port = admin_port, "hawk-server admin listening (127.0.0.1)");
        if let Err(e) = axum::serve(admin_listener, admin_router).await {
            tracing::error!("管理面监听结束: {e}");
        }
    });

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", settings.port)).await?;
    tracing::info!(port = settings.port, "hawk-server listening");
    axum::serve(
        listener,
        api::router(state, viewer).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
}

/// 租户管理子命令：service-key 的创建/列出/吊销（同步执行，结果打印到 stdout）
pub fn run_tenant_command(
    action: &settings::TenantAction,
    data_dir: &std::path::Path,
) -> std::io::Result<()> {
    let keys = keystore::KeyStore::open(data_dir)?;
    match action {
        settings::TenantAction::Add { label } => {
            let (id, service_key) = keys.tenant_add(label)?;
            println!("租户已创建：{id}（{label}）");
            println!("service-key（只显示这一次，请立即保存）: {service_key}");
        }
        settings::TenantAction::List => {
            let tenants = keys.tenant_list();
            if tenants.is_empty() {
                println!("暂无租户（tenant add --label <名称> 创建）");
            }
            for t in tenants {
                println!("{}\t{}\t{}", t.id, t.label, t.created_at_ms);
            }
        }
        settings::TenantAction::Revoke { id } => {
            if keys.tenant_revoke(id)? {
                println!("租户 {id} 已吊销（service-key 与池内 hawk-app key 立即失效）");
            } else {
                eprintln!("租户不存在: {id}");
                std::process::exit(2);
            }
        }
    }
    Ok(())
}

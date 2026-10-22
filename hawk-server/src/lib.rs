//! hawk-server：hawk 远程访问服务端（多租户：service-key 准入 + hawk-app key 映射）。
//! 协议契约见 docs/backend/remote-protocol.md。

pub mod admin;
pub mod api;
pub mod auth;
pub mod identity;
pub mod keystore;
pub mod ratelimit;
pub mod settings;
pub mod state;
pub mod store;
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

    // 内嵌中继（可选组件）：--relay-url 提供公网入口才启用；准入为数据目录里的
    // 共享 token（随会话 connect 经控制面下发给池内成员，控制面已有鉴权 + TLS）
    let relay_url = settings.relay_url.clone().filter(|u| !u.trim().is_empty());
    let _relay_server = match &relay_url {
        Some(url) => {
            let token = load_or_create_relay_token(&data_dir)?;
            let mut config = iroh_relay::server::ServerConfig::default();
            // 中继监听环回（公网入口一律由反代终结，与主端口 9100 同模型）；
            // 内外端口分离：反代对外端口与 relay_port 无关，部署别撞车
            let mut relay_config = iroh_relay::server::RelayConfig::new(SocketAddr::from((
                [127, 0, 0, 1],
                settings.relay_port,
            )));
            relay_config.access = Arc::new(TokenAccess(token.clone()));
            config.relay = Some(relay_config);
            // 可选组件绑定失败只降级（本部署不提供中继），不阻断主服务
            let server = match iroh_relay::server::Server::spawn(config).await {
                Ok(server) => {
                    tracing::info!(
                        port = settings.relay_port,
                        url = url.as_str(),
                        "hawk-server embedded relay listening (127.0.0.1)"
                    );
                    state.relay_info = Some(serde_json::json!({ "url": url, "token": token }));
                    Some(server)
                }
                Err(e) => {
                    tracing::error!(
                        port = settings.relay_port,
                        "内嵌中继绑定失败（本部署不提供中继，退化为纯直连）: {e}"
                    );
                    None
                }
            };
            server
        }
        None => {
            tracing::info!("未配置 --relay-url：本部署不提供中继（纯直连形态）");
            None
        }
    };
    let state = Arc::new(state);

    // 网页版宿主：服务端设备身份 + 「网页版」设备行 upsert + 查看会话管理；
    // 宿主的长驻 iroh endpoint 在此创建（B 侧拨号方；中继配置随服务端部署形态）
    let mut identity =
        identity::Identity::load_or_create(&data_dir).map_err(std::io::Error::other)?;
    let endpoint = {
        let mut builder = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .alpns(vec![viewer::HAWK_ALPN.to_vec()])
            .secret_key(identity.secret_key().map_err(std::io::Error::other)?);
        builder = match &state.relay_info {
            Some(info) => {
                let url: iroh::RelayUrl = info["url"]
                    .as_str()
                    .and_then(|u| u.parse().ok())
                    .ok_or_else(|| std::io::Error::other("relay url 非法"))?;
                let map = iroh::RelayMap::from(url);
                let token = info["token"].as_str().unwrap_or_default();
                builder.relay_mode(iroh::RelayMode::Custom(map.with_auth_token(token)))
            }
            None => builder.relay_mode(iroh::RelayMode::Disabled),
        };
        builder.bind().await.map_err(std::io::Error::other)?
    };
    let device_id =
        viewer::ensure_viewer_device(&state, &identity).map_err(std::io::Error::other)?;
    if identity.device_id.is_none() {
        identity.device_id = Some(device_id);
        identity.persist(&data_dir).map_err(std::io::Error::other)?;
    }
    let viewer = viewer::ViewerManager::new(state.clone(), identity, endpoint, &data_dir);

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

/// 内嵌中继的共享准入 token：数据目录 relay_token 文件（0600，首启自生成）
fn load_or_create_relay_token(data_dir: &std::path::Path) -> std::io::Result<String> {
    let file = data_dir.join("relay_token");
    if let Ok(token) = std::fs::read_to_string(&file) {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return Ok(token);
        }
    }
    let token = identity::hex_encode(&rand::random::<[u8; 32]>());
    std::fs::write(&file, &token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
    }
    Ok(token)
}

/// 内嵌中继准入：共享 token（iroh-relay 的 AccessControl 实现点）
#[derive(Debug)]
struct TokenAccess(String);

impl iroh_relay::server::AccessControl for TokenAccess {
    async fn on_connect(
        &self,
        request: &iroh_relay::server::ClientRequest,
    ) -> iroh_relay::server::Access {
        match request.auth_token().as_deref() == Some(self.0.as_str()) {
            true => iroh_relay::server::Access::Allow,
            false => iroh_relay::server::Access::Deny { reason: None },
        }
    }
}

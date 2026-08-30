//! 组装与启动：参数解析、服务构建、中间件、启动顺序（先监听、索引后台构建）、就绪信号。
//! 与 C# Program.cs 的启动模型一致：Kestrel 先监听（此处为 axum serve 先拉起），
//! 随后装配索引流水线——内存索引由元数据缓存（SQLite 快路径/TOML 回退）注水，
//! 就绪不再等待全库扫描；停机期间的文件增删改由监听实时事件 + 后台对账扫描收敛。

mod api;
mod core;
mod settings;

use api::SharedState;
use crate::core::config::LibraryConfig;
use crate::core::events::EventBus;
use crate::core::index::ItemIndex;
use crate::core::index_db::IndexDb;
use crate::core::metadata_store::MetadataStore;
use crate::core::paths::LibraryPaths;
use crate::core::pipeline::IndexPipeline;
use crate::core::scanner::LibraryScanner;
use crate::settings::Settings;
use crate::core::startup::StartupState;
use crate::core::taxonomy::{CategoryRegistry, TagRegistry, TaxonomyMigrator};
use crate::core::thumbnail::ThumbnailService;
use crate::core::thumbnail_worker::ThumbnailWorker;
use crate::core::view_prefs::ViewPreferences;
use crate::core::watcher::{LibraryWatcher, WatcherEvent};
use std::net::TcpListener as StdTcpListener;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let settings = Settings::from_args();
    let port = resolve_port(settings.port);
    let lan = resolve_lan_binding(&settings);

    // ---------- 服务构建 ----------
    let paths = LibraryPaths::new(&settings.library_root, None);
    paths.ensure_layout();
    let config = Arc::new(LibraryConfig::new(paths.clone()));
    let db = Arc::new(IndexDb::open(&paths.index_db_file));
    let store = Arc::new(MetadataStore::new(paths.clone(), db.clone()));
    let index = Arc::new(ItemIndex::default());
    let bus = EventBus::new();
    let categories = Arc::new(CategoryRegistry::new(&paths));
    let tags = Arc::new(TagRegistry::new(&paths));
    let prefs = Arc::new(ViewPreferences::new(&paths));
    let thumbs = ThumbnailService::new(Arc::new(paths.clone()));
    let worker = ThumbnailWorker::new(thumbs.clone(), config.clone(), bus.clone());
    let migrator = Arc::new(TaxonomyMigrator::new(
        store.clone(),
        index.clone(),
        categories.clone(),
        tags.clone(),
        bus.clone(),
    ));
    let scanner = LibraryScanner::new(paths.clone(), config.clone());
    let startup = Arc::new(StartupState::default());
    let pipeline = IndexPipeline::new(
        paths.clone(),
        config.clone(),
        store.clone(),
        index.clone(),
        thumbs.clone(),
        bus.clone(),
        scanner,
        migrator,
        prefs.clone(),
        worker.clone(),
        startup.clone(),
        settings.clone(),
    );

    let state: SharedState = Arc::new(api::AppState {
        settings: settings.clone(),
        paths: paths.clone(),
        config,
        startup: startup.clone(),
        index,
        bus: bus.clone(),
        pipeline: pipeline.clone(),
        thumbs,
        prefs: prefs.clone(),
        categories,
        tags,
        worker,
    });

    // ---------- 先监听 ----------
    let local_listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("绑定本地端口失败");
    let lan_listener = match lan {
        Some(lan_port) => Some(
            tokio::net::TcpListener::bind(("0.0.0.0", lan_port))
                .await
                .expect("绑定局域网端口失败"),
        ),
        None => None,
    };

    let app = api::build_router(state.clone());

    // 先监听端口：注水/缓存重建期间 startup 端点即可答 starting，客户端有进度反馈
    let local_addr = local_listener.local_addr().unwrap();
    let local_serve = tokio::spawn(async move {
        if let Err(e) = axum::serve(local_listener, app.clone())
            .with_graceful_shutdown(shutdown_signal())
            .await
        {
            tracing::error!("本地监听退出: {e}");
        }
    });
    let lan_serve = if let Some(listener) = lan_listener {
        let addr = listener.local_addr().unwrap();
        let app = api::build_router(state.clone());
        Some(tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app)
                .with_graceful_shutdown(shutdown_signal())
                .await
            {
                tracing::error!("局域网监听退出: {e}");
            }
            let _ = addr;
        }))
    } else {
        None
    };

    // ---------- 随后装配索引流水线（Kestrel 已监听：以下单例的首次构造/注水期间，startup 端点持续可答） ----------
    pipeline.start();
    let watcher = start_watcher(&paths, pipeline.clone(), prefs);
    startup.mark_ready();
    tracing::info!("hawk-server 已就绪（内存索引已由缓存注水），后台对账扫描进行中");

    // 全库对账扫描转后台：完成前停机期间的删除/新增短暂残留（watcher 实时事件已覆盖运行期变更），
    // 失败不置启动错误——周期对账（默认 60s）兜底重试
    {
        let pipeline = pipeline.clone();
        tokio::spawn(async move {
            if let Err(e) = pipeline.run_scan(false).await {
                tracing::error!("后台对账扫描失败（周期对账将重试）: {e}");
            }
        });
    }

    tracing::info!("hawk-server 监听 http://127.0.0.1:{local_addr_port}，素材库: {library}", local_addr_port = local_addr.port(), library = settings.library_root);

    // 保持进程存活直至退出信号
    shutdown_signal().await;
    let _ = watcher;
    let _ = local_serve;
    let _ = lan_serve;
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

/// 默认端口被占用时回退为动态分配
fn resolve_port(preferred: u16) -> u16 {
    match StdTcpListener::bind(("127.0.0.1", preferred)) {
        Ok(listener) => {
            let port = listener.local_addr().map(|a| a.port()).unwrap_or(preferred);
            drop(listener);
            port
        }
        Err(_) => 0,
    }
}

/// 监听地址：桌面 API 恒为环回；[web] 启用且配好 token 时追加局域网绑定。
/// LAN 端口被占用直接启动失败（报错可见），不做静默回退——局域网访问依赖固定端口
fn resolve_lan_binding(settings: &Settings) -> Option<u16> {
    let web = LibraryConfig::peek_web(&settings.library_root);
    if !web.enabled {
        return None;
    }
    if web.token.is_none() {
        eprintln!("[web] enabled 但缺少 token，局域网查看未启动（在设置面板配置 token）");
        return None;
    }
    match StdTcpListener::bind(("0.0.0.0", web.port)) {
        Ok(listener) => {
            drop(listener);
            Some(web.port)
        }
        Err(_) => {
            eprintln!("局域网查看端口 {} 被占用，hawk-server 启动失败：请更换端口或关闭占用进程", web.port);
            std::process::exit(3);
        }
    }
}

fn start_watcher(
    paths: &LibraryPaths,
    pipeline: IndexPipeline,
    prefs: Arc<ViewPreferences>,
) -> Arc<LibraryWatcher> {
    use crate::core::events::REASON_EXTERNAL;
    let watcher = LibraryWatcher::new(paths.clone(), {
        Arc::new(move |event| match event {
            WatcherEvent::FileUpsert(abs) => pipeline.notify_upsert(abs),
            WatcherEvent::Deleted(abs) => pipeline.notify_deleted(abs),
            WatcherEvent::Moved { old, new } => pipeline.notify_moved(old, new),
            WatcherEvent::FolderCreated(path) => {
                tracing::trace!("目录创建: {path}");
                pipeline.notify_folder_changed(REASON_EXTERNAL);
            }
            WatcherEvent::ConfigChanged => pipeline.notify_config_changed(),
            WatcherEvent::RegistryChanged => pipeline.notify_registry_changed(),
            WatcherEvent::PreferencesChanged => prefs.reload(),
            WatcherEvent::Overflow => pipeline.notify_overflow(),
        })
    });
    watcher.start();
    watcher
}

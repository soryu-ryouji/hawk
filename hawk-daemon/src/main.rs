//! 组装与启动：参数解析、服务构建、中间件、启动顺序（先监听、索引后台构建）、就绪信号。
//! 启动模型：axum serve 先拉起监听，
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
    // --dump-openapi：打印代码生成的 OpenAPI schema 到 stdout 后退出（openapi.json 的固化来源；
    // 契约测试校验二者同步，改 API 后用 `cargo run -- --dump-openapi > openapi.json` 更新）
    if std::env::args().any(|a| a == "--dump-openapi") {
        print!("{}", api::build_openapi_json());
        return;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let settings = Settings::from_args();
    let port = resolve_port(settings.port);

    // ---------- 服务构建 ----------
    let paths = LibraryPaths::new(&settings.library_root, None);
    paths.ensure_layout();
    let config = Arc::new(LibraryConfig::new(paths.clone()));
    let db = Arc::new(IndexDb::open(&paths.index_db_file));
    let startup = Arc::new(StartupState::default());
    let store = Arc::new(MetadataStore::new(paths.clone(), db.clone(), &startup));
    let index = Arc::new(ItemIndex::default());
    let bus = EventBus::new();
    let categories = Arc::new(CategoryRegistry::new(&paths));
    let tags = Arc::new(TagRegistry::new(&paths));
    let prefs = Arc::new(ViewPreferences::new(&paths));
    let thumbs = ThumbnailService::new(Arc::new(paths.clone()));
    let worker = ThumbnailWorker::new(thumbs.clone(), bus.clone());
    let migrator = Arc::new(TaxonomyMigrator::new(
        store.clone(),
        index.clone(),
        categories.clone(),
        tags.clone(),
        bus.clone(),
    ));
    let scanner = LibraryScanner::new(paths.clone(), config.clone());
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
    // worker 回流接线：worker 对索引/元数据只读访问，计算结果经队列回流水线（单写者）。
    // 必须在 pipeline.start()（worker.start）之前完成
    worker.attach(index.clone(), store.clone(), pipeline.sender());

    // LAN 监听 supervisor：期望态收敛（首轮回合即按配置绑定，变更由 watcher 唤醒重绑）
    let lan = api::lan::LanSupervisor::new();

    let state: SharedState = Arc::new(api::AppState {
        settings: settings.clone(),
        paths: paths.clone(),
        config: config.clone(),
        startup: startup.clone(),
        index,
        bus: bus.clone(),
        pipeline: pipeline.clone(),
        thumbs,
        prefs: prefs.clone(),
        categories,
        tags,
        worker,
        lan: lan.clone(),
    });

    // ---------- 先监听 ----------
    let local_listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("绑定本地端口失败");

    let app = api::build_router(state.clone());

    // 先监听端口：注水/缓存重建期间 startup 端点即可答 starting，客户端有进度反馈
    let local_addr = local_listener.local_addr().unwrap();
    let local_serve = tokio::spawn(async move {
        if let Err(e) = axum::serve(local_listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await
        {
            tracing::error!("本地监听退出: {e}");
        }
    });
    let lan_task = tokio::spawn(lan.clone().run(state.clone()));

    // ---------- 随后装配索引流水线（HTTP 已监听：以下单例的首次构造/注水期间，startup 端点持续可答） ----------
    pipeline.start();
    let watcher = start_watcher(&paths, pipeline.clone(), prefs, &config, &lan);
    startup.mark_ready();
    tracing::info!("hawk-daemon 已就绪（内存索引已由缓存注水），后台对账扫描进行中");

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

    tracing::info!("hawk-daemon 监听 http://127.0.0.1:{local_addr_port}，素材库: {library}", local_addr_port = local_addr.port(), library = settings.library_root);

    // 保持进程存活直至退出信号
    shutdown_signal().await;
    let _ = watcher;
    let _ = local_serve;
    let _ = lan_task;
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

fn start_watcher(
    paths: &LibraryPaths,
    pipeline: IndexPipeline,
    prefs: Arc<ViewPreferences>,
    config: &Arc<LibraryConfig>,
    lan: &Arc<api::lan::LanSupervisor>,
) -> Arc<LibraryWatcher> {
    use crate::core::events::REASON_EXTERNAL;
    let watcher = LibraryWatcher::new(paths.clone(), {
        let config = config.clone();
        let lan = lan.clone();
        Arc::new(move |event| match event {
            WatcherEvent::FileUpsert(abs) => pipeline.notify_upsert(abs),
            WatcherEvent::Deleted(abs) => pipeline.notify_deleted(abs),
            WatcherEvent::Moved { old, new } => pipeline.notify_moved(old, new),
            WatcherEvent::FolderCreated(path) => {
                tracing::trace!("目录创建: {path}");
                pipeline.notify_folder_changed(REASON_EXTERNAL);
            }
            // 先重读配置再按变更分类分发：ignore 变化 → 强制重扫；[web] 变化 → LAN 热重绑。
            // 仅 name 变化无后续动作（library/info 每次读 current()）
            WatcherEvent::ConfigChanged => {
                let change = config.reload();
                if change.ignore_changed {
                    pipeline.notify_config_changed();
                }
                if change.web_changed {
                    lan.wake();
                }
            }
            WatcherEvent::RegistryChanged => pipeline.notify_registry_changed(),
            WatcherEvent::PreferencesChanged => prefs.reload(),
            WatcherEvent::Overflow => pipeline.notify_overflow(),
        })
    });
    watcher.start();
    watcher
}

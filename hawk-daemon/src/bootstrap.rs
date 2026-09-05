//! 进程组装与启动编排（main 只留入口职责，组件复杂度收敛于此）。
//! 组件图即 api::AppState（单一共享状态对象，axum 生态惯例），按依赖分层构造
//! （存储底座 → 索引流水线 → 派生服务 → LAN），构造顺序与 AppState 字段分组一致。
//! 启动模型：axum serve 先拉起监听，随后装配索引流水线——内存索引由元数据缓存
//! （SQLite 快路径/TOML 回退）注水，就绪不再等待全库扫描；
//! 停机期间的文件增删改由监听实时事件 + 后台对账扫描收敛。

use crate::api;
use crate::api::SharedState;
use crate::core::config::LibraryConfig;
use crate::core::events::EventBus;
use crate::core::global_filter::{publish_changed as publish_global_filter_changed, GlobalFilter};
use crate::core::index::ItemIndex;
use crate::core::metadata_store::MetadataStore;
use crate::core::paths::LibraryPaths;
use crate::core::pipeline::IndexPipeline;
use crate::core::scanner::LibraryScanner;
use crate::core::startup::StartupState;
use crate::core::taxonomy::{CategoryRegistry, TagRegistry, TaxonomyMigrator};
use crate::core::thumbnail::ThumbnailService;
use crate::core::thumbnail_worker::ThumbnailWorker;
use crate::core::view_prefs::ViewPreferences;
use crate::core::watcher::{LibraryWatcher, WatcherEvent};
use crate::settings::Settings;
use std::net::TcpListener as StdTcpListener;
use std::sync::Arc;

/// 进程入口：组装组件图 → 先监听（startup 端点可答 starting）→ 装配索引流水线 → 就绪 → 保活至退出信号
pub async fn run(settings: Settings) {
    let port = resolve_port(settings.port);
    let state = build_state(settings);

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
    let lan_task = tokio::spawn(state.lan.clone().run(state.clone()));

    // 文件夹树缓存失效：与 folder.changed 同线（事件即「目录结构可能变化」信号，含扫描兜底）
    state.folder_tree.spawn_invalidation(&state.bus);

    // ---------- 随后装配索引流水线（HTTP 已监听：以下单例的首次构造/注水期间，startup 端点持续可答） ----------
    state.pipeline.start();
    let watcher = start_watcher(&state);
    state.startup.mark_ready();
    tracing::info!("hawk-daemon 已就绪（内存索引已由缓存注水），后台对账扫描进行中");

    // 全库对账扫描转后台：完成前停机期间的删除/新增短暂残留（watcher 实时事件已覆盖运行期变更），
    // 失败不置启动错误——周期对账（默认 60s）兜底重试
    {
        let pipeline = state.pipeline.clone();
        tokio::spawn(async move {
            if let Err(e) = pipeline.run_scan(false).await {
                tracing::error!("后台对账扫描失败（周期对账将重试）: {e}");
            }
        });
    }

    tracing::info!(
        "hawk-daemon 监听 http://127.0.0.1:{}，素材库: {}",
        local_addr.port(),
        state.settings.library_root
    );

    // 保持进程存活直至退出信号
    shutdown_signal().await;
    let _ = watcher;
    let _ = local_serve;
    let _ = lan_task;
}

/// 组件图组装（→ api::AppState，组合根）：分段顺序即依赖顺序；
/// db/store/scanner/migrator 是构造中间件（由 pipeline/JobCtx 持有），不进组件图
fn build_state(settings: Settings) -> SharedState {
    // ---- 存储底座 ----
    let paths = LibraryPaths::new(&settings.library_root, settings.cache_parent.clone());
    // 缓存位置底线校验（主进程传参错误/手工参数场景；桌面端设置面板已先做用户友好校验）
    if let Some(reason) = paths.cache_location_error() {
        eprintln!("缓存目录位置非法: {reason}（库: {}，缓存: {}）", paths.root, paths.cache_dir);
        std::process::exit(2);
    }
    paths.ensure_layout();
    let config = Arc::new(LibraryConfig::new(paths.clone()));
    let folder_tree = Arc::new(api::folder::FolderTreeCache::new());
    let startup = Arc::new(StartupState::default());
    let store = Arc::new(MetadataStore::new(paths.clone(), &startup));

    // ---- 索引流水线（单写者） ----
    let index = Arc::new(ItemIndex::default());
    let bus = EventBus::new();
    let scanner = LibraryScanner::new(paths.clone(), config.clone());

    // ---- 分类与视图偏好 ----
    let categories = Arc::new(CategoryRegistry::new(&paths));
    let tags = Arc::new(TagRegistry::new(&paths));
    let prefs = Arc::new(ViewPreferences::new(&paths));
    let global_filter = Arc::new(GlobalFilter::new(&paths));
    let migrator = Arc::new(TaxonomyMigrator::new(
        store.clone(),
        index.clone(),
        categories.clone(),
        tags.clone(),
        bus.clone(),
    ));

    // ---- 缩略图派生 ----
    let thumbs = ThumbnailService::new(Arc::new(paths.clone()));
    let worker = ThumbnailWorker::new(thumbs.clone(), bus.clone());

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
        global_filter.clone(),
        worker.clone(),
        startup.clone(),
        settings.clone(),
    );
    // worker 回流接线：worker 对索引/元数据只读访问，计算结果经队列回流水线（单写者）。
    // 必须在 pipeline.start()（worker.start）之前完成
    worker.attach(index.clone(), store.clone(), pipeline.sender());

    // ---- LAN 监听 supervisor：期望态收敛（首轮回合即按配置绑定，变更由 watcher 唤醒重绑） ----
    let lan = api::lan::LanSupervisor::new();

    // 组件图落位：字段分组见 AppState 定义处注释
    Arc::new(api::AppState {
        settings,
        paths,
        config,
        folder_tree,
        startup,
        index,
        bus,
        pipeline,
        store,
        thumbs,
        worker,
        prefs,
        categories,
        tags,
        global_filter,
        lan,
    })
}

/// 文件/配置监听接线：变更按类别分发（流水线任务 / 配置热更 / 视图偏好重读 / LAN 重绑）。
/// 返回的 watcher 由调用方持有保活
fn start_watcher(state: &api::AppState) -> Arc<LibraryWatcher> {
    use crate::core::events::REASON_EXTERNAL;
    let watcher = LibraryWatcher::new(state.paths.clone(), {
        let pipeline = state.pipeline.clone();
        let config = state.config.clone();
        let paths = state.paths.clone();
        let folder_tree = state.folder_tree.clone();
        let prefs = state.prefs.clone();
        let global_filter = state.global_filter.clone();
        let bus = state.bus.clone();
        let lan = state.lan.clone();
        Arc::new(move |event| match event {
            WatcherEvent::FileUpsert(abs) => pipeline.notify_upsert(abs),
            WatcherEvent::Deleted(abs) => {
                pipeline.notify_deleted(abs.clone());
                // 外部空目录删除无索引/注册表信号（Windows 的 Remove 事件不区分文件/目录）：
                // 以目录树缓存为参照，命中已知目录才广播 folder.changed（缓存冷时不发，下次建树即最新）
                if let Some(rel) = paths.to_relative(&abs) {
                    if !rel.is_empty() && folder_tree.known_dir(&rel) {
                        pipeline.notify_folder_changed(REASON_EXTERNAL);
                    }
                }
            }
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
                    // ignore 规则参与树构建，与重扫同点失效
                    folder_tree.invalidate();
                    pipeline.notify_config_changed();
                }
                if change.web_changed {
                    lan.wake();
                }
            }
            WatcherEvent::RegistryChanged => pipeline.notify_registry_changed(),
            WatcherEvent::PreferencesChanged => prefs.reload(),
            // 隐藏集外部变更（网盘同步落地）：重载后有变化才广播，客户端重查列表
            WatcherEvent::GlobalFilterChanged => {
                if global_filter.reload() {
                    publish_global_filter_changed(&bus, &global_filter.snapshot());
                }
            }
            WatcherEvent::Overflow => pipeline.notify_overflow(),
        })
    });
    watcher.start();
    watcher
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

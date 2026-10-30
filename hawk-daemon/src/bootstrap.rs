//! 进程组装与启动编排（main 只留入口职责，组件复杂度收敛于此）。
//! 组件图即 api::AppState（单一共享状态对象，axum 生态惯例），按依赖分层构造
//! （存储底座 → 索引流水线 → 派生服务 → LAN），构造顺序与 AppState 字段分组一致。
//! 启动模型：axum serve 先拉起监听，随后装配索引流水线——内存索引直接由
//! .hawk/metadata.db 注水，就绪不再等待全库扫描；
//! 停机期间的文件增删改由监听实时事件 + 后台对账扫描收敛。

use crate::api;
use crate::api::SharedState;
use crate::core::config::LibraryConfig;
use crate::core::events::EventBus;
use crate::core::global_filter::{publish_changed as publish_global_filter_changed, GlobalFilter};
use crate::core::index::ItemIndex;
use crate::core::locks::{publish_changed as publish_locks_changed, Locks};
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
use crate::settings::{LocalArgs, RemoteArgs, Settings};
use std::sync::Arc;

/// 本地素材库模式
pub async fn run_local(args: LocalArgs) {
    run(Settings::from_local(args)).await;
}

/// 无本地库的远程素材库查看模式
pub async fn run_remote(args: RemoteArgs) {
    run(Settings::from_remote(args)).await;
}

/// daemon 启动流程：初始化诊断 → 先监听（startup 端点可答 starting）→ 组装组件图
/// → 装配索引流水线 → 就绪 → 保活至退出信号。
///
/// 端口严格按 `--port` 绑定，绑不上就报错退出（不静默换端口）：端口的唯一所有权在壳层（Electron
/// 读取用户配置后传入），生态客户端（浏览器插件）据此按固定段探活；daemon 自行漂移会让客户端追不上。
async fn run(settings: Settings) {
    init_diagnostics();
    let port = settings.port;

    // ---------- 先监听 ----------
    // 先绑端口再建索引：端口冲突要在秒级失败（否则被占时仍要白跑一遍索引）
    let local_listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(listener) => listener,
        Err(e) => bind_failed(port, e),
    };
    let state = build_state(settings);

    // 启动期先同步收敛一次 LAN 监听：已有 peer_keys/网站密钥的配置无需用户再进设置保存。
    let _ = state.lan.reconcile_once(&state).await;

    let app = api::build_router(state.clone());
    // remote 数据面转发目标：同一套路由的克隆（A 侧隧道端进程内转发，标记 TunneledPeer）
    let tunnel_router = api::build_router(state.clone());

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
    let remote_library = state
        .library
        .as_ref()
        .map(|lib| crate::remote::LibraryContext {
            paths: lib.paths.clone(),
            config: lib.config.clone(),
        });
    let remote_task = tokio::spawn(state.remote.clone().run(remote_library, tunnel_router));

    // 库子系统（流水线/文件监听/LAN）仅本地库模式装配；远程查看模式监听就绪即 ready
    let mut lan_task = None;
    let mut watcher = None;
    if let Some(lib) = &state.library {
        lan_task = Some(tokio::spawn(state.lan.clone().run(state.clone())));

        // 文件夹树缓存失效：与 folder.changed 同线（事件即「目录结构可能变化」信号，含扫描兜底）
        lib.folder_tree.spawn_invalidation(&state.bus);

        // SQLite 读取失败已由 MetadataStore 写入 startup：保持未就绪，不能把空副本当空库继续写
        if state.startup.snapshot().1.is_some() {
            tracing::error!("元数据初始化失败，保持启动错误态；修复数据库后重启");
        } else {
            // ---------- 随后装配索引流水线（HTTP 已监听：startup 端点持续可答） ----------
            lib.pipeline.start();
            watcher = Some(start_watcher(&state, lib));
            state.startup.mark_ready();
            tracing::info!("hawk-daemon 已就绪（内存索引已由 SQLite 注水），后台对账扫描进行中");

            // 全库对账扫描转后台：完成前停机期间的删除/新增短暂残留（watcher 实时事件已覆盖运行期变更）
            let pipeline = lib.pipeline.clone();
            tokio::spawn(async move {
                if let Err(e) = pipeline.run_scan(false).await {
                    tracing::error!("后台对账扫描失败（周期维护将重试）: {e}");
                }
            });
        }
    } else {
        state.startup.mark_ready();
        tracing::info!("hawk-daemon 已就绪（远程查看模式，无本地素材库）");
    }

    tracing::info!(
        "hawk-daemon 监听 http://127.0.0.1:{}，素材库: {}",
        local_addr.port(),
        state
            .settings
            .library_root
            .as_deref()
            .unwrap_or("（无，远程查看模式）")
    );

    // 保持进程存活直至退出信号
    shutdown_signal().await;
    // 显式 drop：句柄只为保活（任务已分离），退出时一并丢弃
    drop((watcher, lan_task, local_serve, remote_task));
}

/// 组件图组装（→ api::AppState，组合根）：分段顺序即依赖顺序；
/// db/store/scanner/migrator 是构造中间件（由 pipeline/JobCtx 持有），不进组件图。
/// 无库模式（settings.library_root 为 None）不构建 LibraryState，仅保留远程子系统
fn build_state(settings: Settings) -> SharedState {
    let startup = Arc::new(StartupState::default());
    let bus = EventBus::new();
    let lan = api::lan::LanSupervisor::new();

    let library = settings
        .library_root
        .as_deref()
        .map(|root| build_library_state(root, &settings, &startup, &bus));

    // ---- 远程访问 supervisor：remote.toml 启用才激活信令（同 LAN 的 reconcile 模型） ----
    let remote_config = crate::remote::config::RemoteConfigStore::open(
        settings.remote_config.as_deref(),
        library.as_ref().map(|lib| &lib.paths),
    );
    let remote = crate::remote::Supervisor::new(bus.clone(), remote_config);

    // 组件图落位：字段分组见 AppState 定义处注释
    Arc::new(api::AppState {
        settings,
        startup,
        bus,
        sse_lagged: std::sync::atomic::AtomicU64::new(0),
        lan,
        remote,
        library,
    })
}

/// 本地库状态组装（→ api::LibraryState）：存储底座 → 索引流水线 → 派生服务
fn build_library_state(
    root: &str,
    settings: &Settings,
    startup: &Arc<StartupState>,
    bus: &EventBus,
) -> Arc<api::LibraryState> {
    // ---- 存储底座 ----
    let paths = LibraryPaths::new(root, settings.cache_parent.clone());
    // 缓存位置底线校验（主进程传参错误/手工参数场景；桌面端设置面板已先做用户友好校验）
    if let Some(reason) = paths.cache_location_error() {
        eprintln!(
            "缓存目录位置非法: {reason}（库: {}，缓存: {}）",
            paths.root, paths.cache_dir
        );
        std::process::exit(2);
    }
    paths.ensure_layout();
    let config = Arc::new(LibraryConfig::new(paths.clone()));
    let folder_tree = Arc::new(api::folder::FolderTreeCache::new());
    let store = Arc::new(MetadataStore::new(paths.clone(), startup));

    // ---- 索引流水线（单写者） ----
    let index = Arc::new(ItemIndex::default());
    let scanner = LibraryScanner::new(paths.clone(), config.clone());

    // ---- 分类与视图偏好 ----
    let categories = Arc::new(CategoryRegistry::new(&paths));
    let tags = Arc::new(TagRegistry::new(&paths));
    let prefs = Arc::new(ViewPreferences::new(&paths));
    let global_filter = Arc::new(GlobalFilter::new(&paths));
    let locks = Arc::new(Locks::new(&paths));
    let migrator = Arc::new(TaxonomyMigrator::new(
        store.clone(),
        index.clone(),
        categories.clone(),
        tags.clone(),
        locks.clone(),
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
        locks.clone(),
        worker.clone(),
        startup.clone(),
        settings.clone(),
    );
    // worker 回流接线：worker 对索引/元数据只读访问，计算结果经队列回流水线（单写者）。
    // 必须在 pipeline.start()（worker.start）之前完成
    worker.attach(index.clone(), store.clone(), pipeline.sender());

    Arc::new(api::LibraryState {
        paths,
        config,
        folder_tree,
        index,
        pipeline,
        store,
        thumbs,
        worker,
        prefs,
        categories,
        tags,
        global_filter,
        locks,
    })
}

/// 文件/配置监听接线：变更按类别分发（流水线任务 / 配置热更 / 视图偏好重读 / LAN 重绑）。
/// 返回的 watcher 由调用方持有保活
fn start_watcher(state: &api::AppState, lib: &api::LibraryState) -> Arc<LibraryWatcher> {
    use crate::core::events::REASON_EXTERNAL;
    let watcher = LibraryWatcher::new(lib.paths.clone(), lib.config.clone(), {
        let pipeline = lib.pipeline.clone();
        let config = lib.config.clone();
        let paths = lib.paths.clone();
        let folder_tree = lib.folder_tree.clone();
        let prefs = lib.prefs.clone();
        let global_filter = lib.global_filter.clone();
        let locks = lib.locks.clone();
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
                if change.ignore_changed || change.extensions_changed {
                    // ignore 规则参与树构建，与重扫同点失效；扩展名白名单变化不改变目录结构，
                    // 但需强制重扫让既有条目收敛（新增后缀入库、移除后缀出库）
                    if change.ignore_changed {
                        folder_tree.invalidate();
                    }
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
            // 锁集外部变更（网盘同步落地）：重载后有变化才广播，客户端重拉标记并重查列表；
            // 锁条目消失/改名时残留票据自然失效（guard 按当前锁集求差）
            WatcherEvent::LocksChanged => {
                if locks.reload() {
                    publish_locks_changed(&bus, &locks.snapshot());
                }
            }
            WatcherEvent::Overflow => pipeline.notify_overflow(),
            // 系统告知某路径事件被丢弃：定向强制重扫该子树（根路径 = 整库）
            WatcherEvent::Rescan(rel) => pipeline.request_scoped_rescan(rel),
        })
    });
    watcher.start();
    watcher
}

fn init_diagnostics() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // panic 也进日志（默认 hook 仍保留：stderr 尾部是 Electron 侧错误屏的数据源）
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panic: {info}");
        default_hook(info);
    }));
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

/// 端口绑定失败：端口占用以固定文案 + 退出码 4 上报（壳层据此弹「端口被占用」的处理界面），
/// 其他绑定错误退出码 5。不用 panic——stderr 尾部是壳层错误屏的数据源，需要可解析的稳定格式
fn bind_failed(port: u16, error: std::io::Error) -> ! {
    if error.kind() == std::io::ErrorKind::AddrInUse {
        eprintln!("PORT_IN_USE {port}");
        std::process::exit(4);
    }
    eprintln!("PORT_BIND_FAILED {port}: {error}");
    std::process::exit(5);
}

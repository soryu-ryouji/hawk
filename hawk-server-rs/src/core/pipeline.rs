//! 索引流水线:监听事件 / 扫描 / API 写操作全部经有界队列串行处理(单写者)，
//! 哈希与元数据迁移在消费者内完成;缩略图生成在 ThumbnailWorker 后台线程,不阻塞索引。
//! 索引与元数据的所有变更只发生在这里,处理逻辑保证幂等(重复事件无害)。
//! 与 C# IndexPipeline 语义一致；一处改进:宽高在入库即持久化入 TOML
//! （C# 仅在内存更新，重启后依赖扫描重新识别）。

use crate::core::color::{self, PALETTE_VERSION};
use crate::core::color_math;
use crate::core::config::LibraryConfig;
use crate::core::content_hash;
use crate::core::events::{folder_changed_payload, EventBus, TaskProgress, REASON_EXTERNAL};
use crate::core::index::ItemIndex;
use crate::core::item::{ItemDto, PaletteColor};
use crate::core::metadata::{ItemMetadata, PaletteEntry, PathEntry};
use crate::core::metadata_store::MetadataStore;
use crate::core::paths::{unix_ms, LibraryPaths};
use crate::core::scanner::LibraryScanner;
use crate::core::startup::StartupState;
use crate::core::taxonomy::{ItemEvents, TaxonomyMigrator};
use crate::core::thumbnail::ThumbnailService;
use crate::core::thumbnail_worker::ThumbnailWorker;
use crate::core::view_prefs::ViewPreferences;
use crate::settings::Settings;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

/// item/add 的处理结果:索引后的 item 投影与「内容是否已存在」标志
pub struct UpsertResult {
    pub item: ItemDto,
    /// 内容是否已存在；item/add 以「写入前」预计算为准（与 C# 同语义），此字段供其他调用方使用
    #[allow(dead_code)]
    pub already_existed: bool,
}

#[derive(Clone, Debug)]
pub struct ScanProgress {
    pub phase: String,
    pub processed: i32,
    pub total: i32,
}

#[derive(Debug)]
pub struct BatchMetadataResult {
    pub updated: usize,
    pub missing_ids: Vec<String>,
}

const MAX_DEBOUNCE_ATTEMPTS: u32 = 120;
/// 写入防抖窗口：mtime 距今不足该值的文件视为仍在写入，延迟重试
const STABILITY_WINDOW_MS: i64 = 1000;
/// 调色板批量回写：达到该条数立即冲刷（SQLite 事务开销从 N 降到 1，
/// 事件按批平滑补发，避免全库重提炼时的 item.updated 洪峰）
const PALETTE_BATCH: usize = 500;
/// 调色板批量回写的时间冲刷阈值：滞留超时时即使未达批大小也冲刷
const PALETTE_FLUSH_AFTER: Duration = Duration::from_millis(2000);

type Reply<T> = Option<oneshot::Sender<T>>;

enum Job {
    Upsert {
        abs: String,
        force_hash: bool,
        known_hash: Option<String>,
        reply: Reply<Result<Option<UpsertResult>, String>>,
        attempt: u32,
    },
    Delete {
        abs: String,
    },
    Move {
        old_abs: String,
        new_abs: String,
        reply: Reply<Result<(), String>>,
    },
    DirMove {
        old_abs: String,
        new_abs: String,
        reply: Reply<Result<(), String>>,
    },
    Scan {
        full: bool,
        force_walk: bool,
        reply: Reply<Result<(), String>>,
    },
    ClearTrash {
        reply: Reply<Result<(), String>>,
    },
    Metadata {
        hash: String,
        mutate: Box<dyn FnOnce(&mut ItemMetadata) + Send>,
        reply: Reply<Result<(), String>>,
    },
    BatchMetadata {
        hashes: Vec<String>,
        mutate: Box<dyn FnMut(&mut ItemMetadata) + Send>,
        reply: Reply<Result<BatchMetadataResult, String>>,
    },
    MetadataSync {
        reply: Reply<Result<(), String>>,
    },
    PaletteFlush,
    Palette {
        hash: String,
        palette: Vec<PaletteColor>,
    },
    FolderHint {
        reason: String,
    },
    CategoryCreate {
        name: String,
        reply: Reply<Result<(), String>>,
    },
    CategoryUpdate {
        old_name: String,
        new_name: String,
        reply: Reply<Result<(), String>>,
    },
    CategoryDelete {
        name: String,
        reply: Reply<Result<(), String>>,
    },
    TagCreate {
        name: String,
        reply: Reply<Result<(), String>>,
    },
    TagUpdate {
        name: String,
        new_name: String,
        reply: Reply<Result<(), String>>,
    },
    TagDelete {
        name: String,
        reply: Reply<Result<(), String>>,
    },
    RegistryReload,
}

pub struct PipelineCtx {
    paths: LibraryPaths,
    config: Arc<LibraryConfig>,
    store: Arc<MetadataStore>,
    index: Arc<ItemIndex>,
    thumbs: ThumbnailService,
    bus: EventBus,
    scanner: LibraryScanner,
    migrator: Arc<TaxonomyMigrator>,
    prefs: Arc<ViewPreferences>,
    worker: Arc<ThumbnailWorker>,
    startup: Arc<StartupState>,
    settings: Settings,
    tx: std::sync::mpsc::SyncSender<Job>,
    overflow: Arc<AtomicBool>,
    scan_scheduled: Arc<AtomicBool>,
    scanning: Arc<AtomicBool>,
    deferred: Arc<Mutex<HashSet<String>>>,
    queued_jobs: Arc<AtomicI32>,
    last_scan: Arc<Mutex<Option<ScanProgress>>>,
    progress_last_at: Arc<AtomicI64>,
    progress_idle: Arc<AtomicBool>,
    /// 暂存的调色板回写（hash → 最新提炼结果）；同 hash 去重，按批冲刷
    palette_pending: Arc<Mutex<Vec<(String, Vec<PaletteColor>)>>>,
    palette_oldest: Arc<Mutex<Option<std::time::Instant>>>,
    /// 冲刷定时任务是否已排队（避免重复 spawn）
    palette_timer: Arc<AtomicBool>,
    runtime: tokio::runtime::Handle,
}

#[derive(Clone)]
pub struct IndexPipeline {
    ctx: Arc<PipelineCtx>,
    rx: Arc<Mutex<Option<std::sync::mpsc::Receiver<Job>>>>,
}

struct PendingUpsert {
    abs_path: String,
    rel: String,
    lib_path: String,
    size: i64,
    mtime: i64,
    old_hash: Option<String>,
    reused_hash: Option<String>,
    hash: Option<String>,
    dim: Option<(i32, i32)>,
}

impl IndexPipeline {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        paths: LibraryPaths,
        config: Arc<LibraryConfig>,
        store: Arc<MetadataStore>,
        index: Arc<ItemIndex>,
        thumbs: ThumbnailService,
        bus: EventBus,
        scanner: LibraryScanner,
        migrator: Arc<TaxonomyMigrator>,
        prefs: Arc<ViewPreferences>,
        worker: Arc<ThumbnailWorker>,
        startup: Arc<StartupState>,
        settings: Settings,
    ) -> IndexPipeline {
        let (tx, rx) = std::sync::mpsc::sync_channel(4096);
        let ctx = PipelineCtx {
            paths,
            config,
            store,
            index,
            thumbs,
            bus,
            scanner,
            migrator,
            prefs,
            worker,
            startup,
            settings,
            tx,
            overflow: Arc::new(AtomicBool::new(false)),
            scan_scheduled: Arc::new(AtomicBool::new(false)),
            scanning: Arc::new(AtomicBool::new(false)),
            deferred: Arc::new(Mutex::new(HashSet::new())),
            queued_jobs: Arc::new(AtomicI32::new(0)),
            last_scan: Arc::new(Mutex::new(None)),
            progress_last_at: Arc::new(AtomicI64::new(0)),
            progress_idle: Arc::new(AtomicBool::new(true)),
            palette_pending: Arc::new(Mutex::new(Vec::new())),
            palette_oldest: Arc::new(Mutex::new(None)),
            palette_timer: Arc::new(AtomicBool::new(false)),
            runtime: tokio::runtime::Handle::current(),
        };
        IndexPipeline {
            ctx: Arc::new(ctx),
            rx: Arc::new(Mutex::new(Some(rx))),
        }
    }

    // ---------- 启动 ----------

    /// 启动消费循环/缩略图 worker/周期对账，并入队一轮元数据对账（先于初始扫描）
    pub fn start(&self) {
        self.hydrate_index();
        self.attach_worker();

        let rx = self.rx.lock().unwrap().take().expect("pipeline 只能启动一次");
        let ctx = self.ctx.clone();
        std::thread::Builder::new()
            .name("hawk-index-pipeline".to_string())
            .spawn(move || consumer_loop(ctx, rx))
            .expect("启动索引消费线程失败");

        self.ctx.worker.start();

        // 周期对账:只跑元数据对账(.hawk/ 内 TOML,轻量);文件系统变更由 watcher 实时事件 + 启动扫描收敛
        if self.ctx.settings.rescan_interval_seconds > 0 {
            let ctx = self.ctx.clone();
            self.ctx.runtime.spawn(async move {
                let mut ticker = tokio::time::interval(Duration::from_secs(ctx.settings.rescan_interval_seconds));
                ticker.tick().await; // 立即 tick 的一次丢弃
                loop {
                    ticker.tick().await;
                    fire(&ctx, Job::MetadataSync { reply: None });
                }
            });
        }

        // 启动先跑一轮元数据对账（入队于初始扫描之前）：把停机期间网盘同步落地的外部
        // TOML 变更并入内存，避免扫描拿旧副本做迁移继承
        fire(&self.ctx, Job::MetadataSync { reply: None });
    }

    /// 启动注水：内存索引由元数据副本恢复（SQLite 快路径/TOML 回退），就绪无需等待全库扫描
    fn hydrate_index(&self) {
        let entries = self.ctx.store.snapshot();
        for (hash, meta) in &entries {
            self.ctx.index.get_or_add(hash);
            self.ctx
                .index
                .with_item_mut(hash, |item| item.sync_from(meta));
            for p in &meta.paths {
                self.ctx
                    .index
                    .add_or_update_location(hash, &p.path, p.size, p.modification_time);
            }
        }
        if !entries.is_empty() {
            tracing::info!("内存索引已注水 {} 条（来自元数据副本）", entries.len());
        }
    }

    /// 装配缩略图 worker:索引访问/调色板判定/回写的闭环在本类(单写者),worker 只负责生成
    fn attach_worker(&self) {
        let index = self.ctx.index.clone();
        let store = self.ctx.store.clone();
        self.ctx.worker.attach(
            Arc::new(move |hash| index.get_dto(hash)),
            Arc::new(move |hash| match store.try_get(hash) {
                Some(meta) => meta.palette.is_some() && meta.palette_version == PALETTE_VERSION,
                None => false,
            }),
            {
                let ctx = self.ctx.clone();
                Arc::new(move |hash, palette| {
                    // 队列满时丢弃:PaletteJob 幂等,下轮对账/刷新会再触发提炼
                    let _ = try_fire(&ctx, Job::Palette { hash, palette });
                })
            },
        );
    }

    // ---------- 状态快照(task.progress 与 app/status 共用) ----------

    /// 索引管道积压快照:排队 job + 防抖延迟路径 + 是否扫描中
    pub fn backlog(&self) -> (i32, i32, bool) {
        (
            self.ctx.queued_jobs.load(Ordering::SeqCst),
            self.ctx.deferred.lock().unwrap().len() as i32,
            self.ctx.scanning.load(Ordering::SeqCst),
        )
    }

    /// 索引进度快照(task.progress("index") 事件与 app/status 端点共用同一构造)
    pub fn index_progress(&self) -> TaskProgress {
        let (queued, deferred, scanning) = self.backlog();
        let scan = self.ctx.last_scan.lock().unwrap().clone();
        let in_scan = scanning && scan.as_ref().map(|s| s.phase != "done").unwrap_or(false);
        let scan = scan.filter(|_| in_scan);
        TaskProgress {
            task: "index",
            pending: queued + deferred,
            active: if scanning { 1 } else { 0 },
            phase: scan.as_ref().map(|s| s.phase.clone()),
            processed: scan.as_ref().map(|s| s.processed),
            total: scan.as_ref().map(|s| s.total),
        }
    }

    // ---------- 内部入队 ----------

    fn try_enqueue(&self, job: Job) -> Result<(), Job> {
        match self.ctx.tx.try_send(job) {
            Ok(()) => {
                self.ctx.queued_jobs.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            Err(std::sync::mpsc::TrySendError::Full(job)) => Err(job),
            Err(std::sync::mpsc::TrySendError::Disconnected(job)) => Err(job),
        }
    }

    fn fire_and_forget(&self, job: Job) {
        if self.try_enqueue(job).is_err() {
            self.ctx.overflow.store(true, Ordering::SeqCst);
        }
    }

    // ---------- 入口:文件监听(火忘,队列满置溢出标记,由消费者全量扫描兜底) ----------

    pub fn notify_upsert(&self, abs: String) {
        self.fire_and_forget(Job::Upsert {
            abs,
            force_hash: false,
            known_hash: None,
            reply: None,
            attempt: 0,
        });
    }

    pub fn notify_deleted(&self, abs: String) {
        self.fire_and_forget(Job::Delete { abs });
    }

    /// 新路径是目录时按目录移动处理,否则按文件移动
    pub fn notify_moved(&self, old_abs: String, new_abs: String) {
        let job = if std::path::Path::new(&new_abs).is_dir() {
            Job::DirMove {
                old_abs,
                new_abs,
                reply: None,
            }
        } else {
            Job::Move {
                old_abs,
                new_abs,
                reply: None,
            }
        };
        self.fire_and_forget(job);
    }

    /// ignore 规则变化影响全库过滤:强制重新遍历
    pub fn notify_config_changed(&self) {
        self.fire_and_forget(Job::Scan {
            full: false,
            force_walk: true,
            reply: None,
        });
    }

    pub fn notify_overflow(&self) {
        self.ctx.overflow.store(true, Ordering::SeqCst);
    }

    /// 用户手动「刷新缓存」（library/rescan）：忽略快照强制遍历，fire-and-forget
    pub fn request_rescan(&self) {
        self.fire_and_forget(Job::Scan {
            full: false,
            force_walk: true,
            reply: None,
        });
    }

    /// 注册表文件被外部修改(网盘同步等):重新加载
    pub fn notify_registry_changed(&self) {
        self.fire_and_forget(Job::RegistryReload);
    }

    /// 异步触发扫描(library/reindex:立即返回,过程变更照常推送事件)
    pub fn request_scan(&self, full: bool) {
        self.fire_and_forget(Job::Scan {
            full,
            force_walk: false,
            reply: None,
        });
    }

    /// 目录结构可能变化(文件夹增删改移、外部变动、扫描兜底):广播 folder.changed
    pub fn notify_folder_changed(&self, reason: &str) {
        self.fire_and_forget(Job::FolderHint {
            reason: reason.to_string(),
        });
    }

    // ---------- 入口:API / 启动(等待处理完成) ----------

    /// 入库提交。known_hash 为调用方已算好的内容哈希(如 item/add)，提供时流水线跳过重算
    pub async fn submit_upsert(&self, abs: String, known_hash: Option<String>) -> Result<Option<UpsertResult>, String> {
        let (tx, rx) = oneshot::channel();
        let job = Job::Upsert {
            abs,
            force_hash: false,
            known_hash,
            reply: Some(tx),
            attempt: 0,
        };
        self.try_enqueue(job).map_err(|_| "索引队列已满".to_string())?;
        rx.await.map_err(|_| "索引流水线已停止".to_string())?
    }

    pub async fn submit_move(&self, old_abs: String, new_abs: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.try_enqueue(Job::Move {
            old_abs,
            new_abs,
            reply: Some(tx),
        })
        .map_err(|_| "索引队列已满".to_string())?;
        rx.await.map_err(|_| "索引流水线已停止".to_string())?
    }

    pub async fn submit_dir_move(&self, old_abs: String, new_abs: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.try_enqueue(Job::DirMove {
            old_abs,
            new_abs,
            reply: Some(tx),
        })
        .map_err(|_| "索引队列已满".to_string())?;
        rx.await.map_err(|_| "索引流水线已停止".to_string())?
    }

    pub async fn submit_clear_trash(&self) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.try_enqueue(Job::ClearTrash { reply: Some(tx) })
            .map_err(|_| "索引队列已满".to_string())?;
        rx.await.map_err(|_| "索引流水线已停止".to_string())?
    }

    pub async fn submit_metadata(
        &self,
        hash: String,
        mutate: impl FnOnce(&mut ItemMetadata) + Send + 'static,
    ) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.try_enqueue(Job::Metadata {
            hash,
            mutate: Box::new(mutate),
            reply: Some(tx),
        })
        .map_err(|_| "索引队列已满".to_string())?;
        rx.await.map_err(|_| "索引流水线已停止".to_string())?
    }

    /// 批量元数据应用(item/batch_update);不存在的 id 记入 missing_ids
    pub async fn submit_batch_metadata(
        &self,
        hashes: Vec<String>,
        mutate: impl FnMut(&mut ItemMetadata) + Send + 'static,
    ) -> Result<BatchMetadataResult, String> {
        let (tx, rx) = oneshot::channel();
        self.try_enqueue(Job::BatchMetadata {
            hashes,
            mutate: Box::new(mutate),
            reply: Some(tx),
        })
        .map_err(|_| "索引队列已满".to_string())?;
        rx.await.map_err(|_| "索引流水线已停止".to_string())?
    }

    /// 立即触发一轮元数据对账(TOML → 缓存/索引);正常由周期对账驱动,手动重同步用
    #[allow(dead_code)]
    pub async fn submit_metadata_sync(&self) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.try_enqueue(Job::MetadataSync { reply: Some(tx) })
            .map_err(|_| "索引队列已满".to_string())?;
        rx.await.map_err(|_| "索引流水线已停止".to_string())?
    }

    /// 全量扫描。full=true 时对所有文件重算哈希(library/reindex)。
    pub async fn run_scan(&self, full: bool) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.try_enqueue(Job::Scan {
            full,
            force_walk: false,
            reply: Some(tx),
        })
        .map_err(|_| "索引队列已满".to_string())?;
        rx.await.map_err(|_| "索引流水线已停止".to_string())?
    }

    /// 用户手动「刷新缓存」（等待完成）：忽略快照强制遍历（不强制重算哈希）
    #[allow(dead_code)]
    pub async fn submit_rescan(&self) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.try_enqueue(Job::Scan {
            full: false,
            force_walk: true,
            reply: Some(tx),
        })
        .map_err(|_| "索引队列已满".to_string())?;
        rx.await.map_err(|_| "索引流水线已停止".to_string())?
    }

    pub async fn submit_category_create(&self, name: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.try_enqueue(Job::CategoryCreate {
            name,
            reply: Some(tx),
        })
        .map_err(|_| "索引队列已满".to_string())?;
        rx.await.map_err(|_| "索引流水线已停止".to_string())?
    }

    pub async fn submit_category_update(&self, old_name: String, new_name: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.try_enqueue(Job::CategoryUpdate {
            old_name,
            new_name,
            reply: Some(tx),
        })
        .map_err(|_| "索引队列已满".to_string())?;
        rx.await.map_err(|_| "索引流水线已停止".to_string())?
    }

    pub async fn submit_category_delete(&self, name: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.try_enqueue(Job::CategoryDelete {
            name,
            reply: Some(tx),
        })
        .map_err(|_| "索引队列已满".to_string())?;
        rx.await.map_err(|_| "索引流水线已停止".to_string())?
    }

    pub async fn submit_tag_create(&self, name: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.try_enqueue(Job::TagCreate {
            name,
            reply: Some(tx),
        })
        .map_err(|_| "索引队列已满".to_string())?;
        rx.await.map_err(|_| "索引流水线已停止".to_string())?
    }

    pub async fn submit_tag_update(&self, name: String, new_name: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.try_enqueue(Job::TagUpdate {
            name,
            new_name,
            reply: Some(tx),
        })
        .map_err(|_| "索引队列已满".to_string())?;
        rx.await.map_err(|_| "索引流水线已停止".to_string())?
    }

    pub async fn submit_tag_delete(&self, name: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.try_enqueue(Job::TagDelete {
            name,
            reply: Some(tx),
        })
        .map_err(|_| "索引队列已满".to_string())?;
        rx.await.map_err(|_| "索引流水线已停止".to_string())?
    }
}

// ---------- 消费循环 ----------

fn fire(ctx: &PipelineCtx, job: Job) {
    if ctx.tx.try_send(job).is_ok() {
        ctx.queued_jobs.fetch_add(1, Ordering::SeqCst);
    } else {
        ctx.overflow.store(true, Ordering::SeqCst);
    }
}

fn try_fire(ctx: &PipelineCtx, job: Job) -> Result<(), Job> {
    match ctx.tx.try_send(job) {
        Ok(()) => {
            ctx.queued_jobs.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        Err(std::sync::mpsc::TrySendError::Full(job)) => Err(job),
        Err(std::sync::mpsc::TrySendError::Disconnected(job)) => Err(job),
    }
}

fn complete<T>(reply: Reply<T>, value: T) {
    if let Some(tx) = reply {
        let _ = tx.send(value);
    }
}

fn consumer_loop(ctx: Arc<PipelineCtx>, rx: std::sync::mpsc::Receiver<Job>) {
    while let Ok(job) = rx.recv() {
        ctx.queued_jobs.fetch_sub(1, Ordering::SeqCst);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| process_job(&ctx, job)));
        if result.is_err() {
            // 等待中的 API 调用方会因 oneshot 发送端被丢弃而收到 500，不会挂起
            tracing::error!("索引任务处理 panic（已跳过该任务）");
        }

        // 监听事件丢失兜底:不内联扫描(事件风暴期会反复全库扫描),
        // 改为入队去重的 ScanJob——扫描本身会把全部待处理文件入库,一次即可收敛
        if ctx.overflow.swap(false, Ordering::SeqCst) && !ctx.scan_scheduled.swap(true, Ordering::SeqCst) {
            tracing::info!("检测到事件丢失,排队对账扫描");
            fire(&ctx, Job::Scan {
                full: false,
                force_walk: false,
                reply: None,
            });
        }

        maybe_flush_palette(&ctx);
        publish_index_progress(&ctx, false);
    }
}

fn process_job(ctx: &Arc<PipelineCtx>, job: Job) {
    match job {
        Job::Upsert {
            abs,
            force_hash,
            known_hash,
            reply,
            attempt,
        } => {
            let result = do_upsert(ctx, &abs, force_hash, known_hash.as_deref(), attempt, reply.is_some());
            complete(reply, result);
        }
        Job::Delete { abs } => {
            if let Some(rel) = ctx.paths.to_relative(&abs) {
                do_delete(ctx, &rel);
            }
        }
        Job::Move {
            old_abs,
            new_abs,
            reply,
        } => {
            let result = do_move(ctx, &old_abs, &new_abs);
            complete(reply, result);
        }
        Job::DirMove {
            old_abs,
            new_abs,
            reply,
        } => {
            let result = do_dir_move(ctx, &old_abs, &new_abs);
            complete(reply, result);
        }
        Job::Scan {
            full,
            force_walk,
            reply,
        } => {
            ctx.scan_scheduled.store(false, Ordering::SeqCst);
            ctx.config.reload();
            let result = do_scan(ctx, full, force_walk);
            complete(reply, result);
        }
        Job::ClearTrash { reply } => {
            let result = do_clear_trash(ctx);
            complete(reply, result);
        }
        Job::Metadata { hash, mutate, reply } => {
            let result = ctx.migrator.apply_metadata(&hash, mutate).map(|_| ());
            complete(reply, result);
        }
        Job::BatchMetadata {
            hashes,
            mut mutate,
            reply,
        } => {
            let result = (|| {
                let mut missing = Vec::new();
                let updated = ctx.migrator.apply_metadata_batch(&hashes, &mut *mutate, &mut missing)?;
                Ok(BatchMetadataResult {
                    updated,
                    missing_ids: missing,
                })
            })();
            complete(reply, result);
        }
        Job::MetadataSync { reply } => {
            let result = do_metadata_sync(ctx);
            complete(reply, result);
        }
        Job::Palette { hash, palette } => {
            // 调色板回写聚合批量处理（flush 时统一落盘），全库重提炼时避免 N 次单条事务 + 事件洪峰。
            // 语义不变：提炼结果(内容的纯函数)入元数据 TOML;meta 已随漂移/删除消失时丢弃;
            // 空数组是负缓存(已提炼无有效像素),同样持久化。幂等,重复应用无害
            stage_palette(ctx, hash, palette);
        }
        Job::PaletteFlush => {
            // 冲刷定时任务到期：队列安静期（无新 job）也能把暂存回写落盘，不依赖任务到达
            ctx.palette_timer.store(false, Ordering::SeqCst);
            flush_palette_batch(ctx);
        }
        Job::FolderHint { reason } => {
            ctx.bus
                .publish(ItemEvents::FOLDER_CHANGED, folder_changed_payload(&reason));
        }
        Job::CategoryCreate { name, reply } => {
            let result = (|| {
                ctx.migrator.register_category(&name);
                Ok(())
            })();
            complete(reply, result);
        }
        Job::CategoryUpdate {
            old_name,
            new_name,
            reply,
        } => {
            let result = ctx.migrator.rename_category(&old_name, &new_name);
            complete(reply, result);
        }
        Job::CategoryDelete { name, reply } => {
            let result = ctx.migrator.delete_category(&name);
            complete(reply, result);
        }
        Job::TagCreate { name, reply } => {
            let result = (|| {
                ctx.migrator.register_tag(&name);
                Ok(())
            })();
            complete(reply, result);
        }
        Job::TagUpdate {
            name,
            new_name,
            reply,
        } => {
            let result = ctx.migrator.rename_tag(&name, &new_name);
            complete(reply, result);
        }
        Job::TagDelete { name, reply } => {
            let result = ctx.migrator.delete_tag(&name);
            complete(reply, result);
        }
        Job::RegistryReload => {
            ctx.migrator.reload_registries();
        }
    }
}

// ---------- 调色板批量回写 ---------

/// 暂存调色板提炼结果（同 hash 以最新为准），达批大小立即冲刷；
/// 未达批大小时 spawn 一次性定时任务（PALETTE_FLUSH_AFTER 后入队 PaletteFlushJob 冲刷）——
/// 安静期（无后续 job）也能落盘，不依赖消费循环的任务到达
fn stage_palette(ctx: &Arc<PipelineCtx>, hash: String, palette: Vec<PaletteColor>) {
    let mut pending = ctx.palette_pending.lock().unwrap();
    if let Some(entry) = pending.iter_mut().find(|(h, _)| *h == hash) {
        entry.1 = palette;
        return;
    }
    if pending.is_empty() {
        *ctx.palette_oldest.lock().unwrap() = Some(std::time::Instant::now());
    }
    pending.push((hash, palette));
    let full = pending.len() >= PALETTE_BATCH;
    drop(pending);
    if full {
        flush_palette_batch(ctx);
        return;
    }
    if !ctx.palette_timer.swap(true, Ordering::SeqCst) {
        let ctx2 = ctx.clone();
        let runtime = ctx.runtime.clone();
        runtime.spawn(async move {
            tokio::time::sleep(PALETTE_FLUSH_AFTER).await;
            // 到点仍由消费循环执行冲刷（单写者）；定时任务只负责唤醒
            fire(&ctx2, Job::PaletteFlush);
        });
    }
}

/// 消费循环每处理完一个 job 检查一次：滞留超时即使未达批大小也冲刷（事件平滑发出）
fn maybe_flush_palette(ctx: &PipelineCtx) {
    let oldest = *ctx.palette_oldest.lock().unwrap();
    if let Some(t) = oldest {
        if t.elapsed() >= PALETTE_FLUSH_AFTER {
            flush_palette_batch(ctx);
        }
    }
}

/// 冲刷暂存的调色板回写：逐条落 TOML（铁律：权威层先行），随后内存副本与 SQLite 单事务统一应用，
/// 最后逐 item 同步索引并补发 item.updated。meta 已随漂移/删除消失时丢弃
fn flush_palette_batch(ctx: &PipelineCtx) {
    let batch: Vec<(String, Vec<PaletteColor>)> = {
        let mut pending = ctx.palette_pending.lock().unwrap();
        if pending.is_empty() {
            return;
        }
        *ctx.palette_oldest.lock().unwrap() = None;
        std::mem::take(&mut *pending)
    };

    let mut applied: Vec<(String, ItemMetadata, i64)> = Vec::with_capacity(batch.len());
    let batch_len = batch.len();
    for (hash, palette) in batch {
        let Some(mut meta) = ctx.store.try_get(&hash) else {
            continue;
        };
        meta.palette = Some(
            palette
                .iter()
                .map(|p| PaletteEntry {
                    color: color_math::to_hex(p.r, p.g, p.b),
                    percentage: p.percentage,
                })
                .collect(),
        );
        meta.palette_version = color::PALETTE_VERSION;
        let Ok(source_mtime) = ctx.store.save_toml(&hash, &meta) else {
            continue;
        };
        applied.push((hash, meta, source_mtime));
    }
    if applied.is_empty() {
        return;
    }

    ctx.store.apply_batch(&applied);
    tracing::info!("调色板批量冲刷 {} 条（暂存 {} 条）", applied.len(), batch_len);

    for (hash, meta, _) in &applied {
        if ctx.index.contains(hash) {
            ctx.index.with_item_mut(hash, |item| item.sync_from(meta));
            if let Some(dto) = ctx.index.get_dto(hash) {
                ItemEvents::publish_changed(&ctx.bus, &dto);
            }
        }
    }
}

/// 索引进度节流推送(500ms 一帧);刚从非空闲转空闲时补发一帧清零,客户端据此撤掉进度指示
fn publish_index_progress(ctx: &PipelineCtx, force: bool) {
    let progress = {
        let (queued, deferred, scanning) = (
            ctx.queued_jobs.load(Ordering::SeqCst),
            ctx.deferred.lock().unwrap().len() as i32,
            ctx.scanning.load(Ordering::SeqCst),
        );
        let scan = ctx.last_scan.lock().unwrap().clone();
        let in_scan = scanning && scan.as_ref().map(|s| s.phase != "done").unwrap_or(false);
        let scan = scan.filter(|_| in_scan);
        TaskProgress {
            task: "index",
            pending: queued + deferred,
            active: if scanning { 1 } else { 0 },
            phase: scan.as_ref().map(|s| s.phase.clone()),
            processed: scan.as_ref().map(|s| s.processed),
            total: scan.as_ref().map(|s| s.total),
        }
    };
    let idle = progress.pending == 0 && progress.active == 0;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let last = ctx.progress_last_at.load(Ordering::SeqCst);
    let due = now - last >= 500;
    if !force && !due && !(idle && !ctx.progress_idle.load(Ordering::SeqCst)) {
        return;
    }
    if ctx
        .progress_last_at
        .compare_exchange(last, now, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    ctx.progress_idle.store(idle, Ordering::SeqCst);
    ctx.bus
        .publish(ItemEvents::TASK_PROGRESS, serde_json::to_value(progress).unwrap());
}

// ---------- 单文件入库 ----------

fn do_upsert(
    ctx: &Arc<PipelineCtx>,
    abs: &str,
    force_hash: bool,
    known_hash: Option<&str>,
    attempt: u32,
    has_reply: bool,
) -> Result<Option<UpsertResult>, String> {
    // 携带已知哈希(item/add)或等待结果的提交不做防抖:文件由 API 写入,内容已完整
    let allow_defer = known_hash.is_none() && !has_reply;
    let Some(pending) = prepare_upsert(ctx, abs, force_hash, allow_defer, attempt) else {
        return Ok(None);
    };

    let hash = match known_hash {
        Some(h) => Some(h.to_string()),
        None if pending.reused_hash.is_some() => pending.reused_hash.clone(),
        _ => try_compute_hash(ctx, &pending.abs_path),
    };
    let Some(hash) = hash else {
        return Ok(None);
    };

    // 复验:哈希计算期间文件被继续写入(慢速来源常见)时,不得以半截内容入库,
    // 延迟重试直至写入稳定;API 提交(knownHash)信任调用方,维持原语义
    if known_hash.is_none() && pending.reused_hash.is_none() && file_changed_since_prepare(&pending) {
        if allow_defer && attempt < MAX_DEBOUNCE_ATTEMPTS {
            defer_upsert(ctx, pending.abs_path.clone(), attempt);
            return Ok(None);
        }
        tracing::warn!("文件哈希后仍在变化,按现状入库(后续事件自愈): {abs}");
    }

    Ok(Some(apply_upsert(ctx, pending, &hash)?))
}

/// 哈希前 stat(size/mtime)与现状是否一致。不一致(含文件消失、stat 失败)视为仍在写入;
/// 无副作用——文件消失的清理由删除事件/对账扫描兜底
fn file_changed_since_prepare(p: &PendingUpsert) -> bool {
    match std::fs::metadata(&p.abs_path) {
        Ok(m) => {
            let mtime = m.modified().map(unix_ms).unwrap_or(0);
            m.len() as i64 != p.size || mtime != p.mtime
        }
        Err(_) => true,
    }
}

/// 入库准备:路径过滤、文件状态读取、哈希复用判定、写入中文件防抖。
/// 返回 None 表示已处理(跳过/按删除处理/延迟重试)。不读文件内容
fn prepare_upsert(
    ctx: &Arc<PipelineCtx>,
    abs: &str,
    force_hash: bool,
    allow_defer: bool,
    attempt: u32,
) -> Option<PendingUpsert> {
    let rel = ctx.paths.to_relative(abs)?;
    if LibraryPaths::is_internal(&rel) {
        return None;
    }

    let in_trash = LibraryPaths::is_in_trash(&rel);
    if !in_trash && ctx.config.is_ignored(&rel) {
        do_delete(ctx, &rel);
        return None;
    }

    let meta = std::fs::metadata(abs).ok();
    let Some(file_meta) = meta.filter(|m| m.is_file()) else {
        do_delete(ctx, &rel);
        return None;
    };
    let size = file_meta.len() as i64;
    let mtime = file_meta.modified().map(unix_ms).unwrap_or(0);

    let lib_path = if in_trash {
        LibraryPaths::trash_to_library_path(&rel).to_string()
    } else {
        rel.clone()
    };

    // 路径与 size/mtime 均与元数据一致 → 复用哈希(元数据文件名即哈希),不读文件内容
    let old_hash = ctx
        .index
        .hash_by_location(&rel)
        .or_else(|| ctx.store.find_hash_by_path(&lib_path));
    let reuse = !force_hash
        && old_hash.as_ref().is_some_and(|h| {
            ctx.store
                .try_get(h)
                .and_then(|m| m.find_path(&lib_path).cloned())
                .map(|e| e.size == size && e.modification_time == mtime)
                .unwrap_or(false)
        });

    if reuse {
        return Some(PendingUpsert {
            abs_path: abs.to_string(),
            rel,
            lib_path,
            size,
            mtime,
            old_hash: old_hash.clone(),
            reused_hash: old_hash,
            hash: None,
            dim: None,
        });
    }

    // 文件可能仍在写入(如大文件拷贝中):不立即哈希,延迟重试直至写入稳定,
    // 避免对半截内容反复算哈希。超出重试上限后按现状处理(后续事件/扫描会自愈)
    if allow_defer && attempt < MAX_DEBOUNCE_ATTEMPTS && is_unstable(mtime) {
        defer_upsert(ctx, abs.to_string(), attempt);
        return None;
    }

    Some(PendingUpsert {
        abs_path: abs.to_string(),
        rel,
        lib_path,
        size,
        mtime,
        old_hash,
        reused_hash: None,
        hash: None,
        dim: None,
    })
}

/// 文件最近一秒内仍在写入,视为不稳定
fn is_unstable(mtime: i64) -> bool {
    let now = unix_ms(std::time::SystemTime::now());
    now - mtime < STABILITY_WINDOW_MS
}

/// 延迟重试:同一路径只保留一个延迟任务,避免监听事件风暴放大
fn defer_upsert(ctx: &Arc<PipelineCtx>, abs: String, attempt: u32) {
    {
        let mut deferred = ctx.deferred.lock().unwrap();
        if !deferred.insert(abs.clone()) {
            return;
        }
    }
    let ctx = ctx.clone();
    let runtime = ctx.runtime.clone();
    runtime.spawn(async move {
        tokio::time::sleep(Duration::from_millis(STABILITY_WINDOW_MS as u64)).await;
        ctx.deferred.lock().unwrap().remove(&abs);
        fire(&ctx, Job::Upsert {
            abs,
            force_hash: false,
            known_hash: None,
            reply: None,
            attempt: attempt + 1,
        });
    });
}

/// 计算内容哈希;读不了(权限/占用)时告警并返回 None
fn try_compute_hash(_ctx: &PipelineCtx, abs: &str) -> Option<String> {
    match content_hash::hash_file(abs) {
        Ok(h) => Some(h),
        Err(e) => {
            tracing::warn!("计算哈希失败: {abs}: {e}");
            None
        }
    }
}

/// 应用入库结果:元数据迁移与回写、索引更新、事件、缩略图派发。只允许串行调用
fn apply_upsert(ctx: &Arc<PipelineCtx>, pending: PendingUpsert, hash: &str) -> Result<UpsertResult, String> {
    // 内容变动导致哈希漂移 → 按路径迁移元数据,旧 item 摘掉该位置。
    // 注意先取旧元数据用于继承:迁移可能将旧元数据删除(无剩余位置时)
    let inherit_from = if pending.old_hash.as_deref().is_some_and(|h| h != hash) {
        pending.old_hash.as_ref().and_then(|h| ctx.store.try_get(h))
    } else {
        None
    };

    if pending.old_hash.as_deref().is_some_and(|h| h != hash) {
        ctx.index.remove_location(&pending.rel);
        migrate_metadata(ctx, pending.old_hash.as_ref().unwrap(), &pending.lib_path)?;
        ItemEvents::publish_location_loss(&ctx.bus, &ctx.index, pending.old_hash.as_ref().unwrap());
    }

    // 元数据登记路径并回写最新 size/mtime,保持哈希校验依据新鲜
    let mut meta = get_or_create_metadata(ctx, hash, inherit_from.as_ref());
    let mut meta_changed = false;
    match meta.find_path_mut(&pending.lib_path) {
        None => {
            meta.paths.push(PathEntry {
                path: pending.lib_path.clone(),
                size: pending.size,
                modification_time: pending.mtime,
            });
            meta_changed = true;
        }
        Some(entry) => {
            if entry.size != pending.size || entry.modification_time != pending.mtime {
                entry.size = pending.size;
                entry.modification_time = pending.mtime;
                meta_changed = true;
            }
        }
    }
    if meta_changed {
        ctx.store.save(hash, &meta)?;
    }
    ctx.migrator.register_taxonomy(&meta);

    // 索引更新;尺寸为派生信息,索引时从文件读取(扫描路径已在并行哈希阶段预取)
    let created = ctx.index.get_or_add(hash);
    ctx.index.with_item_mut(hash, |item| item.sync_from(&meta));

    // 宽高持久化入 TOML（C# 仅在内存更新；此处按 storage.md 的设计意图落盘）
    let mut dim_persisted = false;
    let needs_dim = ctx.index.with_item_mut(hash, |item| item.width == 0).unwrap_or(false);
    if needs_dim {
        let dim = pending.dim.or_else(|| ThumbnailService::identify(&pending.abs_path));
        if let Some((w, h)) = dim {
            ctx.index.with_item_mut(hash, |item| {
                item.width = w;
                item.height = h;
            });
            meta.width = w;
            meta.height = h;
            ctx.store.save(hash, &meta)?;
            dim_persisted = true;
        }
    }

    let added_location = ctx
        .index
        .add_or_update_location(hash, &pending.rel, pending.size, pending.mtime);

    if created {
        if let Some(dto) = ctx.index.get_dto(hash) {
            ctx.bus.publish(ItemEvents::ADDED, serde_json::to_value(&dto).unwrap());
        }
    } else if added_location || meta_changed || dim_persisted {
        if let Some(dto) = ctx.index.get_dto(hash) {
            ItemEvents::publish_changed(&ctx.bus, &dto);
        }
    }

    // 缩略图/调色板齐备的文件(如对账扫描重放)不再派发:no-op 任务会把队列与积压计数灌满失真
    if needs_thumbnail_work(ctx, hash) {
        ctx.worker.enqueue(hash, &pending.abs_path);
    }

    let dto = ctx
        .index
        .get_dto(hash)
        .expect("入库后索引必含该 item");
    Ok(UpsertResult {
        item: dto,
        already_existed: !created,
    })
}

/// 缩略图任一配置尺寸或调色板缺失时才需要后台生成(调色板存在性查元数据)
fn needs_thumbnail_work(ctx: &PipelineCtx, hash: &str) -> bool {
    let palette_missing = match ctx.store.try_get(hash) {
        Some(meta) => meta.palette.is_none() || meta.palette_version != PALETTE_VERSION,
        None => true,
    };
    palette_missing
        || ctx
            .config
            .current()
            .thumbnail_sizes
            .iter()
            .any(|s| !ctx.thumbs.exists(hash, *s))
}

/// 哈希漂移时按路径迁移:路径从旧元数据移除;旧元数据不再有位置且索引无引用时清理
fn migrate_metadata(ctx: &PipelineCtx, old_hash: &str, lib_path: &str) -> Result<(), String> {
    let Some(mut old_meta) = ctx.store.try_get(old_hash) else {
        return Ok(());
    };
    old_meta.paths.retain(|p| p.path != lib_path);
    let old_item_exists = ctx.index.contains(old_hash);
    if old_meta.paths.is_empty() && !old_item_exists {
        ctx.store.delete(old_hash);
        ctx.thumbs.delete(old_hash, &ctx.config.current().thumbnail_sizes);
    } else {
        ctx.store.save(old_hash, &old_meta)?;
    }
    Ok(())
}

/// 取得元数据;不存在时新建,可从旧元数据继承素材参数(id 漂移场景)
fn get_or_create_metadata(
    ctx: &PipelineCtx,
    hash: &str,
    inherit_from: Option<&ItemMetadata>,
) -> ItemMetadata {
    if let Some(meta) = ctx.store.try_get(hash) {
        return meta;
    }
    let mut created = ItemMetadata::default();
    if let Some(from) = inherit_from {
        created.url = from.url.clone();
        created.tags = from.tags.clone();
        created.categories = from.categories.clone();
        created.star = from.star;
        created.annotation = from.annotation.clone();
    }
    created
}

// ---------- 删除 / 移动 ----------

/// 按相对路径删除:同时按文件(精确)与目录(前缀)匹配,删除事件不区分两者
fn do_delete(ctx: &PipelineCtx, rel: &str) {
    // 目录(或其下的文件)删除:前缀范围内的 folder: 排序偏好一并清除。
    // 同目录下文件与文件夹不可同名,按前缀匹配不会误伤文件夹设置
    ctx.prefs.delete_prefix(rel);

    if let Some(hash) = ctx.index.remove_location(rel) {
        ItemEvents::publish_location_loss(&ctx.bus, &ctx.index, &hash);
    }

    for loc in ctx.index.locations_under(&format!("{rel}/")) {
        if let Some(hash) = ctx.index.remove_location(&loc) {
            ItemEvents::publish_location_loss(&ctx.bus, &ctx.index, &hash);
        }
    }
}

fn do_move(ctx: &Arc<PipelineCtx>, old_abs: &str, new_abs: &str) -> Result<(), String> {
    let Some(old_rel) = ctx.paths.to_relative(old_abs) else {
        return Ok(());
    };
    let new_rel = ctx.paths.to_relative(new_abs);
    let new_usable = new_rel.as_ref().is_some_and(|r| {
        !LibraryPaths::is_internal(r)
            && (LibraryPaths::is_in_trash(r) || !ctx.config.is_ignored(r))
    });
    let Some(new_rel) = new_rel.filter(|_| new_usable) else {
        do_delete(ctx, &old_rel);
        return Ok(());
    };

    let Some(hash) = move_one(ctx, &old_rel, &new_rel)? else {
        // 旧位置未索引(例如改名发生在未索引文件上)→ 按新文件处理
        return do_upsert(ctx, new_abs, false, None, 0, false).map(|_| ());
    };

    ItemEvents::publish_transition(
        &ctx.bus,
        &ctx.index,
        &hash,
        LibraryPaths::is_in_trash(&old_rel),
        LibraryPaths::is_in_trash(&new_rel),
    );
    Ok(())
}

fn do_dir_move(ctx: &Arc<PipelineCtx>, old_abs: &str, new_abs: &str) -> Result<(), String> {
    let Some(old_rel) = ctx.paths.to_relative(old_abs) else {
        return Ok(());
    };
    let new_rel = ctx.paths.to_relative(new_abs);
    let new_usable = new_rel.as_ref().is_some_and(|r| {
        !LibraryPaths::is_internal(r)
            && (LibraryPaths::is_in_trash(&format!("{r}/")) || !ctx.config.is_ignored(r))
    });
    let Some(new_rel) = new_rel.filter(|_| new_usable) else {
        do_delete(ctx, &old_rel);
        return Ok(());
    };

    let mut affected: HashSet<String> = HashSet::new();
    for loc_path in ctx.index.locations_under(&format!("{old_rel}/")) {
        let new_loc_path = format!("{new_rel}{}", &loc_path[old_rel.len()..]);
        if let Some(hash) = move_one(ctx, &loc_path, &new_loc_path)? {
            affected.insert(hash);
        }
    }

    // 排序偏好跟随目录移动/重命名(含移入回收站,恢复时随之回归)
    ctx.prefs.rename_prefix(&old_rel, &new_rel);

    let old_in_trash = LibraryPaths::is_in_trash(&format!("{old_rel}/"));
    let new_in_trash = LibraryPaths::is_in_trash(&format!("{new_rel}/"));
    for hash in &affected {
        ItemEvents::publish_transition(&ctx.bus, &ctx.index, hash, old_in_trash, new_in_trash);
    }

    // 目录移动后目录结构必然变化,广播 folder.changed(folder/list 全量建树,客户端重拉即可)
    ctx.bus
        .publish(ItemEvents::FOLDER_CHANGED, folder_changed_payload(REASON_EXTERNAL));

    // 目录下可能有监听遗漏的文件,补扫新位置
    if std::path::Path::new(new_abs).is_dir() {
        for file in ctx.scanner.walk_directory(new_abs) {
            do_upsert(ctx, &file, false, None, 0, false)?;
        }
    }
    Ok(())
}

/// 单个位置的移动:更新索引与元数据路径。返回是否命中已索引位置
fn move_one(ctx: &PipelineCtx, old_rel: &str, new_rel: &str) -> Result<Option<String>, String> {
    let Some(hash) = ctx.index.move_location(old_rel, new_rel) else {
        return Ok(None);
    };

    // lib→lib:元数据路径跟随;lib↔trash:去前缀后库内路径不变,元数据保持原路径(恢复目标)
    let old_lib = if LibraryPaths::is_in_trash(old_rel) {
        LibraryPaths::trash_to_library_path(old_rel)
    } else {
        old_rel
    };
    let new_lib = if LibraryPaths::is_in_trash(new_rel) {
        LibraryPaths::trash_to_library_path(new_rel)
    } else {
        new_rel
    };
    if old_lib != new_lib {
        if let Some(mut meta) = ctx.store.try_get(&hash) {
            if let Some(entry) = meta.find_path_mut(old_lib) {
                entry.path = new_lib.to_string();
                ctx.store.save(&hash, &meta)?;
            }
        }
    }
    Ok(Some(hash))
}

// ---------- 扫描 ----------

/// 进度上报:按 150ms 节流,阶段切换/总数变化时强制发一帧。内部状态线程安全(并行哈希阶段多线程上报)
struct ScanReporter {
    ctx: Arc<PipelineCtx>,
    state: Mutex<ScanReporterState>,
}

struct ScanReporterState {
    last_at: std::time::Instant,
    phase: String,
    total: i32,
}

impl ScanReporter {
    fn new(ctx: Arc<PipelineCtx>) -> ScanReporter {
        ScanReporter {
            ctx,
            state: Mutex::new(ScanReporterState {
                last_at: std::time::Instant::now() - Duration::from_secs(60),
                phase: String::new(),
                total: -1,
            }),
        }
    }

    fn report(&self, phase: &str, processed: i32, total: i32, force: bool) {
        let mut state = self.state.lock().unwrap();
        let now = std::time::Instant::now();
        if !force
            && phase == state.phase
            && total == state.total
            && now.duration_since(state.last_at) < Duration::from_millis(150)
        {
            return;
        }
        state.last_at = now;
        state.phase = phase.to_string();
        state.total = total;
        let progress = ScanProgress {
            phase: phase.to_string(),
            processed,
            total,
        };
        *self.ctx.last_scan.lock().unwrap() = Some(progress.clone());
        self.ctx
            .startup
            .report(&progress.phase, progress.processed, progress.total);
        publish_index_progress(&self.ctx, false);
    }
}

/// 全量扫描分两阶段:串行遍历做复用判定(不读文件内容),需要哈希的文件并行计算,
/// 最后串行应用索引/元数据变更——并行仅限纯计算阶段,单写者模型不变。
/// full=true 时对所有文件重算哈希(library/reindex)。
/// force_walk=用户手动刷新:忽略快照强制遍历全部文件(仍按 size/mtime 复用哈希,不读内容)
fn do_scan(ctx: &Arc<PipelineCtx>, full: bool, force_walk: bool) -> Result<(), String> {
    ctx.scanning.store(true, Ordering::SeqCst);
    let result = do_scan_core(ctx, full, force_walk);
    ctx.scanning.store(false, Ordering::SeqCst);
    // 扫描结束强制发一帧(扫描期间的进度由 reporter 节流推送),客户端据此撤掉进度指示
    publish_index_progress(ctx, true);
    result
}

fn do_scan_core(ctx: &Arc<PipelineCtx>, full: bool, force_walk: bool) -> Result<(), String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut pending: Vec<PendingUpsert> = Vec::new();
    let mut count = 0i32;
    let walk_incomplete = AtomicBool::new(false);
    let reporter = ScanReporter::new(ctx.clone());

    // 阶段一:目录快照对比。遍历目录取 (mtime, 直接子项数),
    // 与上轮快照一致 = 无增删重命名 → 跳过整个目录的文件级访问;
    // 首轮快照为空或强制遍历(手动刷新) = 全部深入
    let snapshots: HashMap<String, (i64, i64)> = if full || force_walk {
        HashMap::new()
    } else {
        ctx.store.load_folder_snapshots()
    };
    let mut dir_stats: HashMap<String, (i64, i64)> = HashMap::new();
    let mut seen_dirs: HashSet<String> = HashSet::new();
    let mut dirty_dirs: Vec<String> = Vec::new();
    reporter.report("scan", 0, 0, true);
    for (rel, mtime, entries) in ctx.scanner.walk_directory_stats(&walk_incomplete) {
        seen_dirs.insert(rel.clone());
        dir_stats.insert(rel.clone(), (mtime, entries));
        if snapshots.get(&rel).map(|s| *s != (mtime, entries)).unwrap_or(true) {
            dirty_dirs.push(rel);
        }
        reporter.report("scan", seen_dirs.len() as i32, 0, false);
    }

    // 阶段二:只深入有变化的目录,枚举直接文件做复用判定/哈希(clean 目录不碰文件系统)
    for rel_dir in &dirty_dirs {
        let abs_dir = if rel_dir.is_empty() {
            ctx.paths.root.clone()
        } else {
            match ctx.paths.to_absolute(rel_dir) {
                Some(a) => a,
                None => continue,
            }
        };
        for abs in ctx.scanner.walk_files_in_directory(&abs_dir) {
            if let Some(rel) = ctx.paths.to_relative(&abs) {
                seen.insert(rel);
            }
            let Some(prepared) = prepare_upsert(ctx, &abs, full, true, 0) else {
                continue;
            };
            count += 1;
            if prepared.reused_hash.is_some() {
                let hash = prepared.reused_hash.clone().unwrap();
                apply_upsert(ctx, prepared, &hash)?;
            } else {
                pending.push(prepared);
            }
        }
    }

    if !pending.is_empty() {
        reporter.report("hash", 0, pending.len() as i32, true);
        // 阶段三:并行哈希(纯计算阶段,索引/元数据应用仍串行);
        // 图像头部解析与哈希同属只读阶段,一并并行
        let pending_total = pending.len() as i32;
        // 哈希是纯计算阶段：留 1 核给 API，其余吃满（桌面端大批量入库/重建索引时吞吐优先）
        let parallelism = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .saturating_sub(1)
            .clamp(2, 24)
            .min(pending.len());
        let hashed = AtomicUsize::new(0);
        let chunk_size = pending.len().div_ceil(parallelism);
        let chunks: Vec<&mut [PendingUpsert]> = pending.chunks_mut(chunk_size).collect();
        std::thread::scope(|s| {
            for chunk in chunks {
                s.spawn(|| {
                    for p in chunk {
                        if let Some(hash) = try_compute_hash(ctx, &p.abs_path) {
                            p.dim = ThumbnailService::identify(&p.abs_path);
                            // 复验:哈希期间仍在写入的文件不半截入库,延迟重试(与 do_upsert 同一纪律)
                            if !file_changed_since_prepare(p) {
                                p.hash = Some(hash);
                            } else if std::path::Path::new(&p.abs_path).exists() {
                                defer_upsert(ctx, p.abs_path.clone(), 0);
                            }
                        }
                        reporter.report(
                            "hash",
                            hashed.fetch_add(1, Ordering::SeqCst) as i32 + 1,
                            pending_total,
                            false,
                        );
                    }
                });
            }
        });

        reporter.report("apply", 0, pending.len() as i32, true);
        let mut applied = 0i32;
        for p in &pending {
            if let Some(hash) = p.hash.clone() {
                apply_upsert(ctx, clone_pending(p), &hash)?;
            }
            applied += 1;
            reporter.report("apply", applied, pending.len() as i32, false);
        }
    }

    reporter.report("done", count, count, true);

    if walk_incomplete.load(Ordering::SeqCst) {
        // 遍历不完整(部分目录枚举失败)时 seenDirs 不可信:本轮跳过消失对账与快照替换,
        // 避免误删已索引位置或写入残缺快照;最终一致由下一轮对账保证
        tracing::warn!("扫描遍历不完整(目录枚举失败),跳过本轮消失对账与快照更新");
    } else {
        // 消失对账:
        // - 所在目录已不存在(目录树遍历不到)→ 位置必然消失
        // - 所在目录本轮深入过却没在枚举中见到 → 已消失
        // - clean 目录快照与磁盘一致(无增删)→ 位置必然还在,不访问文件系统
        let dirty_set: HashSet<&str> = dirty_dirs.iter().map(String::as_str).collect();
        for rel in ctx.index.all_location_paths() {
            if seen.contains(&rel) {
                continue;
            }
            let dir = LibraryPaths::dir_of(&rel);
            if !seen_dirs.contains(dir) || dirty_set.contains(dir) {
                do_delete(ctx, &rel);
            }
        }

        // 快照整体替换为本轮统计(下轮增量的对比基准)
        ctx.store.replace_folder_snapshots(&dir_stats);
    }

    // 对账扫描是目录结构变化的兜底(外部删空目录等不会产生任何事件),广播一次 folder.changed
    ctx.bus
        .publish(ItemEvents::FOLDER_CHANGED, folder_changed_payload(REASON_EXTERNAL));

    tracing::info!(
        "扫描完成:{} 个文件({} 个计算哈希,{} 个目录中 {} 个深入),{} 个索引位置",
        count,
        pending.len(),
        seen_dirs.len(),
        dirty_dirs.len(),
        ctx.index.all_location_paths().len()
    );
    Ok(())
}

// apply 阶段的辅助：克隆 PendingUpsert（apply 需要所有权，扫描循环里用引用）
fn clone_pending(p: &PendingUpsert) -> PendingUpsert {
    PendingUpsert {
        abs_path: p.abs_path.clone(),
        rel: p.rel.clone(),
        lib_path: p.lib_path.clone(),
        size: p.size,
        mtime: p.mtime,
        old_hash: p.old_hash.clone(),
        reused_hash: p.reused_hash.clone(),
        hash: p.hash.clone(),
        dim: p.dim,
    }
}

// ---------- 元数据对账（只进不出：TOML → 缓存/索引） ----------

/// 周期对账：.hawk/metadata/ 的 TOML 是唯一权威源（参与网盘同步），本机 SQLite 缓存与
/// 内存副本经此跟随外部变更（网盘同步落地、手工编辑）。按 mtime 与缓存记录比对，
/// 只有变化的文件才重新解析。解析失败的文件跳过且不清空状态，下轮重试
fn do_metadata_sync(ctx: &PipelineCtx) -> Result<(), String> {
    let Some(mtimes) = ctx.store.source_mtimes() else {
        return Ok(()); // 缓存不可用：跳过本轮（退化为重启才收敛）
    };

    let mut seen: HashSet<String> = HashSet::new();
    let mut synced = 0i32;
    let read_dir = match std::fs::read_dir(&ctx.paths.metadata_dir) {
        Ok(d) => d,
        Err(_) => return Ok(()),
    };
    for entry in read_dir.flatten() {
        let file_name = entry.file_name().to_string_lossy().to_string();
        if !file_name.ends_with(".toml") {
            continue;
        }
        let hash = file_name.strip_suffix(".toml").unwrap();
        if !crate::core::metadata::is_valid_hash_file_name(hash) {
            continue;
        }
        seen.insert(hash.to_string());
        let file = entry.path().to_string_lossy().to_string();
        let mtime = match std::fs::metadata(&file).and_then(|m| m.modified()) {
            Ok(t) => unix_ms(t),
            Err(e) => {
                tracing::warn!("元数据 mtime 读取失败，跳过: {file}: {e}");
                continue;
            }
        };
        if mtimes.get(hash).map(|known| *known == mtime).unwrap_or(false) {
            continue;
        }
        if ctx.store.apply_external_toml(hash, &file, mtime) {
            sync_index_from_metadata(ctx, hash);
        }
        synced += 1;
        if synced % 100 == 0 {
            // 对账可能持续很久（大库 TOML 解析），期间扫描尚未开始、进度帧会断流：
            // 按 100 文件一帧上报（StartupState → /app/startup），启动屏靠它续命
            ctx.startup.report("sync", synced, 0);
        }
    }

    // TOML 已消失：清空素材参数（重启后无元数据的等价语义；item 与位置由扫描决定存续）
    for hash in mtimes.keys() {
        if !seen.contains(hash) {
            ctx.store.clear_external(hash);
            sync_index_from_metadata(ctx, hash);
        }
    }

    // 派生缓存自愈：palette 缺失（= 缩略图/调色板派生工作未完成，如 worker 队列高峰期的丢失项、
    // 解码曾失败的素材）→ 派发 worker 任务补齐（生成缺失尺寸缩略图 + 提炼调色板，完成后补发
    // item.updated，前端据此重建 404 占位）。纯内存扫描 + in-flight 去重，稳态零开销。
    // 源文件已不在的项由 worker 静默跳过（删除对账会收敛位置）
    for hash in ctx.store.hashes_with_missing_palette(color::PALETTE_VERSION) {
        if let Some(abs) = ctx.index.main_source_abs(&hash, &ctx.paths) {
            ctx.worker.enqueue(&hash, &abs);
        }
    }
    Ok(())
}

/// 对账应用后刷新索引查询副本、登记分类/标签并推送 item.updated
fn sync_index_from_metadata(ctx: &PipelineCtx, hash: &str) {
    let Some(meta) = ctx.store.try_get(hash) else {
        return;
    };
    ctx.migrator.register_taxonomy(&meta);
    if ctx.index.contains(hash) {
        ctx.index.with_item_mut(hash, |item| item.sync_from(&meta));
        if let Some(dto) = ctx.index.get_dto(hash) {
            ItemEvents::publish_changed(&ctx.bus, &dto);
        }
    }
}

// ---------- 回收站 ----------

/// 清空回收站:清理位置与对应元数据、缩略图(库内仍有引用的内容除外)。物理删除由 API 层完成
fn do_clear_trash(ctx: &PipelineCtx) -> Result<(), String> {
    // 回收站内的 folder: 排序偏好随清空一并移除
    ctx.prefs
        .delete_prefix(&format!("{}/{}", LibraryPaths::HAWK_DIR_NAME, LibraryPaths::TRASH_DIR_NAME));

    for rel in ctx
        .index
        .all_location_paths()
        .into_iter()
        .filter(|p| LibraryPaths::is_in_trash(p))
        .collect::<Vec<_>>()
    {
        let Some(hash) = ctx.index.remove_location(&rel) else {
            continue;
        };
        let lib_path = LibraryPaths::trash_to_library_path(&rel);
        let item_gone = !ctx.index.contains(&hash);
        if let Some(mut meta) = ctx.store.try_get(&hash) {
            meta.paths.retain(|p| p.path != lib_path);
            if meta.paths.is_empty() && item_gone {
                ctx.store.delete(&hash);
                ctx.thumbs.delete(&hash, &ctx.config.current().thumbnail_sizes);
            } else {
                ctx.store.save(&hash, &meta)?;
            }
        }
        if item_gone {
            ctx.bus.publish(ItemEvents::REMOVED, serde_json::json!({ "id": hash }));
        } else if let Some(dto) = ctx.index.get_dto(&hash) {
            ItemEvents::publish_changed(&ctx.bus, &dto);
        }
    }
    Ok(())
}

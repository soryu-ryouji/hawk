//! 索引流水线（单写者 actor）：监听事件 / 扫描 / API 写操作全部经有界队列串行处理，
//! **索引与元数据的所有变更只发生在消费循环线程**，处理逻辑保证幂等（重复事件无害）。
//!
//! 结构（按职责拆分，消费循环是唯一入口）：
//! - `ctx`：上下文、JobSender（外部线程回流通道）、进度快照
//! - `upsert`：单文件入库生命周期（防抖/哈希/迁移继承/应用）
//! - `fs_ops`：删除/移动/回收站
//! - `scan`：全库扫描（runner 线程做 IO/哈希，结果回流穿插应用，收尾消失对账）
//! - `reconcile`：元数据周期对账（TOML → 缓存/索引）与派生缓存自愈
//! - `derived`：worker 提炼结果的批量回写（调色板/宽高）
//!
//! 扫描不再独占消费线程：Job::ScanStart 只建会话并 spawn runner，文件结果以
//! Job::ScanFile 回流与其他任务穿插应用（交互写延迟有界），Job::ScanEnd 收尾。
//! 入口两类：watcher/worker 火忘（队列满置溢出标记，由兜底扫描收敛）；
//! API/启动提交携带 oneshot 回复并等待（统一超时，消费循环停止/panic 不会挂起调用方）。

mod ctx;
mod derived;
mod fs_ops;
mod reconcile;
mod scan;
mod upsert;

pub use ctx::{BatchMetadataResult, UpsertResult};
pub(crate) use ctx::JobSender;

use crate::core::config::LibraryConfig;
use crate::core::events::{EventBus, TaskProgress};
use crate::core::index::ItemIndex;
use crate::core::item::PaletteColor;
use crate::core::metadata_store::MetadataStore;
use crate::core::metadata::ItemMetadata;
use crate::core::paths::LibraryPaths;
use crate::core::scanner::LibraryScanner;
use crate::core::startup::StartupState;
use crate::core::taxonomy::{ItemEvents, TaxonomyMigrator};
use crate::core::thumbnail::ThumbnailService;
use crate::core::thumbnail_worker::ThumbnailWorker;
use crate::core::view_prefs::ViewPreferences;
use crate::settings::Settings;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

use ctx::{complete, publish_index_progress, PipelineCtx, Reply};

/// 提交等待超时：消费循环被长任务（大批量迁移等）占住时，调用方得到明确错误而非无限挂起。
/// 超时后任务仍可能完成——处理幂等，晚到结果无害。扫描等待不设超时（等整轮完成，后台 await）
const REPLY_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) enum Job {
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
    /// 启动扫描：建会话 + spawn runner 线程。已在扫描中则合并请求
    ScanStart {
        full: bool,
        force_walk: bool,
        reply: Reply<Result<(), String>>,
    },
    /// runner 回流的单文件结果（pending.hash 必为 Some），消费循环穿插应用
    ScanFile {
        pending: upsert::PendingUpsert,
    },
    /// runner 收尾：消失对账/快照替换/尾批冲刷/回复（阻塞入队保证到达）
    ScanEnd {
        session: Arc<scan::ScanSession>,
        walk: scan::WalkOutcome,
        error: Option<String>,
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
    MetadataSync,
    PaletteFlush,
    Palette {
        hash: String,
        palette: Vec<PaletteColor>,
    },
    FixDim {
        hash: String,
        w: i32,
        h: i32,
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

#[derive(Clone)]
pub struct IndexPipeline {
    ctx: Arc<PipelineCtx>,
    rx: Arc<Mutex<Option<std::sync::mpsc::Receiver<Job>>>>,
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
        let queued_jobs = Arc::new(std::sync::atomic::AtomicI32::new(0));
        let overflow = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sender = JobSender::new(tx, queued_jobs.clone(), overflow.clone());
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
            sender,
            overflow,
            scan_scheduled: std::sync::atomic::AtomicBool::new(false),
            scanning: std::sync::atomic::AtomicBool::new(false),
            deferred: Mutex::new(std::collections::HashSet::new()),
            queued_jobs,
            last_scan: Mutex::new(None),
            progress_last_at: std::sync::atomic::AtomicI64::new(0),
            progress_idle: std::sync::atomic::AtomicBool::new(true),
            palette_pending: Mutex::new(Vec::new()),
            palette_oldest: Mutex::new(None),
            palette_timer: std::sync::atomic::AtomicBool::new(false),
            runtime: tokio::runtime::Handle::current(),
            scan_session: Mutex::new(None),
        };
        IndexPipeline {
            ctx: Arc::new(ctx),
            rx: Arc::new(Mutex::new(Some(rx))),
        }
    }

    // ---------- 启动 ----------

    /// 启动消费循环/缩略图 worker/周期对账，并入队一轮元数据对账（先于初始扫描）。
    /// worker 的回流接线由 main 装配（worker.attach(index, store, pipeline.sender())）
    pub fn start(&self) {
        self.hydrate_index();

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
                let mut ticker =
                    tokio::time::interval(Duration::from_secs(ctx.settings.rescan_interval_seconds));
                ticker.tick().await; // 立即 tick 的一次丢弃
                loop {
                    ticker.tick().await;
                    ctx.sender.fire(Job::MetadataSync);
                }
            });
        }

        // 启动先跑一轮元数据对账（入队于初始扫描之前）：把停机期间网盘同步落地的外部
        // TOML 变更并入内存，避免扫描拿旧副本做迁移继承
        self.ctx.sender.fire(Job::MetadataSync);
    }

    /// 启动注水：内存索引由元数据副本恢复（SQLite 快路径/TOML 回退），就绪无需等待全库扫描
    fn hydrate_index(&self) {
        let entries = self.ctx.store.snapshot();
        for (hash, meta) in &entries {
            // 无位置的元数据不进索引：item 的存续由位置决定（否则产生零位置 ghost item）
            if meta.paths.is_empty() {
                continue;
            }
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

    // ---------- 状态快照(task.progress 与 app/status 共用) ----------

    /// 索引进度快照(task.progress("index") 事件与 app/status 端点共用同一构造)
    pub fn index_progress(&self) -> TaskProgress {
        ctx::index_progress_snapshot(&self.ctx)
    }

    /// 队列回流句柄：worker 等外部线程向消费循环派发 FixDim/Palette 任务的通道
    pub fn sender(&self) -> JobSender {
        self.ctx.sender.clone()
    }

    // ---------- 入口:文件监听(火忘,队列满置溢出标记,由消费者全量扫描兜底) ----------

    pub fn notify_upsert(&self, abs: String) {
        self.ctx.sender.fire(Job::Upsert {
            abs,
            force_hash: false,
            known_hash: None,
            reply: None,
            attempt: 0,
        });
    }

    pub fn notify_deleted(&self, abs: String) {
        self.ctx.sender.fire(Job::Delete { abs });
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
        self.ctx.sender.fire(job);
    }

    /// ignore 规则变化影响全库过滤:强制重新遍历
    pub fn notify_config_changed(&self) {
        self.ctx.sender.fire(Job::ScanStart {
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
        self.ctx.sender.fire(Job::ScanStart {
            full: false,
            force_walk: true,
            reply: None,
        });
    }

    /// 注册表文件被外部修改(网盘同步等):重新加载
    pub fn notify_registry_changed(&self) {
        self.ctx.sender.fire(Job::RegistryReload);
    }

    /// 异步触发扫描(library/reindex:立即返回,过程变更照常推送事件)
    pub fn request_scan(&self, full: bool) {
        self.ctx.sender.fire(Job::ScanStart {
            full,
            force_walk: false,
            reply: None,
        });
    }

    /// 目录结构可能变化(文件夹增删改移、外部变动、扫描兜底):广播 folder.changed
    pub fn notify_folder_changed(&self, reason: &str) {
        self.ctx.sender.fire(Job::FolderHint {
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
        if !self.ctx.sender.try_fire(job) {
            return Err("索引队列已满".to_string());
        }
        await_reply(rx).await
    }

    pub async fn submit_move(&self, old_abs: String, new_abs: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        if !self.ctx.sender.try_fire(Job::Move {
            old_abs,
            new_abs,
            reply: Some(tx),
        }) {
            return Err("索引队列已满".to_string());
        }
        await_reply(rx).await
    }

    pub async fn submit_dir_move(&self, old_abs: String, new_abs: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        if !self.ctx.sender.try_fire(Job::DirMove {
            old_abs,
            new_abs,
            reply: Some(tx),
        }) {
            return Err("索引队列已满".to_string());
        }
        await_reply(rx).await
    }

    pub async fn submit_clear_trash(&self) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        if !self.ctx.sender.try_fire(Job::ClearTrash { reply: Some(tx) }) {
            return Err("索引队列已满".to_string());
        }
        await_reply(rx).await
    }

    pub async fn submit_metadata(
        &self,
        hash: String,
        mutate: impl FnOnce(&mut ItemMetadata) + Send + 'static,
    ) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        if !self.ctx.sender.try_fire(Job::Metadata {
            hash,
            mutate: Box::new(mutate),
            reply: Some(tx),
        }) {
            return Err("索引队列已满".to_string());
        }
        await_reply(rx).await
    }

    /// 批量元数据应用(item/batch_update);不存在的 id 记入 missing_ids
    pub async fn submit_batch_metadata(
        &self,
        hashes: Vec<String>,
        mutate: impl FnMut(&mut ItemMetadata) + Send + 'static,
    ) -> Result<BatchMetadataResult, String> {
        let (tx, rx) = oneshot::channel();
        if !self.ctx.sender.try_fire(Job::BatchMetadata {
            hashes,
            mutate: Box::new(mutate),
            reply: Some(tx),
        }) {
            return Err("索引队列已满".to_string());
        }
        await_reply(rx).await
    }

    /// 全库扫描。full=true 时对所有文件重算哈希(library/reindex)。
    /// 回复在扫描收尾（ScanEnd）时完成；不设超时——扫描可持续很久（冷缓存大库），
    /// 调用方（启动流程）在后台 await，期间消费循环照常处理其他任务
    pub async fn run_scan(&self, full: bool) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        if !self.ctx.sender.try_fire(Job::ScanStart {
            full,
            force_walk: false,
            reply: Some(tx),
        }) {
            return Err("索引队列已满".to_string());
        }
        match rx.await {
            Ok(result) => result,
            Err(_) => Err("索引流水线已停止".to_string()),
        }
    }

    pub async fn submit_category_create(&self, name: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        if !self.ctx.sender.try_fire(Job::CategoryCreate {
            name,
            reply: Some(tx),
        }) {
            return Err("索引队列已满".to_string());
        }
        await_reply(rx).await
    }

    pub async fn submit_category_update(&self, old_name: String, new_name: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        if !self.ctx.sender.try_fire(Job::CategoryUpdate {
            old_name,
            new_name,
            reply: Some(tx),
        }) {
            return Err("索引队列已满".to_string());
        }
        await_reply(rx).await
    }

    pub async fn submit_category_delete(&self, name: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        if !self.ctx.sender.try_fire(Job::CategoryDelete {
            name,
            reply: Some(tx),
        }) {
            return Err("索引队列已满".to_string());
        }
        await_reply(rx).await
    }

    pub async fn submit_tag_create(&self, name: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        if !self.ctx.sender.try_fire(Job::TagCreate {
            name,
            reply: Some(tx),
        }) {
            return Err("索引队列已满".to_string());
        }
        await_reply(rx).await
    }

    pub async fn submit_tag_update(&self, name: String, new_name: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        if !self.ctx.sender.try_fire(Job::TagUpdate {
            name,
            new_name,
            reply: Some(tx),
        }) {
            return Err("索引队列已满".to_string());
        }
        await_reply(rx).await
    }

    pub async fn submit_tag_delete(&self, name: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        if !self.ctx.sender.try_fire(Job::TagDelete {
            name,
            reply: Some(tx),
        }) {
            return Err("索引队列已满".to_string());
        }
        await_reply(rx).await
    }
}

/// 等待回复的统一出口：oneshot 断开（消费循环停止/任务 panic 丢弃发送端）或超时返回错误。
/// 超时后任务仍可能完成——处理幂等，晚到结果无害
async fn await_reply<T>(rx: oneshot::Receiver<Result<T, String>>) -> Result<T, String> {
    match tokio::time::timeout(REPLY_TIMEOUT, rx).await {
        // 通道内承载的即是任务结果（Result），拉平；断开/超时返回错误。
        // 超时后任务仍可能完成——处理幂等，晚到结果无害
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("索引流水线已停止".to_string()),
        Err(_) => Err("索引流水线繁忙（等待超时）".to_string()),
    }
}

// ---------- 消费循环 ----------

fn consumer_loop(ctx: Arc<PipelineCtx>, rx: std::sync::mpsc::Receiver<Job>) {
    while let Ok(job) = rx.recv() {
        ctx.queued_jobs.fetch_sub(1, Ordering::SeqCst);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| process_job(&ctx, job)));
        if result.is_err() {
            // 等待中的 API 调用方会因 oneshot 发送端被丢弃而收到 500，不会挂起
            tracing::error!("索引任务处理 panic（已跳过该任务）");
        }

        // 监听事件丢失兜底:不内联扫描(事件风暴期会反复全库扫描),
        // 改为入队去重的扫描任务——扫描本身会把全部待处理文件入库,一次即可收敛
        if ctx.overflow.swap(false, Ordering::SeqCst) && !ctx.scan_scheduled.swap(true, Ordering::SeqCst) {
            tracing::info!("检测到事件丢失,排队对账扫描");
            ctx.sender.fire(Job::ScanStart {
                full: false,
                force_walk: false,
                reply: None,
            });
        }

        derived::maybe_flush_palette(&ctx);
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
            let result = upsert::do_upsert(ctx, &abs, force_hash, known_hash.as_deref(), attempt, reply.is_some());
            complete(reply, result);
        }
        Job::Delete { abs } => {
            if let Some(rel) = ctx.paths.to_relative(&abs) {
                fs_ops::do_delete(ctx, &rel);
            }
        }
        Job::Move {
            old_abs,
            new_abs,
            reply,
        } => {
            let result = fs_ops::do_move(ctx, &old_abs, &new_abs);
            complete(reply, result);
        }
        Job::DirMove {
            old_abs,
            new_abs,
            reply,
        } => {
            let result = fs_ops::do_dir_move(ctx, &old_abs, &new_abs);
            complete(reply, result);
        }
        Job::ScanStart {
            full,
            force_walk,
            reply,
        } => scan::start(ctx, full, force_walk, reply),
        Job::ScanFile { pending } => scan::apply_scan_file(ctx, pending),
        Job::ScanEnd {
            session,
            walk,
            error,
        } => scan::finish(ctx, session, walk, error),
        Job::ClearTrash { reply } => {
            let result = fs_ops::do_clear_trash(ctx);
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
        Job::MetadataSync => reconcile::do_metadata_sync(ctx),
        Job::Palette { hash, palette } => {
            // 调色板回写聚合批量处理（flush 时统一落盘），全库重提炼时避免 N 次单条事务 + 事件洪峰。
            // 语义不变：提炼结果(内容的纯函数)入元数据 TOML;meta 已随漂移/删除消失时丢弃;
            // 空数组是负缓存(已提炼无有效像素),同样持久化。幂等,重复应用无害
            derived::stage_palette(ctx, hash, palette);
        }
        Job::PaletteFlush => {
            // 冲刷定时任务到期：队列安静期（无新 job）也能把暂存回写落盘，不依赖任务到达
            ctx.palette_timer.store(false, Ordering::SeqCst);
            derived::flush_palette_batch(ctx);
        }
        Job::FixDim { hash, w, h } => {
            derived::do_fix_dim(ctx, &hash, w, h);
        }
        Job::FolderHint { reason } => {
            ctx.bus
                .publish(ItemEvents::FOLDER_CHANGED, crate::core::events::folder_changed_payload(&reason));
        }
        Job::CategoryCreate { name, reply } => {
            ctx.migrator.register_category(&name);
            complete(reply, Ok(()));
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
            ctx.migrator.register_tag(&name);
            complete(reply, Ok(()));
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

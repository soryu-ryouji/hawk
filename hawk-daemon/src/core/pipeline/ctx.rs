//! 流水线上下文与任务发送器。
//! PipelineCtx 聚合全部协作者；各处理阶段文件只取所需字段，避免服务定位扩散。
//! JobSender 是队列的轻量句柄——worker / 扫描 runner / 防抖定时器等外部线程经它回流任务
//! （FixDim / Palette / ScanFile ...），写入仍全部收敛到消费循环（单写者不变）。

use crate::core::config::LibraryConfig;
use crate::core::events::{EventBus, TaskProgress};
use crate::core::global_filter::GlobalFilter;
use crate::core::index::ItemIndex;
use crate::core::item::PaletteColor;
use crate::core::metadata_store::MetadataStore;
use crate::core::paths::LibraryPaths;
use crate::core::scanner::LibraryScanner;
use crate::core::startup::StartupState;
use crate::core::taxonomy::TaxonomyMigrator;
use crate::core::thumbnail::ThumbnailService;
use crate::core::thumbnail_worker::ThumbnailWorker;
use crate::core::view_prefs::ViewPreferences;
use crate::settings::Settings;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

use super::scan::ScanSession;
use super::Job;

/// item/add 的处理结果:索引后的 item 投影与「内容是否已存在」标志
pub struct UpsertResult {
    pub item: crate::core::item::ItemDto,
    /// 内容是否已存在；item/add 以「写入前」预计算为准，此字段供其他调用方使用
    #[allow(dead_code)]
    pub already_existed: bool,
}

/// 扫描进度快照（task.progress("index") 事件与 app/status 端点共用）
#[derive(Clone, Debug)]
pub struct ScanProgress {
    pub phase: String,
    pub processed: i32,
    pub total: i32,
}

/// 批量元数据应用结果（item/batch_update）：实际更新数与不存在的 id
#[derive(Debug)]
pub struct BatchMetadataResult {
    pub updated: usize,
    pub missing_ids: Vec<String>,
}

pub(crate) type Reply<T> = Option<oneshot::Sender<T>>;

/// 任务队列句柄：外部线程向消费循环回流的唯一通道。
/// try_fire 失败置溢出标记，由消费循环的兜底扫描收敛（火忘任务幂等，丢弃无害）
#[derive(Clone)]
pub(crate) struct JobSender {
    tx: std::sync::mpsc::SyncSender<Job>,
    queued_jobs: Arc<AtomicI32>,
    overflow: Arc<AtomicBool>,
}

impl JobSender {
    pub(crate) fn new(
        tx: std::sync::mpsc::SyncSender<Job>,
        queued_jobs: Arc<AtomicI32>,
        overflow: Arc<AtomicBool>,
    ) -> JobSender {
        JobSender { tx, queued_jobs, overflow }
    }

    /// 尽力入队；成功返回 true，队列满/已断开置溢出标记并返回 false
    pub(crate) fn try_fire(&self, job: Job) -> bool {
        match self.tx.try_send(job) {
            Ok(()) => {
                self.queued_jobs.fetch_add(1, Ordering::SeqCst);
                true
            }
            Err(_) => {
                self.overflow.store(true, Ordering::SeqCst);
                false
            }
        }
    }

    /// 尽力入队，失败由溢出标记兜底（幂等任务）
    pub(crate) fn fire(&self, job: Job) {
        let _ = self.try_fire(job);
    }

    /// 阻塞入队（仅扫描收尾 ScanEnd 用）：队列满时等待消费循环排空；
    /// 消费循环从不等待 runner 线程，无死锁。保证扫描收尾必然到达
    pub(crate) fn send_blocking(&self, job: Job) {
        if self.tx.send(job).is_ok() {
            self.queued_jobs.fetch_add(1, Ordering::SeqCst);
        }
    }
}

pub(crate) struct PipelineCtx {
    pub(crate) paths: LibraryPaths,
    pub(crate) config: Arc<LibraryConfig>,
    pub(crate) store: Arc<MetadataStore>,
    pub(crate) index: Arc<ItemIndex>,
    pub(crate) thumbs: ThumbnailService,
    pub(crate) bus: EventBus,
    pub(crate) scanner: LibraryScanner,
    pub(crate) migrator: Arc<TaxonomyMigrator>,
    pub(crate) prefs: Arc<ViewPreferences>,
    pub(crate) global_filter: Arc<GlobalFilter>,
    pub(crate) worker: Arc<ThumbnailWorker>,
    pub(crate) startup: Arc<StartupState>,
    pub(crate) settings: Settings,
    pub(crate) sender: JobSender,
    pub(crate) overflow: Arc<AtomicBool>,
    /// 溢出兜底扫描已排队（避免事件风暴期反复排队扫描）
    pub(crate) scan_scheduled: AtomicBool,
    /// 扫描会话活动中（runner 在途或 ScanFile 尚未收尾）
    pub(crate) scanning: AtomicBool,
    /// 暂存的防抖延迟路径（同路径去重）
    pub(crate) deferred: Mutex<HashSet<String>>,
    pub(crate) queued_jobs: Arc<AtomicI32>,
    pub(crate) last_scan: Mutex<Option<ScanProgress>>,
    pub(crate) progress_last_at: AtomicI64,
    pub(crate) progress_idle: AtomicBool,
    /// 暂存的调色板回写（hash → 最新提炼结果）；同 hash 去重，按批冲刷
    pub(crate) palette_pending: Mutex<Vec<(String, Vec<PaletteColor>)>>,
    pub(crate) palette_oldest: Mutex<Option<std::time::Instant>>,
    /// 冲刷定时任务是否已排队（避免重复 spawn）
    pub(crate) palette_timer: AtomicBool,
    pub(crate) runtime: tokio::runtime::Handle,
    /// 活动扫描会话（扫描窗口内消费侧变更的簿记载体）
    pub(crate) scan_session: Mutex<Option<Arc<ScanSession>>>,
}

/// 当前活动扫描会话；无扫描在途返回 None
pub(crate) fn active_session(ctx: &PipelineCtx) -> Option<Arc<ScanSession>> {
    ctx.scan_session.lock().unwrap().clone()
}

/// 完成回复通道（无回复方静默）
pub(crate) fn complete<T>(reply: Reply<T>, value: T) {
    if let Some(tx) = reply {
        let _ = tx.send(value);
    }
}

/// 索引进度快照的唯一组装点（index_progress 方法与 publish_index_progress 共用）
pub(crate) fn index_progress_snapshot(ctx: &PipelineCtx) -> TaskProgress {
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
}

/// 进度节流推送(500ms 一帧)；刚从非空闲转空闲时补发一帧清零，客户端据此撤掉进度指示
pub(crate) fn publish_index_progress(ctx: &PipelineCtx, force: bool) {
    let progress = index_progress_snapshot(ctx);
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
        .publish(crate::core::taxonomy::ItemEvents::TASK_PROGRESS, serde_json::to_value(progress).unwrap());
}

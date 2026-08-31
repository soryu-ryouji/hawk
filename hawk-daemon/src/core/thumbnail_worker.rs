//! 缩略图/调色板后台 worker：独立队列与专用线程池(CPU 密集)，不阻塞索引消费循环。
//! 完成结果经回调回流水线（PaletteJob/FixDim 回写索引，保持单写者）。
//! 队列是尽力而为的缓存：in-flight 去重，重复派发只产生 no-op 任务。
//!
//! 任务三种（ThumbJobKind，in-flight 去重 key 分命名空间）：
//! - PaletteOnly：入库/对账派发，提炼调色板 + 补缺失宽高——
//!   扫描导入已在并行哈希阶段单次解码产出调色板+缩略图（见 pipeline.rs），
//!   此任务兜底增量路径与解码失败自愈；颜色搜索依赖全量 palette，必须即时
//! - Repair：读取端 /item/thumbnail 未命中与范围刷新缓存（library/refresh_cache）派发，
//!   补缺失宽高 + 生成缺失尺寸缩略图 + 按需提炼调色板（不重建已有文件）
//! - ForceRebuild：单 item 手动刷新（item/refresh_thumbnail）派发，强制重建全部尺寸
//!
//! 补宽高（ensure_dim）是三类任务共有的前置步骤：入库时 decode/identify 暂时失败
//! （文件占用等）会把 width=0 落库且无自愈路径，此处用 identify（只解头部）兜底回写。

use crate::core::color;
use crate::core::config::LibraryConfig;
use crate::core::events::{EventBus, TaskProgress};
use crate::core::item::{ItemDto, PaletteColor};
use crate::core::taxonomy::ItemEvents;
use crate::core::thumbnail::ThumbnailService;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

/// 任务模式
enum ThumbJobKind {
    /// 仅提炼调色板 + 补缺失宽高（入库/对账派发）
    PaletteOnly,
    /// 补全派生缓存：缺失宽高 + 缺失尺寸缩略图 + 调色板（读取端/范围刷新派发）
    Repair,
    /// 强制重建全部尺寸缩略图 + 补宽高 + 调色板（单 item 手动刷新派发）
    ForceRebuild,
}

struct ThumbJob {
    hash: String,
    source_abs: String,
    kind: ThumbJobKind,
}

type GetItemDto = Arc<dyn Fn(&str) -> Option<ItemDto> + Send + Sync>;
type ItemDimZero = Arc<dyn Fn(&str) -> bool + Send + Sync>;
type FixDim = Arc<dyn Fn(String, i32, i32) + Send + Sync>;
type HasPalette = Arc<dyn Fn(&str) -> bool + Send + Sync>;
type EnqueuePalette = Arc<dyn Fn(String, Vec<PaletteColor>) + Send + Sync>;

#[derive(Clone)]
struct WorkerCallbacks {
    get_item_dto: GetItemDto,
    item_dim_zero: ItemDimZero,
    fix_dim: FixDim,
    has_palette: HasPalette,
    enqueue_palette: EnqueuePalette,
}

pub struct ThumbnailWorker {
    tx: Mutex<Option<std::sync::mpsc::Sender<ThumbJob>>>,
    rx: Arc<Mutex<std::sync::mpsc::Receiver<ThumbJob>>>,
    inflight: Arc<Mutex<HashSet<String>>>,
    queued: Arc<AtomicI32>,
    active: Arc<AtomicI32>,
    callbacks: Mutex<WorkerCallbacks>,
    thumbs: ThumbnailService,
    config: Arc<LibraryConfig>,
    bus: EventBus,
    started: AtomicBool,
}

impl ThumbnailWorker {
    pub fn new(
        thumbs: ThumbnailService,
        config: Arc<LibraryConfig>,
        bus: EventBus,
    ) -> Arc<ThumbnailWorker> {
        // 无界队列：任务是尽力而为的缓存，但「队列满静默丢弃」曾导致大批量入库时 20%+ 素材
        // 永久丢失缩略图（24k 库丢 5.6k）。任务仅 ~100B 且 in-flight 有去重，无界是安全的；
        // 积压经 task.progress 可见，周期对账会自愈真正缺失的派生缓存
        let (tx, rx) = std::sync::mpsc::channel();
        Arc::new(ThumbnailWorker {
            tx: Mutex::new(Some(tx)),
            rx: Arc::new(Mutex::new(rx)),
            inflight: Arc::new(Mutex::new(HashSet::new())),
            queued: Arc::new(AtomicI32::new(0)),
            active: Arc::new(AtomicI32::new(0)),
            callbacks: Mutex::new(WorkerCallbacks {
                get_item_dto: Arc::new(|_| None),
                item_dim_zero: Arc::new(|_| false),
                fix_dim: Arc::new(|_, _, _| {}),
                has_palette: Arc::new(|_| true),
                enqueue_palette: Arc::new(|_, _| {}),
            }),
            thumbs,
            config,
            bus,
            started: AtomicBool::new(false),
        })
    }

    /// 由 IndexPipeline 装配：索引访问与回写的闭环在流水线侧（单写者）
    pub fn attach(
        &self,
        get_item_dto: GetItemDto,
        item_dim_zero: ItemDimZero,
        fix_dim: FixDim,
        has_palette: HasPalette,
        enqueue_palette: EnqueuePalette,
    ) {
        *self.callbacks.lock().unwrap() = WorkerCallbacks {
            get_item_dto,
            item_dim_zero,
            fix_dim,
            has_palette,
            enqueue_palette,
        };
    }

    /// 积压快照(排队 + 生成中)；task.progress 事件与 app/status 端点共用
    pub fn backlog(&self) -> (i32, i32) {
        (self.queued.load(Ordering::SeqCst), self.active.load(Ordering::SeqCst))
    }

    pub fn start(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        // 纯 CPU 后台任务(解码/缩放/WebP 编码)：专用后台线程，与 API 线程争用 CPU 时靠 OS 调度；
        // 并发 CPU/2（封顶 12）——缩略图是尽力而为的缓存，但桌面端大批量导入时吞吐优先，磁盘未饱和前吃满 CPU
        let parallelism = (std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4) as i32
            / 2)
            .clamp(2, 12);
        for _ in 0..parallelism {
            let rx = self.rx.clone();
            let inflight = self.inflight.clone();
            let queued = self.queued.clone();
            let active = self.active.clone();
            let callbacks = self.callbacks.lock().unwrap().clone();
            let thumbs = self.thumbs.clone();
            let config = self.config.clone();
            let bus = self.bus.clone();
            let progress_last = Arc::new(AtomicI64::new(0));
            let progress_idle = Arc::new(AtomicBool::new(true));
            std::thread::spawn(move || loop {
                let job = {
                    let rx = rx.lock().unwrap();
                    match rx.recv() {
                        Ok(job) => job,
                        Err(_) => return, // 发送端已关闭（进程退出）
                    }
                };
                queued.fetch_sub(1, Ordering::SeqCst);
                active.fetch_add(1, Ordering::SeqCst);
                process_job(
                    &job,
                    &callbacks,
                    &thumbs,
                    &config,
                    &bus,
                );
                inflight.lock().unwrap().remove(&job.dedup_key());
                active.fetch_sub(1, Ordering::SeqCst);
                report_progress(&bus, &queued, &active, &progress_last, &progress_idle);
            });
        }
    }

    /// 派发 Repair 任务：读取端未命中/范围刷新缓存时调用，补宽高 + 生成缺失尺寸（含按需调色板）。
    /// 返回是否实际入队（in-flight 去重时丢弃）
    pub fn enqueue_thumbs(&self, hash: &str, source_abs: &str) -> bool {
        self.enqueue(hash, source_abs, ThumbJobKind::Repair)
    }

    /// 派发 ForceRebuild 任务：单 item 手动刷新时调用，强制重建全部尺寸
    pub fn enqueue_force_rebuild(&self, hash: &str, source_abs: &str) -> bool {
        self.enqueue(hash, source_abs, ThumbJobKind::ForceRebuild)
    }

    /// 派发 PaletteOnly 任务：入库/启动对账/读取端宽高自愈时调用，提炼调色板 + 补宽高
    pub fn enqueue_palette(&self, hash: &str, source_abs: &str) -> bool {
        self.enqueue(hash, source_abs, ThumbJobKind::PaletteOnly)
    }

    fn enqueue(&self, hash: &str, source_abs: &str, kind: ThumbJobKind) -> bool {
        let job = ThumbJob {
            hash: hash.to_string(),
            source_abs: source_abs.to_string(),
            kind,
        };
        if !self.inflight.lock().unwrap().insert(job.dedup_key()) {
            return false;
        }
        let tx = self.tx.lock().unwrap();
        if let Some(tx) = tx.as_ref() {
            if tx.send(job).is_ok() {
                self.queued.fetch_add(1, Ordering::SeqCst);
                return true;
            }
        }
        false
    }
}

impl ThumbJob {
    /// in-flight 去重 key：PaletteOnly 与其余任务独立命名空间，互不挤占
    /// （PaletteOnly 在途时到达的 Repair/ForceRebuild 必须执行，反之 Repair 自带调色板检查）
    fn dedup_key(&self) -> String {
        match self.kind {
            ThumbJobKind::PaletteOnly => format!("palette:{}", self.hash),
            _ => self.hash.clone(),
        }
    }
}

fn process_job(
    job: &ThumbJob,
    callbacks: &WorkerCallbacks,
    thumbs: &ThumbnailService,
    config: &Arc<LibraryConfig>,
    bus: &EventBus,
) {
    // 补缺失宽高：identify 只解头部，代价小。回写经回调走流水线（单写者），
    // item.updated 由回写侧发出；此处只需记住发生了修复，与缩略图生成合并发事件。
    // 识别失败（非图像/损坏文件）记 debug：非图像文件的宽高合法为 0，对账会周期性重派，warn 会刷屏
    let mut dim_fixed = false;
    if (callbacks.item_dim_zero)(&job.hash) {
        match ThumbnailService::identify(&job.source_abs) {
            Some((w, h)) => {
                (callbacks.fix_dim)(job.hash.clone(), w, h);
                dim_fixed = true;
            }
            None => {
                tracing::debug!("宽高识别失败 {source}: 非图像或损坏文件，0 宽高保持", source = job.source_abs);
            }
        }
    }

    let need_palette = !(callbacks.has_palette)(&job.hash);
    // PaletteOnly 不生成缩略图（缩略图是惰性缓存，由读取端派发）
    let generated = match &job.kind {
        ThumbJobKind::PaletteOnly => false,
        ThumbJobKind::Repair => {
            let sizes: Vec<i32> = config
                .current()
                .thumbnail_sizes
                .into_iter()
                .filter(|s| !thumbs.exists(&job.hash, *s))
                .collect();
            !sizes.is_empty() && thumbs.generate(&job.hash, &job.source_abs, &sizes, false)
        }
        ThumbJobKind::ForceRebuild => {
            thumbs.generate(&job.hash, &job.source_abs, &config.current().thumbnail_sizes, true)
        }
    };

    // 调色板优先从最小尺寸的已有缩略图提炼（解码代价小）；缩略图尚未生成
    // （惰性首访前、仅调色板任务）时直接解码原图——内容寻址保证同一内容，提炼结果一致。
    // 提炼结果(含空数组负缓存)经 PaletteJob 写入元数据 TOML——内容的纯函数，全平台复用。
    // 缩略图生成失败但已有任一尺寸缩略图时也照常提炼（取最小已有尺寸）
    if need_palette {
        let mut all_sizes = config.current().thumbnail_sizes;
        all_sizes.sort();
        let source = all_sizes
            .into_iter()
            .map(|s| thumbs.get_path(&job.hash, s))
            .find(|p| std::path::Path::new(p).is_file())
            .unwrap_or_else(|| job.source_abs.clone());
        if let Some(palette) = color::extract(&source) {
            (callbacks.enqueue_palette)(job.hash.clone(), palette);
        }
    }

    // 完成后补发 item.updated:前端缩略图此前的 404 占位据此重建 <img>；
    // 宽高修复同样需要前端刷新骨架/卡片（0 × 0 → 实际尺寸）
    if generated || dim_fixed {
        if let Some(dto) = (callbacks.get_item_dto)(&job.hash) {
            bus.publish(ItemEvents::UPDATED, serde_json::to_value(&dto).unwrap());
        }
    }
}

/// 积压变化时节流推送 task.progress(500ms 一帧);刚从非空闲转空闲时补发一帧清零
fn report_progress(
    bus: &EventBus,
    queued: &AtomicI32,
    active: &AtomicI32,
    last_at: &AtomicI64,
    idle: &AtomicBool,
) {
    let pending = queued.load(Ordering::SeqCst);
    let act = active.load(Ordering::SeqCst);
    let now_idle = pending == 0 && act == 0;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let last = last_at.load(Ordering::SeqCst);
    let due = now - last >= 500;
    if !due && !(now_idle && !idle.load(Ordering::SeqCst)) {
        return;
    }
    if last_at
        .compare_exchange(last, now, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    idle.store(now_idle, Ordering::SeqCst);
    bus.publish(
        ItemEvents::TASK_PROGRESS,
        serde_json::to_value(TaskProgress {
            task: "thumbnail",
            pending,
            active: act,
            phase: None,
            processed: None,
            total: None,
        })
        .unwrap(),
    );
}

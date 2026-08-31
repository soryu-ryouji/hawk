//! 缩略图/调色板后台 worker：独立队列与专用线程池(CPU 密集)，不阻塞索引消费循环。
//! 完成结果经回调回流水线（PaletteJob 回写索引，保持单写者）。
//! 队列是尽力而为的缓存：满时丢弃，in-flight 去重。
//!
//! 任务两种（generate_thumbs 区分，in-flight 去重 key 分命名空间）：
//! - 缩略图任务：读取端 /item/thumbnail 未命中时派发，生成缺失尺寸并按需提炼调色板
//! - 调色板任务：入库/启动对账派发（needs_palette_work），只提炼调色板——
//!   缩略图是惰性缓存，不在入库时批量生成；颜色搜索依赖全量 palette，必须即时
//! 与 C# ThumbnailWorker 语义不同（C# 为全量生成）。

use crate::core::color;
use crate::core::config::LibraryConfig;
use crate::core::events::{EventBus, TaskProgress};
use crate::core::item::{ItemDto, PaletteColor};
use crate::core::taxonomy::ItemEvents;
use crate::core::thumbnail::ThumbnailService;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

struct ThumbJob {
    hash: String,
    source_abs: String,
    /// true：生成缺失尺寸缩略图（读取端派发）；false：仅提炼调色板（入库/对账派发）
    generate_thumbs: bool,
}

type GetItemDto = Arc<dyn Fn(&str) -> Option<ItemDto> + Send + Sync>;
type HasPalette = Arc<dyn Fn(&str) -> bool + Send + Sync>;
type EnqueuePalette = Arc<dyn Fn(String, Vec<PaletteColor>) + Send + Sync>;

#[derive(Clone)]
struct WorkerCallbacks {
    get_item_dto: GetItemDto,
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
                has_palette: Arc::new(|_| true),
                enqueue_palette: Arc::new(|_, _| {}),
            }),
            thumbs,
            config,
            bus,
            started: AtomicBool::new(false),
        })
    }

    /// 由 IndexPipeline 装配：索引访问与调色板回写的闭环在流水线侧（单写者）
    pub fn attach(&self, get_item_dto: GetItemDto, has_palette: HasPalette, enqueue_palette: EnqueuePalette) {
        *self.callbacks.lock().unwrap() = WorkerCallbacks {
            get_item_dto,
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

    /// 派发缩略图任务：读取端未命中时调用，生成缺失尺寸（含按需调色板）。
    /// in-flight 去重（队列中/生成中的同 hash 重复派发直接丢弃）——
    /// 幂等，重复派发只产生 no-op 任务（尺寸齐备 + 调色板已提炼时不做事）
    pub fn enqueue_thumbs(&self, hash: &str, source_abs: &str) {
        self.enqueue(hash, source_abs, true);
    }

    /// 派发调色板任务：入库/启动对账时调用，只提炼调色板，不生成缩略图
    pub fn enqueue_palette(&self, hash: &str, source_abs: &str) {
        self.enqueue(hash, source_abs, false);
    }

    fn enqueue(&self, hash: &str, source_abs: &str, generate_thumbs: bool) {
        let job = ThumbJob {
            hash: hash.to_string(),
            source_abs: source_abs.to_string(),
            generate_thumbs,
        };
        if !self.inflight.lock().unwrap().insert(job.dedup_key()) {
            return;
        }
        let tx = self.tx.lock().unwrap();
        if let Some(tx) = tx.as_ref() {
            if tx.send(job).is_ok() {
                self.queued.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
}

impl ThumbJob {
    /// in-flight 去重 key：缩略图与调色板任务独立命名空间，互不挤占
    /// （调色板任务在途时到达的缩略图任务必须执行，反之缩略图任务自带调色板检查）
    fn dedup_key(&self) -> String {
        if self.generate_thumbs {
            self.hash.clone()
        } else {
            format!("palette:{}", self.hash)
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
    let need_palette = !(callbacks.has_palette)(&job.hash);
    // 仅调色板任务不生成缩略图（缩略图是惰性缓存，由读取端派发）
    let sizes: Vec<i32> = if job.generate_thumbs {
        config
            .current()
            .thumbnail_sizes
            .into_iter()
            .filter(|s| !thumbs.exists(&job.hash, *s))
            .collect()
    } else {
        Vec::new()
    };
    if sizes.is_empty() && !need_palette {
        return;
    }

    let generated = !sizes.is_empty() && thumbs.generate(&job.hash, &job.source_abs, &sizes, false);

    // 调色板优先从最小尺寸的已有缩略图提炼（解码代价小）；缩略图尚未生成
    // （惰性首访前、仅调色板任务）时直接解码原图——内容寻址保证同一内容，提炼结果一致。
    // 提炼结果(含空数组负缓存)经 PaletteJob 写入元数据 TOML——内容的纯函数，全平台复用。
    // 缩略图生成失败但已有任一尺寸缩略图时也照常提炼（与 C# 行为一致：取最小已有）
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

    // 生成完成后补发 item.updated:前端缩略图此前的 404 占位据此重建 <img>
    if generated {
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



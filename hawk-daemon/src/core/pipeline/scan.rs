//! 全库扫描。职责分三层，索引/元数据写入仍全部发生在消费循环（单写者不变）：
//!
//! - **runner 线程**（`hawk-scan`）：目录快照对比 → dirty 目录深入做哈希复用判定 →
//!   需哈希文件并行处理（哈希 + 单次解码产出 调色板/缩略图/宽高）。只读源文件、
//!   读索引/元数据快照、写内容寻址缩略图缓存（幂等），不写任何索引状态。
//! - **回流**：结果以 `Job::ScanFile` 入队，消费循环与其他任务穿插应用——
//!   消费线程不再被长扫描独占，交互写延迟有界。runner 单线程入队保证 FIFO
//!   （ScanEnd 必然最后到达）；ScanFile 满队丢弃由溢出兜底扫描收敛。
//! - **收尾**（`Job::ScanEnd`）：消失对账（seen ∪ touched 豁免）、目录快照整体替换、
//!   items.added 尾批冲刷、folder.changed 广播、回复完成。
//!
//! 扫描窗口内的消费侧变更经会话簿记收敛新竞态：
//! - `touched`：窗口内 upsert/移动 新增的位置——消失对账豁免
//!   （目录可能已被 runner 枚举过，后到的文件不在 seen 集，否则会被误判消失）
//! - `invalidated`：窗口内删除/移走的位置——迟到的 ScanFile 丢弃，不复活已删位置
//! - `rescan_requested`：扫描在途时的新扫描请求（full/force_walk 按位或合并），完成后自动补扫
//!
//! full=true 对所有文件重算哈希(library/reindex)；force_walk=用户手动刷新:
//! 忽略快照强制遍历全部文件(仍按 size/mtime 复用哈希,不读内容)。

use crate::core::color;
use crate::core::events::{folder_changed_payload, REASON_EXTERNAL};
use crate::core::paths::LibraryPaths;
use crate::core::taxonomy::ItemEvents;
use crate::core::thumbnail::ThumbnailService;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::ctx::{active_session, publish_index_progress, PipelineCtx, Reply, ScanProgress};
use super::ctx::complete as complete_reply;
use super::fs_ops;
use super::upsert::{apply_upsert, defer_upsert, file_changed_since_prepare, needs_palette_work, prepare_upsert, PrepareOutcome, PendingUpsert, try_compute_hash};
use super::Job;

/// items.added 批量事件合并窗口：距首条暂存超该值即冲刷
/// （apply 是单线程连续循环，逐条查时钟即可，无需定时器）
const ADDED_BATCH_AFTER: Duration = Duration::from_millis(300);
/// items.added 批量事件条数上限：达到立即冲刷，约束单条 SSE 负载
const ADDED_BATCH_MAX: usize = 2000;

/// 活动扫描会话：runner 与消费循环共享的簿记载体。
/// 会话由消费循环创建（ScanStart）与摘除（ScanEnd），窗口内的变更记录保证收尾对账正确
pub(crate) struct ScanSession {
    pub(crate) full: bool,
    pub(crate) force_walk: bool,
    pub(crate) reply: Mutex<Reply<Result<(), String>>>,
    /// 窗口内消费侧新增/刷新的位置（消失对账豁免）
    pub(crate) touched: Mutex<HashSet<String>>,
    /// 窗口内消费侧删除/移走的位置（迟到 ScanFile 丢弃）
    pub(crate) invalidated: Mutex<HashSet<String>>,
    /// items.added 批量合并暂存（仅消费循环访问）
    pub(crate) batcher: Mutex<AddedBatcher>,
    /// 已应用的扫描文件数（日志/进度用）
    pub(crate) applied: AtomicI32,
    /// 扫描在途时的合并请求 (full, force_walk)，完成后自动补扫
    pub(crate) rescan_requested: Mutex<(bool, bool)>,
}

/// runner 的遍历结果，随 Job::ScanEnd 交给收尾（单写者传递，无并发访问）
#[derive(Default)]
pub(crate) struct WalkOutcome {
    /// dirty 目录中枚举到的文件相对路径
    pub(crate) seen: HashSet<String>,
    pub(crate) seen_dirs: HashSet<String>,
    pub(crate) dirty_dirs: Vec<String>,
    pub(crate) dir_stats: HashMap<String, (i64, i64)>,
    /// 遍历不完整（目录枚举失败）：收尾跳过消失对账与快照替换
    pub(crate) incomplete: bool,
    /// 枚举文件数 / 计算哈希数（日志用）
    pub(crate) files: i32,
    pub(crate) hashed: i32,
}

/// 扫描启动（Job::ScanStart 处理）：消费线程内完成——建会话、spawn runner，立即返回。
/// 已有扫描在途时合并请求（回复立即成功，本轮完成后自动补扫）
pub(crate) fn start(ctx: &Arc<PipelineCtx>, full: bool, force_walk: bool, reply: Reply<Result<(), String>>) {
    ctx.scan_scheduled.store(false, Ordering::SeqCst);
    if ctx.scanning.swap(true, Ordering::SeqCst) {
        if let Some(session) = active_session(ctx) {
            let mut req = session.rescan_requested.lock().unwrap();
            req.0 |= full;
            req.1 |= force_walk;
        }
        complete_reply(reply, Ok(()));
        return;
    }
    ctx.config.reload();
    let session = Arc::new(ScanSession {
        full,
        force_walk,
        reply: Mutex::new(reply),
        touched: Mutex::new(HashSet::new()),
        invalidated: Mutex::new(HashSet::new()),
        batcher: Mutex::new(AddedBatcher::default()),
        applied: AtomicI32::new(0),
        rescan_requested: Mutex::new((false, false)),
    });
    *ctx.scan_session.lock().unwrap() = Some(session.clone());

    let ctx2 = ctx.clone();
    let session_for_runner = session.clone();
    let spawned = std::thread::Builder::new()
        .name("hawk-scan".to_string())
        .spawn(move || run_runner(ctx2, session_for_runner));
    if spawned.is_err() {
        // 线程创建失败：就地收尾报错（会话摘除、标志复位），恢复可再扫描状态
        *ctx.scan_session.lock().unwrap() = None;
        ctx.scanning.store(false, Ordering::SeqCst);
        let reply = session.reply.lock().unwrap().take();
        complete_reply(reply, Err("启动扫描线程失败".to_string()));
    }
}

/// runner 线程主体：遍历 + 并行哈希，结果经队列回流；任何失败都保证 ScanEnd 到达
/// （阻塞入队）——消费循环据此收尾，扫描状态不会卡死
fn run_runner(ctx: Arc<PipelineCtx>, session: Arc<ScanSession>) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_phases(&ctx, &session)));
    let (walk, error) = match result {
        Ok(Ok(walk)) => (walk, None),
        Ok(Err(e)) => (WalkOutcome::default(), Some(e)),
        Err(_) => (WalkOutcome::default(), Some("扫描线程 panic".to_string())),
    };
    ctx.sender.send_blocking(Job::ScanEnd { session, walk, error });
}

/// 扫描主体：目录级增量判定 → 脏目录文件级复用判定 → 需哈希文件并行处理。
/// 全程不写索引/元数据；结果（复用项与哈希完成项）以 Job::ScanFile 回流
fn run_phases(ctx: &Arc<PipelineCtx>, session: &Arc<ScanSession>) -> Result<WalkOutcome, String> {
    let mut walk = WalkOutcome::default();
    let walk_incomplete = AtomicBool::new(false);
    let reporter = ScanReporter::new();

    // 阶段一:目录快照对比。遍历目录取 (mtime, 直接子项数),
    // 与上轮快照一致 = 无增删重命名 → 跳过整个目录的文件级访问;
    // 首轮快照为空或强制遍历(手动刷新) = 全部深入
    let snapshots: HashMap<String, (i64, i64)> = if session.full || session.force_walk {
        HashMap::new()
    } else {
        ctx.store.load_folder_snapshots()
    };
    reporter.report(ctx, "scan", 0, 0, true);
    for (rel, mtime, entries) in ctx.scanner.walk_directory_stats(&walk_incomplete) {
        walk.seen_dirs.insert(rel.clone());
        walk.dir_stats.insert(rel.clone(), (mtime, entries));
        if snapshots.get(&rel).map(|s| *s != (mtime, entries)).unwrap_or(true) {
            walk.dirty_dirs.push(rel);
        }
        reporter.report(ctx, "scan", walk.seen_dirs.len() as i32, 0, false);
    }

    // 阶段二:只深入有变化的目录,枚举直接文件做复用判定/哈希(clean 目录不碰文件系统)
    let mut pending: Vec<PendingUpsert> = Vec::new();
    for rel_dir in &walk.dirty_dirs {
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
                walk.seen.insert(rel);
                walk.files += 1;
            }
            match prepare_upsert(ctx, &abs, session.full, true, 0) {
                PrepareOutcome::Apply(mut p) => {
                    if let Some(hash) = p.reused_hash.clone() {
                        // 复用项免哈希,直接回流应用
                        p.hash = Some(hash);
                        ctx.sender.fire(Job::ScanFile { pending: p });
                    } else {
                        pending.push(p);
                    }
                }
                PrepareOutcome::Remove(rel) => {
                    // ignore 规则命中/文件已消失：经队列按删除处理（单写者纪律）
                    if let Some(abs) = ctx.paths.to_absolute(&rel) {
                        ctx.sender.fire(Job::Delete { abs });
                    }
                }
                PrepareOutcome::Skip => {}
            }
        }
    }

    // 阶段三:并行哈希。哈希+解码是导入吞吐瓶颈：留 1 核给 API，其余吃满
    walk.hashed = pending.len() as i32;
    if !pending.is_empty() {
        let pending_total = walk.hashed;
        reporter.report(ctx, "hash", 0, pending_total, true);
        let parallelism = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .saturating_sub(1)
            .clamp(2, 24)
            .min(pending.len());
        let hashed = AtomicUsize::new(0);
        // 多个 move 闭包共享：引用可 Copy，闭包各自捕获一份（与旧实现同法）
        let hashed = &hashed;
        let reporter = &reporter;
        // 分成 owned 块散给工作线程（结果经 channel 按值传回，借用块无法移出元素）
        let chunk_size = pending.len().div_ceil(parallelism);
        let mut chunks: Vec<Vec<PendingUpsert>> = Vec::new();
        let mut it = pending.into_iter();
        for _ in 0..parallelism {
            let chunk: Vec<PendingUpsert> = it.by_ref().take(chunk_size).collect();
            if chunk.is_empty() {
                break;
            }
            chunks.push(chunk);
        }
        let (tx, rx) = std::sync::mpsc::channel::<PendingUpsert>();
        std::thread::scope(|s| {
            for chunk in chunks {
                let tx = tx.clone();
                s.spawn(move || {
                    for mut p in chunk {
                        process_scan_hash(ctx, &mut p);
                        // 哈希成功（含派生产出）才回流；失败/写入中已由 process_scan_hash 自愈
                        if p.hash.is_some() {
                            let _ = tx.send(p);
                        }
                        reporter.report(
                            ctx,
                            "hash",
                            hashed.fetch_add(1, Ordering::SeqCst) as i32 + 1,
                            pending_total,
                            false,
                        );
                    }
                });
            }
            // 工作线程结果统一回流队列（由消费循环穿插应用）
            drop(tx);
            while let Ok(p) = rx.recv() {
                ctx.sender.fire(Job::ScanFile { pending: p });
            }
        });
    }

    walk.incomplete = walk_incomplete.load(Ordering::SeqCst);
    Ok(walk)
}

/// 并行阶段单文件处理：内容哈希 → 写入复验 → 单次解码产出 调色板/缩略图/宽高。
/// 只读源文件 + 写内容寻址的缩略图缓存（幂等）；索引/元数据由消费循环经回流结果改写。
/// 派生齐备（调色板版本一致 + 尺寸齐全）时不解码,哈希后即走。
/// 解码失败不阻断入库：调色板保持未提炼（对账/worker 重试），缩略图走读取端惰性兜底
fn process_scan_hash(ctx: &Arc<PipelineCtx>, p: &mut PendingUpsert) {
    let Some(hash) = try_compute_hash(&p.abs_path) else {
        return;
    };
    // 复验:哈希期间仍在写入的文件不半截入库,延迟重试(与 do_upsert 同一纪律)
    if file_changed_since_prepare(p) {
        if std::path::Path::new(&p.abs_path).exists() {
            defer_upsert(ctx, p.abs_path.clone(), 0);
        }
        return;
    }
    p.hash = Some(hash.clone());

    let sizes: Vec<i32> = ctx
        .config
        .current()
        .thumbnail_sizes
        .into_iter()
        .filter(|s| !ctx.thumbs.exists(&hash, *s))
        .collect();
    let need_palette = needs_palette_work(ctx, &hash);
    if sizes.is_empty() && !need_palette {
        return;
    }

    match ThumbnailService::decode(&p.abs_path) {
        Some(image) => {
            p.dim = Some((image.width() as i32, image.height() as i32));
            if need_palette {
                if let Some(rgba) = image.as_rgba8() {
                    p.palette = Some(color::extract_from_rgba(rgba));
                }
            }
            if !sizes.is_empty() {
                ctx.thumbs.generate_from_image(&hash, &image, &sizes);
            }
        }
        None => {
            // 解码失败（截断/非图像）：宽高退头部解析，派生由周期对账自愈
            p.dim = ThumbnailService::identify(&p.abs_path);
        }
    }
}

/// 单个扫描文件的应用（Job::ScanFile 处理，消费循环内穿插执行）
pub(crate) fn apply_scan_file(ctx: &Arc<PipelineCtx>, pending: PendingUpsert) {
    let Some(session) = active_session(ctx) else {
        return; // 会话已收尾：理论上不可能（runner 单线程 FIFO 保证 ScanEnd 最后到达）
    };
    let Some(hash) = pending.hash.clone() else {
        return;
    };
    if session.invalidated.lock().unwrap().contains(&pending.rel) {
        return; // 窗口内已删除/移走：迟到的扫描结果不复活位置
    }
    // 哈希与入库之间存在时间差（穿插应用的窗口）：文件可能已被改写/删除。
    // stat 复验不一致即丢弃——以穿插到达的实时事件/下轮对账为准，不以陈旧内容入库
    if file_changed_since_prepare(&pending) {
        return;
    }
    let mut batcher = session.batcher.lock().unwrap();
    match apply_upsert(ctx, pending, &hash, Some(&mut batcher)) {
        Ok(_) => {
            session.applied.fetch_add(1, Ordering::SeqCst);
        }
        Err(e) => {
            // 单文件失败不中止扫描：其余文件照常入库，失败项由下轮对账重试
            tracing::warn!("扫描入库失败（跳过）: {e}");
        }
    }
}

/// 扫描收尾（Job::ScanEnd 处理）：对账、快照、尾批、事件、回复、补扫检查
pub(crate) fn finish(
    ctx: &Arc<PipelineCtx>,
    session: Arc<ScanSession>,
    walk: WalkOutcome,
    error: Option<String>,
) {
    *ctx.scan_session.lock().unwrap() = None;
    let reply = session.reply.lock().unwrap().take();
    // 尾批冲刷（含失败路径）：滞存的 item.added 不能丢
    session.batcher.lock().unwrap().flush(&ctx.bus);

    if let Some(err) = error {
        tracing::error!("扫描失败: {err}");
        complete_reply(reply, Err(err));
    } else {
        if walk.incomplete {
            // 遍历不完整(部分目录枚举失败)时 seen 不可信:本轮跳过消失对账与快照替换,
            // 避免误删已索引位置或写入残缺快照;最终一致由下一轮对账保证
            tracing::warn!("扫描遍历不完整(目录枚举失败),跳过本轮消失对账与快照更新");
        } else {
            reconcile_missing(ctx, &session, &walk);
            // 快照整体替换为本轮统计(下轮增量的对比基准)
            ctx.store.replace_folder_snapshots(&walk.dir_stats);
        }
        // 对账扫描是目录结构变化的兜底(外部删空目录等不会产生任何事件),广播一次 folder.changed
        ctx.bus
            .publish(ItemEvents::FOLDER_CHANGED, folder_changed_payload(REASON_EXTERNAL));
        report_done(ctx, walk.files);

        tracing::info!(
            "扫描完成:{} 个文件({} 个计算哈希,{} 个目录中 {} 个深入,{} 个已应用),{} 个索引位置",
            walk.files,
            walk.hashed,
            walk.seen_dirs.len(),
            walk.dirty_dirs.len(),
            session.applied.load(Ordering::SeqCst),
            ctx.index.all_location_paths().len()
        );
        complete_reply(reply, Ok(()));
    }

    ctx.scanning.store(false, Ordering::SeqCst);
    // 扫描结束强制发一帧,客户端据此撤掉进度指示
    publish_index_progress(ctx, true);

    let (again_full, again_walk) = *session.rescan_requested.lock().unwrap();
    if again_full || again_walk {
        tracing::info!("扫描在途期间收到新扫描请求，自动补扫");
        ctx.scan_scheduled.store(true, Ordering::SeqCst);
        ctx.sender.fire(Job::ScanStart {
            full: again_full,
            force_walk: again_walk,
            reply: None,
        });
    }
}

/// 消失对账:
/// - 所在目录已不存在(目录树遍历不到)→ 位置必然消失
/// - 所在目录本轮深入过却没在枚举中见到 → 已消失
/// - clean 目录快照与磁盘一致(无增删)→ 位置必然还在,不访问文件系统
/// - 窗口内消费侧新增的位置(touched)→ 豁免（目录可能已被枚举过，后到的文件不在 seen 集）
fn reconcile_missing(ctx: &PipelineCtx, session: &ScanSession, walk: &WalkOutcome) {
    let touched = session.touched.lock().unwrap();
    let dirty_set: HashSet<&str> = walk.dirty_dirs.iter().map(String::as_str).collect();
    for rel in ctx.index.all_location_paths() {
        if walk.seen.contains(&rel) || touched.contains(&rel) {
            continue;
        }
        let dir = LibraryPaths::dir_of(&rel);
        if !walk.seen_dirs.contains(dir) || dirty_set.contains(dir) {
            fs_ops::do_delete(ctx, &rel);
        }
    }
}

fn report_done(ctx: &PipelineCtx, processed: i32) {
    *ctx.last_scan.lock().unwrap() = Some(ScanProgress {
        phase: "done".to_string(),
        processed,
        total: processed,
    });
    ctx.startup.report("done", processed, processed);
}

/// 进度上报:按 150ms 节流,阶段切换/总数变化时强制发一帧。内部状态线程安全(并行哈希阶段多线程上报)
struct ScanReporter {
    state: Mutex<ScanReporterState>,
}

struct ScanReporterState {
    last_at: std::time::Instant,
    phase: String,
    total: i32,
}

impl ScanReporter {
    fn new() -> ScanReporter {
        ScanReporter {
            state: Mutex::new(ScanReporterState {
                last_at: std::time::Instant::now() - Duration::from_secs(60),
                phase: String::new(),
                total: -1,
            }),
        }
    }

    fn report(&self, ctx: &Arc<PipelineCtx>, phase: &str, processed: i32, total: i32, force: bool) {
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
        *ctx.last_scan.lock().unwrap() = Some(ScanProgress {
            phase: phase.to_string(),
            processed,
            total,
        });
        ctx.startup.report(phase, processed, total);
        publish_index_progress(ctx, false);
    }
}

/// 扫描路径 item.added 的批量合并暂存：300ms 窗口 / 2000 条上限合成一条 items.added，
/// 全量导入时避免逐条事件风暴（SSE 订阅者积压 1024 条即被断开）。仅扫描路径使用；
/// 监听/API 单条入库维持即时 item.added
#[derive(Default)]
pub(crate) struct AddedBatcher {
    ids: Vec<String>,
    oldest: Option<std::time::Instant>,
}

impl AddedBatcher {
    /// 暂存一条；到窗口/上限即冲刷
    pub(crate) fn stage(&mut self, bus: &crate::core::events::EventBus, hash: &str) {
        if self.ids.is_empty() {
            self.oldest = Some(std::time::Instant::now());
        }
        self.ids.push(hash.to_string());
        if self.due() {
            self.flush(bus);
        }
    }

    fn due(&self) -> bool {
        self.ids.len() >= ADDED_BATCH_MAX || self.oldest.is_some_and(|t| t.elapsed() >= ADDED_BATCH_AFTER)
    }

    /// 冲刷为一条 items.added；扫描结束/出错前兜底调用，避免尾批滞留
    pub(crate) fn flush(&mut self, bus: &crate::core::events::EventBus) {
        if self.ids.is_empty() {
            return;
        }
        self.oldest = None;
        bus.publish(
            ItemEvents::ITEMS_ADDED,
            serde_json::json!({ "ids": std::mem::take(&mut self.ids) }),
        );
    }
}

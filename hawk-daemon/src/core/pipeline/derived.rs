//! 派生数据回写：worker 提炼的调色板批量回写与宽高补全。
//! 回写走队列回流（Job::Palette/FixDim），应用发生在消费循环（单写者）。

use crate::core::item::PaletteColor;
use crate::core::metadata::ItemMetadata;
use crate::core::taxonomy::ItemEvents;
use std::sync::Arc;
use std::time::Duration;

use super::ctx::PipelineCtx;
use super::upsert::palette_to_entries;

/// 调色板批量回写：达到该条数立即冲刷（SQLite 事务开销从 N 降到 1，
/// 事件按批平滑补发，避免全库重提炼时的 item.updated 洪峰）
const PALETTE_BATCH: usize = 500;
/// 调色板批量回写的时间冲刷阈值：滞留超时时即使未达批大小也冲刷
const PALETTE_FLUSH_AFTER: Duration = Duration::from_millis(2000);

/// 暂存调色板提炼结果（同 hash 以最新为准），达批大小立即冲刷；
/// 未达批大小时 spawn 一次性定时任务（PALETTE_FLUSH_AFTER 后入队 PaletteFlush 冲刷）——
/// 安静期（无后续 job）也能落盘，不依赖消费循环的任务到达
pub(crate) fn stage_palette(ctx: &Arc<PipelineCtx>, hash: String, palette: Vec<PaletteColor>) {
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
    if !ctx.palette_timer.swap(true, std::sync::atomic::Ordering::SeqCst) {
        let ctx2 = ctx.clone();
        let runtime = ctx.runtime.clone();
        runtime.spawn(async move {
            tokio::time::sleep(PALETTE_FLUSH_AFTER).await;
            // 到点仍由消费循环执行冲刷（单写者）；定时任务只负责唤醒
            ctx2.sender.fire(super::Job::PaletteFlush);
        });
    }
}

/// 消费循环每处理完一个 job 检查一次：滞留超时即使未达批大小也冲刷（事件平滑发出）
pub(crate) fn maybe_flush_palette(ctx: &PipelineCtx) {
    let oldest = *ctx.palette_oldest.lock().unwrap();
    if let Some(t) = oldest {
        if t.elapsed() >= PALETTE_FLUSH_AFTER {
            flush_palette_batch(ctx);
        }
    }
}

/// 冲刷暂存的调色板回写：逐条落 TOML（铁律：权威层先行），随后内存副本与 SQLite 单事务统一应用，
/// 最后逐 item 同步索引并补发 item.updated。meta 已随漂移/删除消失时丢弃
pub(crate) fn flush_palette_batch(ctx: &PipelineCtx) {
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
        meta.palette = Some(palette_to_entries(&palette));
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

    // 合并为一条 items.updated 发出：一批最多 500 条，逐条 item.updated 会冲爆 SSE 订阅者
    let mut dtos: Vec<crate::core::item::ItemDto> = Vec::with_capacity(applied.len());
    for (hash, meta, _) in &applied {
        if ctx.index.contains(hash) {
            ctx.index.with_item_mut(hash, |item| item.sync_from(meta));
            if let Some(dto) = ctx.index.get_dto(hash) {
                dtos.push(dto);
            }
        }
    }
    if !dtos.is_empty() {
        ctx.bus
            .publish(ItemEvents::ITEMS_UPDATED, serde_json::json!({ "items": dtos }));
    }
}

/// 宽高回写（worker ensure_dim 派发）：仅补 0 值——幂等守卫，任务在途期间 upsert/其他任务
/// 可能已写入正确值，不覆盖。索引与 TOML 同步更新后发 item.updated，前端据此重建骨架/卡片（0 × 0 → 实际尺寸）
pub(crate) fn do_fix_dim(ctx: &PipelineCtx, hash: &str, w: i32, h: i32) {
    let applied = ctx
        .index
        .with_item_mut(hash, |item| {
            if item.width == 0 && item.height == 0 {
                item.width = w;
                item.height = h;
                true
            } else {
                false
            }
        })
        .unwrap_or(false);
    if !applied {
        return;
    }
    if let Some(mut meta) = ctx.store.try_get(hash) {
        if meta.width == 0 && meta.height == 0 {
            meta.width = w;
            meta.height = h;
            if ctx.store.save(hash, &meta).is_err() {
                tracing::warn!("宽高回写 TOML 失败 {hash}");
            }
        }
    }
    if let Some(dto) = ctx.index.get_dto(hash) {
        ItemEvents::publish_changed(&ctx.bus, &dto);
    }
}

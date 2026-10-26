//! 派生数据回写：调色板/pHash 按批落 SQLite，宽高单条补全。
//! 所有应用都发生在消费循环（单写者）；worker 只计算并经 JobSender 回流。

use crate::core::item::PaletteColor;
use crate::core::metadata::{ItemMetadata, PerceptualHash};
use crate::core::taxonomy::ItemEvents;
use std::sync::Arc;
use std::time::Duration;

use super::ctx::PipelineCtx;
use super::upsert::palette_to_entries;

const DERIVED_BATCH: usize = 500;
const DERIVED_FLUSH_AFTER: Duration = Duration::from_millis(2000);

pub(crate) fn stage_palette(ctx: &Arc<PipelineCtx>, hash: String, palette: Vec<PaletteColor>) {
    let full = {
        let mut pending = ctx.palette_pending.lock().unwrap();
        if let Some(entry) = pending.iter_mut().find(|(h, _)| *h == hash) {
            entry.1 = palette;
        } else {
            pending.push((hash, palette));
        }
        pending.len() >= DERIVED_BATCH
    };
    arm_flush(ctx, full);
}

pub(crate) fn stage_phash(ctx: &Arc<PipelineCtx>, hash: String, phash: PerceptualHash) {
    let full = {
        let mut pending = ctx.phash_pending.lock().unwrap();
        if let Some(entry) = pending.iter_mut().find(|(h, _)| *h == hash) {
            entry.1 = phash;
        } else {
            pending.push((hash, phash));
        }
        pending.len() >= DERIVED_BATCH
    };
    arm_flush(ctx, full);
}

/// 达批立即冲刷；否则记录最早暂存时间并只安排一个唤醒任务
fn arm_flush(ctx: &Arc<PipelineCtx>, full: bool) {
    if full {
        flush_derived_batches(ctx);
        return;
    }
    let mut oldest = ctx.derived_oldest.lock().unwrap();
    if oldest.is_none() {
        *oldest = Some(std::time::Instant::now());
    }
    drop(oldest);
    if !ctx
        .derived_timer
        .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        let ctx2 = ctx.clone();
        let runtime = ctx.runtime.clone();
        runtime.spawn(async move {
            tokio::time::sleep(DERIVED_FLUSH_AFTER).await;
            ctx2.sender.fire(super::Job::DerivedFlush);
        });
    }
}

pub(crate) fn maybe_flush_derived(ctx: &PipelineCtx) {
    let oldest = *ctx.derived_oldest.lock().unwrap();
    if oldest.is_some_and(|time| time.elapsed() >= DERIVED_FLUSH_AFTER) {
        flush_derived_batches(ctx);
    }
}

pub(crate) fn flush_derived_batches(ctx: &PipelineCtx) {
    let palette = std::mem::take(&mut *ctx.palette_pending.lock().unwrap());
    let phash = std::mem::take(&mut *ctx.phash_pending.lock().unwrap());
    *ctx.derived_oldest.lock().unwrap() = None;

    let palette_entries: Vec<(String, ItemMetadata)> = palette
        .into_iter()
        .filter_map(|(hash, palette)| {
            let mut meta = ctx.store.try_get(&hash)?;
            meta.palette = Some(palette_to_entries(&palette));
            Some((hash, meta))
        })
        .collect();
    persist_and_publish(ctx, palette_entries, "调色板");

    let phash_entries: Vec<(String, ItemMetadata)> = phash
        .into_iter()
        .filter_map(|(hash, phash)| {
            let mut meta = ctx.store.try_get(&hash)?;
            meta.phash = Some(phash);
            Some((hash, meta))
        })
        .collect();
    persist_and_publish(ctx, phash_entries, "pHash");
}

/// 单事务落盘 + 内存/索引同步 + 合并事件。meta 已随漂移/删除消失的项在组装阶段丢弃
fn persist_and_publish(ctx: &PipelineCtx, to_write: Vec<(String, ItemMetadata)>, label: &str) {
    if to_write.is_empty() {
        return;
    }
    let batch_len = to_write.len();
    let (applied, failed) = ctx.store.persist_batch(&to_write);
    if !failed.is_empty() {
        tracing::warn!(
            "{label}批量回写：{} 条落盘失败（下轮维护重试）",
            failed.len()
        );
    }
    if applied.is_empty() {
        return;
    }
    tracing::info!(
        "{label}批量回写 {} 条（暂存 {batch_len} 条）",
        applied.len()
    );

    let mut dtos = Vec::with_capacity(applied.len());
    for (hash, meta) in &applied {
        if ctx.index.contains(hash) {
            ctx.index.with_item_mut(hash, |item| item.sync_from(meta));
            if ItemEvents::lock_gates(&ctx.locks, &ctx.index, hash) {
                if let Some(dto) = ctx.index.get_dto(hash) {
                    dtos.push(dto);
                }
            }
        }
    }
    if !dtos.is_empty() {
        ctx.bus.publish(
            ItemEvents::ITEMS_UPDATED,
            serde_json::json!({ "items": dtos }),
        );
    }
}

/// 宽高回写（worker ensure_dim 派发）：仅补 0 值——幂等守卫，任务在途期间 upsert/其他任务
/// 可能已写入正确值，不覆盖。索引与 SQLite 同步更新后发 item.updated，前端据此重建骨架/卡片
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
                tracing::warn!("宽高回写 SQLite 失败 {hash}");
            }
        }
    }
    ItemEvents::publish_changed(&ctx.bus, &ctx.index, &ctx.locks, hash);
}

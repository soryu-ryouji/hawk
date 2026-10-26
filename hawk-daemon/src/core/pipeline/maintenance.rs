//! 派生数据维护：周期/启动时补齐调色板、宽高与 pHash；SQLite 是唯一元数据来源，无 TOML 对账。

use super::ctx::PipelineCtx;

/// 纯内存扫描 + worker in-flight 去重，稳态零文件 IO。源文件已消失的项由扫描对账收敛。
pub(crate) fn do_derived_maintenance(ctx: &PipelineCtx) {
    for hash in ctx.store.hashes_with_missing_palette() {
        if let Some(abs) = ctx.index.main_source_abs(&hash, &ctx.paths) {
            ctx.worker.enqueue_derived(&hash, &abs);
        }
    }

    // 入库时 decode/identify 暂时失败会把 width=0 落库；非图像负缓存的 0 是终态，不重试
    for hash in ctx.store.hashes_with_zero_dim() {
        if ctx.store.palette_negative_cached(&hash) {
            continue;
        }
        if ctx
            .index
            .with_item_mut(&hash, |item| item.width == 0)
            .unwrap_or(false)
        {
            if let Some(abs) = ctx.index.main_source_abs(&hash, &ctx.paths) {
                ctx.worker.enqueue_derived(&hash, &abs);
            }
        }
    }

    // 老库/算法升级后的 pHash 补算；与调色板共用 worker，避免重复解码
    for hash in ctx.store.hashes_with_missing_phash() {
        if let Some(abs) = ctx.index.main_source_abs(&hash, &ctx.paths) {
            ctx.worker.enqueue_derived(&hash, &abs);
        }
    }
}

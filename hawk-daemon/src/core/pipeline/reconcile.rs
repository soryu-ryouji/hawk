//! 元数据对账（只进不出：TOML → 缓存/索引）。
//! `.hawk/metadata/` 的 TOML 是唯一权威源（参与网盘同步），本机 SQLite 缓存与
//! 内存副本经此跟随外部变更（网盘同步落地、手工编辑）。

use crate::core::metadata;
use crate::core::paths::unix_ms;
use crate::core::taxonomy::ItemEvents;
use std::collections::HashSet;

use super::ctx::PipelineCtx;

/// 周期对账：按 mtime 与缓存记录比对，只有变化的文件才重新解析。
/// 解析失败的文件跳过且不清空状态，下轮重试。
/// 派生缓存自愈（palette/宽高缺失派发）也在此触发。
pub(crate) fn do_metadata_sync(ctx: &PipelineCtx) {
    let Some(mtimes) = ctx.store.source_mtimes() else {
        return; // 缓存不可用：跳过本轮（退化为重启才收敛）
    };

    let mut seen: HashSet<String> = HashSet::new();
    let mut synced = 0i32;
    let read_dir = match std::fs::read_dir(&ctx.paths.metadata_dir) {
        Ok(d) => d,
        Err(_) => return,
    };
    for entry in read_dir.flatten() {
        let file_name = entry.file_name().to_string_lossy().to_string();
        if !file_name.ends_with(".toml") {
            continue;
        }
        let hash = file_name.strip_suffix(".toml").unwrap();
        if !metadata::is_valid_hash_file_name(hash) {
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

    // 派生缓存自愈：palette 缺失（如 worker 队列高峰期的丢失项、解码曾失败的素材）
    // → 派发仅调色板任务补齐。缩略图不在对账中批量生成（惰性，读取端派发）。
    // 纯内存扫描 + in-flight 去重，稳态零开销。源文件已不在的项由 worker 静默跳过（删除对账会收敛位置）
    for hash in ctx.store.hashes_with_missing_palette() {
        if let Some(abs) = ctx.index.main_source_abs(&hash, &ctx.paths) {
            ctx.worker.enqueue_palette(&hash, &abs);
        }
    }
    // 宽高自愈：入库时 decode/identify 暂时失败（文件占用等）会把 width=0 落库且无自愈路径，
    // 增量扫描按 size/mtime 复用不再触及 → 永久滞留 0 × 0。同一任务（PaletteOnly 含补宽高）兜底
    for hash in ctx.store.hashes_with_zero_dim() {
        if ctx.index.with_item_mut(&hash, |item| item.width == 0).unwrap_or(false) {
            if let Some(abs) = ctx.index.main_source_abs(&hash, &ctx.paths) {
                ctx.worker.enqueue_palette(&hash, &abs);
            }
        }
    }
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

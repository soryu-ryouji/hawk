//! 单文件入库：防抖（写入中文件延迟重试）→ 哈希（known/复用/计算）→ 复验 → 应用。
//! 应用含哈希漂移的元数据迁移继承、位置登记、宽高持久化、事件发布与派生补全派发。
//! prepare 是纯判定（只读索引/元数据/文件系统，不产生副作用），扫描 runner 线程与消费循环共用；
//! 需要删除路径时返回 Remove 由调用方决定如何执行（消费循环内联、runner 经队列回流）。

use crate::core::color_math;
use crate::core::content_hash;
use crate::core::item::PaletteColor;
use crate::core::metadata::{ItemMetadata, PaletteEntry, PathEntry};
use crate::core::paths::{unix_ms, LibraryPaths};
use crate::core::taxonomy::ItemEvents;
use crate::core::thumbnail::ThumbnailService;
use std::sync::Arc;
use std::time::Duration;

use super::ctx::{active_session, PipelineCtx, UpsertResult};
use super::fs_ops;
use super::scan::AddedBatcher;

const MAX_DEBOUNCE_ATTEMPTS: u32 = 120;
/// 写入防抖窗口：mtime 距今不足该值的文件视为仍在写入，延迟重试
const STABILITY_WINDOW_MS: i64 = 1000;

/// 入库准备结果
pub(crate) enum PrepareOutcome {
    Apply(PendingUpsert),
    /// 路径应从索引移除（被 ignore / 不是文件）
    Remove(String),
    /// 跳过（.hawk 内部 / 已安排延迟重试）
    Skip,
}

pub(crate) struct PendingUpsert {
    pub(crate) abs_path: String,
    pub(crate) rel: String,
    pub(crate) lib_path: String,
    pub(crate) size: i64,
    pub(crate) mtime: i64,
    pub(crate) old_hash: Option<String>,
    pub(crate) reused_hash: Option<String>,
    /// 已确认的内容哈希（known/复用直接填、扫描并行阶段计算后填）
    pub(crate) hash: Option<String>,
    pub(crate) dim: Option<(i32, i32)>,
    /// 扫描并行阶段单次解码提炼的调色板（含空数组负缓存）；增量路径为 None
    pub(crate) palette: Option<Vec<PaletteColor>>,
}

/// 单文件入库（消费循环内联调用：watcher 事件 / API 提交 / 移动回退路径）
pub(crate) fn do_upsert(
    ctx: &Arc<PipelineCtx>,
    abs: &str,
    force_hash: bool,
    known_hash: Option<&str>,
    attempt: u32,
    has_reply: bool,
) -> Result<Option<UpsertResult>, String> {
    // 携带已知哈希(item/add)或等待结果的提交不做防抖:文件由 API 写入,内容已完整
    let allow_defer = known_hash.is_none() && !has_reply;
    let pending = match prepare_upsert(ctx, abs, force_hash, allow_defer, attempt) {
        PrepareOutcome::Apply(p) => p,
        PrepareOutcome::Remove(rel) => {
            fs_ops::do_delete(ctx, &rel);
            return Ok(None);
        }
        PrepareOutcome::Skip => return Ok(None),
    };

    let hash = match known_hash {
        Some(h) => Some(h.to_string()),
        None if pending.reused_hash.is_some() => pending.reused_hash.clone(),
        _ => try_compute_hash(&pending.abs_path),
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

    Ok(Some(apply_upsert(ctx, pending, &hash, None)?))
}

/// 哈希前 stat(size/mtime)与现状是否一致。不一致(含文件消失、stat 失败)视为仍在写入;
/// 无副作用——文件消失的清理由删除事件/对账扫描兜底
pub(crate) fn file_changed_since_prepare(p: &PendingUpsert) -> bool {
    match std::fs::metadata(&p.abs_path) {
        Ok(m) => {
            let mtime = m.modified().map(unix_ms).unwrap_or(0);
            m.len() as i64 != p.size || mtime != p.mtime
        }
        Err(_) => true,
    }
}

/// 入库准备:路径过滤、文件状态读取、哈希复用判定、写入中文件防抖。不读文件内容、无副作用
pub(crate) fn prepare_upsert(
    ctx: &Arc<PipelineCtx>,
    abs: &str,
    force_hash: bool,
    allow_defer: bool,
    attempt: u32,
) -> PrepareOutcome {
    let Some(rel) = ctx.paths.to_relative(abs) else {
        return PrepareOutcome::Skip;
    };
    if LibraryPaths::is_internal(&rel) {
        return PrepareOutcome::Skip;
    }

    let in_trash = LibraryPaths::is_in_trash(&rel);
    if !in_trash && ctx.config.is_ignored(&rel) {
        return PrepareOutcome::Remove(rel);
    }

    let meta = std::fs::metadata(abs).ok();
    let Some(file_meta) = meta.filter(|m| m.is_file()) else {
        return PrepareOutcome::Remove(rel);
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
        return PrepareOutcome::Apply(PendingUpsert {
            abs_path: abs.to_string(),
            rel,
            lib_path,
            size,
            mtime,
            old_hash: old_hash.clone(),
            reused_hash: old_hash,
            hash: None,
            dim: None,
            palette: None,
        });
    }

    // 文件可能仍在写入(如大文件拷贝中):不立即哈希,延迟重试直至写入稳定,
    // 避免对半截内容反复算哈希。超出重试上限后按现状处理(后续事件/扫描会自愈)
    if allow_defer && attempt < MAX_DEBOUNCE_ATTEMPTS && is_unstable(mtime) {
        defer_upsert(ctx, abs.to_string(), attempt);
        return PrepareOutcome::Skip;
    }

    PrepareOutcome::Apply(PendingUpsert {
        abs_path: abs.to_string(),
        rel,
        lib_path,
        size,
        mtime,
        old_hash,
        reused_hash: None,
        hash: None,
        dim: None,
        palette: None,
    })
}

/// 文件最近一秒内仍在写入,视为不稳定
fn is_unstable(mtime: i64) -> bool {
    let now = unix_ms(std::time::SystemTime::now());
    now - mtime < STABILITY_WINDOW_MS
}

/// 延迟重试:同一路径只保留一个延迟任务,避免监听事件风暴放大
pub(crate) fn defer_upsert(ctx: &Arc<PipelineCtx>, abs: String, attempt: u32) {
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
        ctx.sender.fire(super::Job::Upsert {
            abs,
            force_hash: false,
            known_hash: None,
            reply: None,
            attempt: attempt + 1,
        });
    });
}

/// 计算内容哈希;读不了(权限/占用)时告警并返回 None
pub(crate) fn try_compute_hash(abs: &str) -> Option<String> {
    match content_hash::hash_file(abs) {
        Ok(h) => Some(h),
        Err(e) => {
            tracing::warn!("计算哈希失败: {abs}: {e}");
            None
        }
    }
}

/// 调色板缺失时才需要后台提炼（存在性查元数据）。缩略图是惰性缓存，
/// 不在入库/对账时生成，由读取端（/item/thumbnail 未命中）派发
pub(crate) fn needs_palette_work(ctx: &PipelineCtx, hash: &str) -> bool {
    match ctx.store.try_get(hash) {
        Some(meta) => meta.palette.is_none(),
        None => true,
    }
}

/// 应用入库结果:元数据迁移与回写、索引更新、事件、派生补全派发。只允许串行调用。
/// added_batch 为 Some 时（扫描路径）created 事件合并进 items.added；
/// 增量路径传 None，维持即时单条 item.added
pub(crate) fn apply_upsert(
    ctx: &Arc<PipelineCtx>,
    pending: PendingUpsert,
    hash: &str,
    mut added_batch: Option<&mut AddedBatcher>,
) -> Result<UpsertResult, String> {
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
    // 扫描导入通道单次解码已提炼调色板（内容的纯函数,含空数组负缓存）：随首版 TOML 一并
    // 持久化,颜色索引随 item 就绪即全量可用;None（解码失败/未提炼）保持现状,由对账/worker 重试
    if let Some(palette) = pending.palette.as_ref() {
        meta.palette = Some(
            palette
                .iter()
                .map(|p| PaletteEntry {
                    color: color_math::to_hex(p.r, p.g, p.b),
                    percentage: p.percentage,
                })
                .collect(),
        );
        meta_changed = true;
    }
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

    // 索引更新;创建即在同一锁内携带位置（零位置 item 不对并发查询可见）；
    // 尺寸为派生信息,索引时从文件读取(扫描路径已在并行哈希阶段预取)
    let (created, added_location) = ctx
        .index
        .get_or_add_with_location(hash, &pending.rel, pending.size, pending.mtime);
    ctx.index.with_item_mut(hash, |item| item.sync_from(&meta));

    // 宽高持久化入 TOML（按 storage.md 的设计意图落盘）
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

    if created {
        match added_batch.as_mut() {
            // 扫描批量路径：合并进 items.added（窗口/上限到期冲刷,扫描结束兜底）
            Some(batch) => batch.stage(&ctx.bus, hash),
            None => {
                if let Some(dto) = ctx.index.get_dto(hash) {
                    ctx.bus.publish(ItemEvents::ADDED, serde_json::to_value(&dto).unwrap());
                }
            }
        }
    } else if added_location || meta_changed || dim_persisted {
        if let Some(dto) = ctx.index.get_dto(hash) {
            ItemEvents::publish_changed(&ctx.bus, &dto);
        }
    }

    // 缩略图是惰性缓存（读取端未命中时派发），入库/对账只保证调色板（颜色搜索依赖全量 palette）；
    // 派生齐备的文件(如对账扫描重放)不再派发:no-op 任务会把队列与积压计数灌满失真
    if needs_palette_work(ctx, hash) {
        ctx.worker.enqueue_palette(hash, &pending.abs_path);
    }

    // 扫描窗口内新增/刷新的位置记入会话：消失对账时豁免
    // （目录可能已被 runner 枚举过，窗口内后到的文件不在 seen 集内，否则会被误判消失）
    if let Some(session) = active_session(ctx) {
        session.touched.lock().unwrap().insert(pending.rel.clone());
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

/// 供 scan 模块访问（调色板持久化字段组装）
pub(crate) fn palette_to_entries(palette: &[PaletteColor]) -> Vec<PaletteEntry> {
    palette
        .iter()
        .map(|p| PaletteEntry {
            color: color_math::to_hex(p.r, p.g, p.b),
            percentage: p.percentage,
        })
        .collect()
}

//! 删除 / 移动 / 回收站：位置与元数据路径的变更、排序偏好跟随、事件转换。
//! 扫描窗口内的删除与移动记入会话簿记（invalidated/touched）：
//! 迟到的扫描结果（ScanFile）据此不复活已删位置；消失对账据此豁免窗口内新增的位置。

use crate::core::events::folder_changed_payload;
use crate::core::paths::LibraryPaths;
use crate::core::taxonomy::ItemEvents;
use std::collections::HashSet;
use std::sync::Arc;

use super::ctx::{active_session, PipelineCtx};
use super::upsert::do_upsert;

/// 按相对路径删除:同时按文件(精确)与目录(前缀)匹配,删除事件不区分两者
pub(crate) fn do_delete(ctx: &PipelineCtx, rel: &str) {
    // 目录(或其下的文件)删除:前缀范围内的 folder: 排序偏好一并清除。
    // 同目录下文件与文件夹不可同名,按前缀匹配不会误伤文件夹设置
    ctx.prefs.delete_prefix(rel);

    if let Some(hash) = ctx.index.remove_location(rel) {
        note_invalidated(ctx, rel);
        ItemEvents::publish_location_loss(&ctx.bus, &ctx.index, &hash);
    }

    for loc in ctx.index.locations_under(&format!("{rel}/")) {
        if let Some(hash) = ctx.index.remove_location(&loc) {
            note_invalidated(ctx, &loc);
            ItemEvents::publish_location_loss(&ctx.bus, &ctx.index, &hash);
        }
    }
}

pub(crate) fn do_move(ctx: &Arc<PipelineCtx>, old_abs: &str, new_abs: &str) -> Result<(), String> {
    let Some(old_rel) = ctx.paths.to_relative(old_abs) else {
        return Ok(());
    };
    let new_rel = ctx.paths.to_relative(new_abs);
    let new_usable = new_rel.as_ref().is_some_and(|r| {
        !LibraryPaths::is_internal(r) && (LibraryPaths::is_in_trash(r) || !ctx.config.is_ignored(r))
    });
    let Some(new_rel) = new_rel.filter(|_| new_usable) else {
        do_delete(ctx, &old_rel);
        return Ok(());
    };

    let Some(hash) = move_one(ctx, &old_rel, &new_rel)? else {
        // 旧位置未索引(例如改名发生在未索引文件上)→ 按新文件处理
        return do_upsert(ctx, new_abs, false, None, 0, false).map(|_| ());
    };

    ItemEvents::publish_transition(
        &ctx.bus,
        &ctx.index,
        &hash,
        LibraryPaths::is_in_trash(&old_rel),
        LibraryPaths::is_in_trash(&new_rel),
    );
    Ok(())
}

pub(crate) fn do_dir_move(ctx: &Arc<PipelineCtx>, old_abs: &str, new_abs: &str) -> Result<(), String> {
    let Some(old_rel) = ctx.paths.to_relative(old_abs) else {
        return Ok(());
    };
    let new_rel = ctx.paths.to_relative(new_abs);
    let new_usable = new_rel.as_ref().is_some_and(|r| {
        !LibraryPaths::is_internal(r) && (LibraryPaths::is_in_trash(&format!("{r}/")) || !ctx.config.is_ignored(r))
    });
    let Some(new_rel) = new_rel.filter(|_| new_usable) else {
        do_delete(ctx, &old_rel);
        return Ok(());
    };

    let mut affected: HashSet<String> = HashSet::new();
    for loc_path in ctx.index.locations_under(&format!("{old_rel}/")) {
        let new_loc_path = format!("{new_rel}{}", &loc_path[old_rel.len()..]);
        if let Some(hash) = move_one(ctx, &loc_path, &new_loc_path)? {
            affected.insert(hash);
        }
    }

    // 排序偏好跟随目录移动/重命名(含移入回收站,恢复时随之回归)
    ctx.prefs.rename_prefix(&old_rel, &new_rel);

    let old_in_trash = LibraryPaths::is_in_trash(&format!("{old_rel}/"));
    let new_in_trash = LibraryPaths::is_in_trash(&format!("{new_rel}/"));
    for hash in &affected {
        ItemEvents::publish_transition(&ctx.bus, &ctx.index, hash, old_in_trash, new_in_trash);
    }

    // 目录移动后目录结构必然变化,广播 folder.changed(folder/list 全量建树,客户端重拉即可)
    ctx.bus
        .publish(ItemEvents::FOLDER_CHANGED, folder_changed_payload(crate::core::events::REASON_EXTERNAL));

    // 目录下可能有监听遗漏的文件,补扫新位置
    if std::path::Path::new(new_abs).is_dir() {
        for file in ctx.scanner.walk_directory(new_abs) {
            do_upsert(ctx, &file, false, None, 0, false)?;
        }
    }
    Ok(())
}

/// 单个位置的移动:更新索引与元数据路径。返回是否命中已索引位置
fn move_one(ctx: &PipelineCtx, old_rel: &str, new_rel: &str) -> Result<Option<String>, String> {
    let Some(hash) = ctx.index.move_location(old_rel, new_rel) else {
        return Ok(None);
    };
    note_invalidated(ctx, old_rel);
    note_touched(ctx, new_rel);

    // lib→lib:元数据路径跟随;lib↔trash:去前缀后库内路径不变,元数据保持原路径(恢复目标)
    let old_lib = if LibraryPaths::is_in_trash(old_rel) {
        LibraryPaths::trash_to_library_path(old_rel)
    } else {
        old_rel
    };
    let new_lib = if LibraryPaths::is_in_trash(new_rel) {
        LibraryPaths::trash_to_library_path(new_rel)
    } else {
        new_rel
    };
    if old_lib != new_lib {
        if let Some(mut meta) = ctx.store.try_get(&hash) {
            if let Some(entry) = meta.find_path_mut(old_lib) {
                entry.path = new_lib.to_string();
                ctx.store.save(&hash, &meta)?;
            }
        }
    }
    Ok(Some(hash))
}

/// 清空回收站:清理位置与对应元数据、缩略图(库内仍有引用的内容除外)。物理删除由 API 层完成
pub(crate) fn do_clear_trash(ctx: &PipelineCtx) -> Result<(), String> {
    // 回收站内的 folder: 排序偏好随清空一并移除
    ctx.prefs
        .delete_prefix(&format!("{}/{}", LibraryPaths::HAWK_DIR_NAME, LibraryPaths::TRASH_DIR_NAME));

    for rel in ctx
        .index
        .all_location_paths()
        .into_iter()
        .filter(|p| LibraryPaths::is_in_trash(p))
        .collect::<Vec<_>>()
    {
        let Some(hash) = ctx.index.remove_location(&rel) else {
            continue;
        };
        note_invalidated(ctx, &rel);
        let lib_path = LibraryPaths::trash_to_library_path(&rel);
        let item_gone = !ctx.index.contains(&hash);
        if let Some(mut meta) = ctx.store.try_get(&hash) {
            meta.paths.retain(|p| p.path != lib_path);
            if meta.paths.is_empty() && item_gone {
                ctx.store.delete(&hash);
                ctx.thumbs.delete(&hash, &ctx.config.current().thumbnail_sizes);
            } else {
                ctx.store.save(&hash, &meta)?;
            }
        }
        if item_gone {
            ctx.bus.publish(ItemEvents::REMOVED, serde_json::json!({ "id": hash }));
        } else if let Some(dto) = ctx.index.get_dto(&hash) {
            ItemEvents::publish_changed(&ctx.bus, &dto);
        }
    }
    Ok(())
}

/// 窗口内被删除/移走的位置：迟到的扫描结果不复活
pub(crate) fn note_invalidated(ctx: &PipelineCtx, rel: &str) {
    if let Some(session) = active_session(ctx) {
        session.invalidated.lock().unwrap().insert(rel.to_string());
    }
}

/// 窗口内新增/刷新的位置：消失对账时豁免（apply_upsert 内统一记录，此为移动路径专用）
fn note_touched(ctx: &PipelineCtx, rel: &str) {
    if let Some(session) = active_session(ctx) {
        session.touched.lock().unwrap().insert(rel.to_string());
    }
}

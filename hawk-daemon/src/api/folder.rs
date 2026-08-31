//! folder 端点。folder 即素材库中的真实目录。操作直接作用于文件系统，索引由文件监听/流水线同步。

use crate::api::envelope::{success, ApiError, Envelope};
use crate::api::SharedState;
use crate::core::events::REASON_EXTERNAL;
use crate::core::fs_util;
use crate::core::index::ItemIndex;
use crate::core::paths::{unix_ms, LibraryPaths};
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use std::sync::Arc;

pub fn routes() -> Router<SharedState> {
    Router::new()
        .route("/api/v1/folder/list", get(folder_list))
        .route("/api/v1/folder/create", post(folder_create))
        .route("/api/v1/folder/update", post(folder_update))
        .route("/api/v1/folder/delete", post(folder_delete))
        .route("/api/v1/folder/restore", post(folder_restore))
}

#[derive(Serialize)]
struct FolderNode {
    path: String,
    name: String,
    children: Vec<FolderNode>,
    modification_time: i64,
    count: usize,
}

#[derive(serde::Deserialize)]
struct FolderCreateRequest {
    name: String,
    parent_path: Option<String>,
}

#[derive(serde::Deserialize)]
struct FolderUpdateRequest {
    path: String,
    name: Option<String>,
    parent_path: Option<String>,
}

#[derive(serde::Deserialize)]
struct FolderPathRequest {
    path: String,
}

/// 返回完整文件夹树（节点字段 path/name/children/modification_time/count）
async fn folder_list(State(state): State<SharedState>) -> Json<Envelope<FolderNode>> {
    Json(Envelope::ok(build_tree(&state.paths, &state.config, &state.index)))
}

async fn folder_create(
    State(state): State<SharedState>,
    Json(req): Json<FolderCreateRequest>,
) -> Result<Json<Envelope<FolderNode>>, ApiError> {
    if !fs_util::is_valid_name(Some(&req.name)) {
        return Err(ApiError::invalid_param(format!("非法文件夹名称: {}", req.name)));
    }
    let parent_rel = req.parent_path.unwrap_or_default();
    let parent_abs = resolve_existing_dir(&state.paths, &parent_rel)?;
    let target_abs = format!("{parent_abs}/{}", req.name);
    if std::path::Path::new(&target_abs).is_dir() {
        return Err(ApiError::file_exists(join_rel(&parent_rel, &req.name)));
    }
    std::fs::create_dir(&target_abs).map_err(|e| ApiError::internal(format!("创建目录失败: {e}")))?;
    // 目录结构变化广播(folder.changed):本端操作 + 其他客户端的 SSE 刷新统一走事件
    state.pipeline.notify_folder_changed(REASON_EXTERNAL);
    Ok(Json(Envelope::ok(to_node(
        &state.paths,
        &state.config,
        &state.index,
        &target_abs,
    ))))
}

async fn folder_update(
    State(state): State<SharedState>,
    Json(req): Json<FolderUpdateRequest>,
) -> Result<Json<Envelope<FolderNode>>, ApiError> {
    if !LibraryPaths::is_valid_library_path(Some(&req.path)) {
        return Err(ApiError::invalid_param(format!("非法文件夹路径: {}", req.path)));
    }
    let dir_abs = state.paths.to_absolute(&req.path).unwrap();
    if !std::path::Path::new(&dir_abs).is_dir() {
        return Err(ApiError::folder_not_found(&req.path));
    }

    let new_name = req
        .name
        .clone()
        .unwrap_or_else(|| req.path.rsplit('/').next().unwrap_or(&req.path).to_string());
    if !fs_util::is_valid_name(Some(&new_name)) {
        return Err(ApiError::invalid_param(format!("非法文件夹名称: {}", req.name.clone().unwrap_or_default())));
    }

    let new_parent_rel = req.parent_path.clone().unwrap_or_else(|| LibraryPaths::dir_of(&req.path).to_string());
    let new_parent_abs = resolve_existing_dir(&state.paths, &new_parent_rel)?;
    let target_rel = join_rel(&new_parent_rel, &new_name);
    if target_rel == req.path {
        return Ok(Json(Envelope::ok(to_node(
            &state.paths,
            &state.config,
            &state.index,
            &dir_abs,
        ))));
    }
    // 不允许移动到自身子目录
    if target_rel.starts_with(&format!("{}/", req.path)) {
        return Err(ApiError::invalid_param("不能移动到自身子目录"));
    }
    let target_abs = format!("{new_parent_abs}/{new_name}");
    if std::path::Path::new(&target_abs).exists() {
        return Err(ApiError::file_exists(&target_rel));
    }

    std::fs::rename(&dir_abs, &target_abs).map_err(|e| ApiError::internal(format!("移动目录失败: {e}")))?;
    state
        .pipeline
        .submit_dir_move(dir_abs.clone(), target_abs.clone())
        .await
        .map_err(ApiError::internal)?;
    // DirMoveJob 内会广播 folder.changed,此处不重复通知
    Ok(Json(Envelope::ok(to_node(
        &state.paths,
        &state.config,
        &state.index,
        &target_abs,
    ))))
}

/// 删除:整体移入 .hawk/trash/(保留目录结构)
async fn folder_delete(
    State(state): State<SharedState>,
    Json(req): Json<FolderPathRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !LibraryPaths::is_valid_library_path(Some(&req.path)) {
        return Err(ApiError::invalid_param(format!("非法文件夹路径: {}", req.path)));
    }
    let dir_abs = state.paths.to_absolute(&req.path).unwrap();
    if !std::path::Path::new(&dir_abs).is_dir() {
        return Err(ApiError::folder_not_found(&req.path));
    }

    let trash_abs = fs_util::find_free_trash_path(&state.paths, &req.path, true);
    fs_util::ensure_parent_dir(&trash_abs);
    std::fs::rename(&dir_abs, &trash_abs).map_err(|e| ApiError::internal(format!("移入回收站失败: {e}")))?;
    state
        .pipeline
        .submit_dir_move(dir_abs, trash_abs)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(success()))
}

/// 恢复：按原路径放回，被占用时报 FILE_EXISTS
async fn folder_restore(
    State(state): State<SharedState>,
    Json(req): Json<FolderPathRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !LibraryPaths::is_valid_library_path(Some(&req.path)) {
        return Err(ApiError::invalid_param(format!("非法文件夹路径: {}", req.path)));
    }
    let trash_abs = join(&state.paths.trash_dir, &req.path);
    if !std::path::Path::new(&trash_abs).is_dir() {
        return Err(ApiError::folder_not_found(&req.path));
    }
    let target_abs = state.paths.to_absolute(&req.path).unwrap();
    if std::path::Path::new(&target_abs).exists() {
        return Err(ApiError::file_exists(&req.path));
    }
    fs_util::ensure_parent_dir(&target_abs);
    std::fs::rename(&trash_abs, &target_abs).map_err(|e| ApiError::internal(format!("恢复目录失败: {e}")))?;
    state
        .pipeline
        .submit_dir_move(trash_abs, target_abs)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(success()))
}

/// 实时从文件系统构建文件夹树（排除 .hawk 与被 ignore 的目录），附库内 item 计数
fn build_tree(paths: &LibraryPaths, config: &Arc<crate::core::config::LibraryConfig>, index: &Arc<ItemIndex>) -> FolderNode {
    let counts = index.folder_counts();
    to_node_with_counts(paths, config, &counts, &paths.root.clone())
}

fn to_node(
    paths: &LibraryPaths,
    config: &Arc<crate::core::config::LibraryConfig>,
    index: &Arc<ItemIndex>,
    abs_dir: &str,
) -> FolderNode {
    let counts = index.folder_counts();
    to_node_with_counts(paths, config, &counts, abs_dir)
}

fn to_node_with_counts(
    paths: &LibraryPaths,
    config: &Arc<crate::core::config::LibraryConfig>,
    counts: &std::collections::HashMap<String, usize>,
    abs_dir: &str,
) -> FolderNode {
    let rel = paths.to_relative(abs_dir).unwrap_or_default();
    let is_root = rel.is_empty();
    let name = if is_root {
        abs_dir.trim_end_matches('/').rsplit('/').next().unwrap_or(abs_dir).to_string()
    } else {
        rel.rsplit('/').next().unwrap_or(&rel).to_string()
    };

    let mut children = Vec::new();
    if let Ok(entries) = std::fs::read_dir(abs_dir) {
        let mut dirs: Vec<_> = entries
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter(|e| {
                let child_name = e.file_name().to_string_lossy().to_string();
                if is_root && child_name == LibraryPaths::HAWK_DIR_NAME {
                    return false;
                }
                let child_rel = if is_root {
                    child_name.clone()
                } else {
                    format!("{rel}/{child_name}")
                };
                !config.is_ignored(&child_rel)
            })
            .collect();
        dirs.sort_by_key(|e| e.file_name().to_string_lossy().to_lowercase());
        for d in dirs {
            let child_abs = d.path().to_string_lossy().replace('\\', "/");
            children.push(to_node_with_counts(paths, config, counts, &child_abs));
        }
    }

    let modification_time = std::fs::metadata(abs_dir)
        .and_then(|m| m.modified())
        .map(unix_ms)
        .unwrap_or(0);
    let count = counts.get(&rel).copied().unwrap_or(0);
    FolderNode {
        path: rel,
        name,
        children,
        modification_time,
        count,
    }
}

/// 解析父目录：缺省为库根目录；必须已存在
fn resolve_existing_dir(paths: &LibraryPaths, rel: &str) -> Result<String, ApiError> {
    if rel.is_empty() {
        return Ok(paths.root.clone());
    }
    if !LibraryPaths::is_valid_library_path(Some(rel)) {
        return Err(ApiError::invalid_param(format!("非法文件夹路径: {rel}")));
    }
    let abs = paths.to_absolute(rel).unwrap();
    if !std::path::Path::new(&abs).is_dir() {
        return Err(ApiError::folder_not_found(rel));
    }
    Ok(abs)
}

fn join_rel(parent_rel: &str, name: &str) -> String {
    if parent_rel.is_empty() {
        name.to_string()
    } else {
        format!("{parent_rel}/{name}")
    }
}

fn join(base: &str, child: &str) -> String {
    if base.ends_with('/') {
        format!("{base}{child}")
    } else {
        format!("{base}/{child}")
    }
}

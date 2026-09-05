//! folder 端点。folder 即素材库中的真实目录。操作直接作用于文件系统，索引由文件监听/流水线同步。

use crate::api::envelope::{success, ApiError, Envelope, SuccessOnly};
use crate::api::SharedState;
use crate::core::events::{EventBus, REASON_EXTERNAL};
use crate::core::fs_util;
use crate::core::index::ItemIndex;
use crate::core::paths::{unix_ms, LibraryPaths};
use crate::core::taxonomy::ItemEvents;
use axum::extract::State;
use axum::Json;
use serde::Serialize;
use std::sync::{Arc, RwLock};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn routes() -> OpenApiRouter<SharedState> {
    OpenApiRouter::new()
        .routes(routes!(folder_list))
        .routes(routes!(folder_create))
        .routes(routes!(folder_update))
        .routes(routes!(folder_delete))
        .routes(routes!(folder_restore))
}

#[derive(Clone, Serialize, utoipa::ToSchema)]
pub(crate) struct FolderNode {
    path: String,
    name: String,
    /// 子目录（自引用；no_recursion 生成 $ref 切断内联递归）
    #[schema(no_recursion)]
    children: Vec<FolderNode>,
    modification_time: i64,
    count: usize,
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
struct FolderCreateRequest {
    name: String,
    parent_path: Option<String>,
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
struct FolderUpdateRequest {
    path: String,
    name: Option<String>,
    parent_path: Option<String>,
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
struct FolderPathRequest {
    path: String,
}

/// 完整文件夹树（目录结构缓存 + 实时计数合并；count 含祖先目录）
#[utoipa::path(
    get,
    path = "/api/v1/folder/list",
    tags = ["folder"],
    responses((status = 200, description = "OK", body = Envelope<FolderNode>))
)]
async fn folder_list(State(state): State<SharedState>) -> Json<Envelope<FolderNode>> {
    let mut root = state.folder_tree.get_or_build(&state.paths, &state.config);
    fill_counts(&mut root, &state.index.folder_counts());
    Json(Envelope::ok(root))
}

/// 新建目录（目标已存在报 FILE_EXISTS）；目录结构变化经 folder.changed 广播
#[utoipa::path(
    post,
    path = "/api/v1/folder/create",
    tags = ["folder"],
    request_body = FolderCreateRequest,
    responses((status = 200, description = "OK", body = Envelope<FolderNode>))
)]
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
    state.folder_tree.invalidate();
    // 目录结构变化广播(folder.changed):本端操作 + 其他客户端的 SSE 刷新统一走事件
    state.pipeline.notify_folder_changed(REASON_EXTERNAL);
    Ok(Json(Envelope::ok(to_node(
        &state.paths,
        &state.config,
        &state.index,
        &target_abs,
    ))))
}

/// 重命名/移动目录（禁止移入自身子目录；级联迁移经 DirMoveJob 由流水线执行）
#[utoipa::path(
    post,
    path = "/api/v1/folder/update",
    tags = ["folder"],
    request_body = FolderUpdateRequest,
    responses((status = 200, description = "OK", body = Envelope<FolderNode>))
)]
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
    state.folder_tree.invalidate();
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
#[utoipa::path(
    post,
    path = "/api/v1/folder/delete",
    tags = ["folder"],
    request_body = FolderPathRequest,
    responses((status = 200, description = "OK", body = SuccessOnly))
)]
async fn folder_delete(
    State(state): State<SharedState>,
    Json(req): Json<FolderPathRequest>,
) -> Result<Json<SuccessOnly>, ApiError> {
    if !LibraryPaths::is_valid_library_path(Some(&req.path)) {
        return Err(ApiError::invalid_param(format!("非法文件夹路径: {}", req.path)));
    }
    let dir_abs = state.paths.to_absolute(&req.path).unwrap();
    if !std::path::Path::new(&dir_abs).is_dir() {
        // 目录已被外部删除：按幂等删除处理——流水线按前缀清掉索引位置与目录设置残留，
        // 广播树变化让各客户端刷新，返回成功（删除的目标已达成，不报错）
        state.folder_tree.invalidate();
        state.pipeline.notify_deleted(dir_abs);
        state.pipeline.notify_folder_changed(REASON_EXTERNAL);
        return Ok(success());
    }

    let trash_abs = fs_util::find_free_trash_path(&state.paths, &req.path, true);
    fs_util::ensure_parent_dir(&trash_abs);
    std::fs::rename(&dir_abs, &trash_abs).map_err(|e| ApiError::internal(format!("移入回收站失败: {e}")))?;
    state.folder_tree.invalidate();
    state
        .pipeline
        .submit_dir_move(dir_abs, trash_abs)
        .await
        .map_err(ApiError::internal)?;
    Ok(success())
}

/// 恢复：按原路径放回，被占用时报 FILE_EXISTS
#[utoipa::path(
    post,
    path = "/api/v1/folder/restore",
    tags = ["folder"],
    request_body = FolderPathRequest,
    responses((status = 200, description = "OK", body = SuccessOnly))
)]
async fn folder_restore(
    State(state): State<SharedState>,
    Json(req): Json<FolderPathRequest>,
) -> Result<Json<SuccessOnly>, ApiError> {
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
    state.folder_tree.invalidate();
    state
        .pipeline
        .submit_dir_move(trash_abs, target_abs)
        .await
        .map_err(ApiError::internal)?;
    Ok(success())
}

/// 目录结构缓存：递归 read_dir 建树是 folder/list 唯一的重活（计数为内存合并，极轻）。
/// 缓存树 count 恒为 0，serve 时以索引实时计数覆写；失效与 folder.changed 同线——
/// 一切目录结构变化（API 操作/外部进程/对账扫描兜底）最终都会广播该事件，
/// 缓存新鲜度与引入缓存前「客户端收事件重拉」的保证一致。config ignore 变更同样失效。
pub struct FolderTreeCache {
    inner: RwLock<CacheInner>,
}

struct CacheInner {
    tree: Option<FolderNode>,
    generation: u64,
}

impl FolderTreeCache {
    pub fn new() -> FolderTreeCache {
        FolderTreeCache {
            inner: RwLock::new(CacheInner { tree: None, generation: 0 }),
        }
    }

    /// 目录结构可能已变化 → 丢弃缓存（下一次 get_or_build 重建）
    pub fn invalidate(&self) {
        let mut inner = self.inner.write().unwrap();
        inner.tree = None;
        inner.generation += 1;
    }

    /// 路径是否为缓存树中的已知目录（外部空目录删除检测用；缓存冷时返回 false——
    /// 下次建树即最新状态，无陈旧残留风险）
    pub fn known_dir(&self, rel: &str) -> bool {
        let inner = self.inner.read().unwrap();
        match &inner.tree {
            Some(tree) => node_contains(tree, rel),
            None => false,
        }
    }

    /// 订阅事件总线：folder.changed 到达即失效（bootstrap 与测试装配共用）。
    /// 覆盖外部进程改动与扫描兜底；API 写操作由端点内同步失效（响应即最新，不等事件调度）
    pub fn spawn_invalidation(self: &Arc<Self>, bus: &EventBus) {
        let cache = Arc::clone(self);
        let mut rx = bus.subscribe();
        tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                if event.kind == ItemEvents::FOLDER_CHANGED {
                    cache.invalidate();
                }
            }
        });
    }

    /// 取目录结构树（count 未填）：命中返回克隆；未命中现建并回填。
    /// 建树在锁外进行（IO 不堵失效/并发读）；建树期间发生失效则本次结果不回填缓存
    pub fn get_or_build(&self, paths: &LibraryPaths, config: &Arc<crate::core::config::LibraryConfig>) -> FolderNode {
        if let Some(tree) = &self.inner.read().unwrap().tree {
            return clone_node(tree);
        }
        let generation = self.inner.read().unwrap().generation;
        let built = structural_node(paths, config, &paths.root.clone());
        let mut inner = self.inner.write().unwrap();
        if inner.generation == generation {
            inner.tree = Some(clone_node(&built));
        }
        built
    }
}

fn node_contains(node: &FolderNode, rel: &str) -> bool {
    node.path == rel || node.children.iter().any(|c| node_contains(c, rel))
}

fn clone_node(node: &FolderNode) -> FolderNode {
    FolderNode {
        path: node.path.clone(),
        name: node.name.clone(),
        children: node.children.iter().map(clone_node).collect(),
        modification_time: node.modification_time,
        count: node.count,
    }
}

/// 索引实时计数覆写（树为克隆体，原地填 count）
fn fill_counts(node: &mut FolderNode, counts: &std::collections::HashMap<String, usize>) {
    node.count = counts.get(&node.path).copied().unwrap_or(0);
    for child in &mut node.children {
        fill_counts(child, counts);
    }
}

fn to_node(
    paths: &LibraryPaths,
    config: &Arc<crate::core::config::LibraryConfig>,
    index: &Arc<ItemIndex>,
    abs_dir: &str,
) -> FolderNode {
    // 操作响应现建现计（不经过缓存：操作刚落盘，保证响应即最新）
    let mut node = structural_node(paths, config, abs_dir);
    fill_counts(&mut node, &index.folder_counts());
    node
}

/// 实时从文件系统递归建树（排除 .hawk 与被 ignore 的目录）；count 恒 0，由 fill_counts 覆写
fn structural_node(
    paths: &LibraryPaths,
    config: &Arc<crate::core::config::LibraryConfig>,
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
            children.push(structural_node(paths, config, &child_abs));
        }
    }

    let modification_time = std::fs::metadata(abs_dir)
        .and_then(|m| m.modified())
        .map(unix_ms)
        .unwrap_or(0);
    FolderNode {
        path: rel,
        name,
        children,
        modification_time,
        count: 0,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// known_dir：缓存冷/暖/失效三态（外部空目录删除检测的参照）
    #[test]
    fn known_dir_tracks_cached_tree() {
        let dir = std::env::temp_dir().join(format!("hawk-foldertree-test-{}", std::process::id()));
        let root = dir.join("lib");
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        let paths = LibraryPaths::new(root.to_str().unwrap(), None);
        paths.ensure_layout();
        let config = Arc::new(crate::core::config::LibraryConfig::new(paths.clone()));
        let cache = FolderTreeCache::new();

        // 冷缓存：一律 false（下次建树即最新，无陈旧风险）
        assert!(!cache.known_dir("a"));

        // 建树后：命中已知目录（含多级），未知路径 false
        let tree = cache.get_or_build(&paths, &config);
        assert!(tree.children.iter().any(|c| c.path == "a"));
        assert!(cache.known_dir("a"));
        assert!(cache.known_dir("a/b"));
        assert!(!cache.known_dir("a/c"));
        assert!(!cache.known_dir("zz"));

        // 失效后回到冷态
        cache.invalidate();
        assert!(!cache.known_dir("a"));

        // 磁盘变化后重建反映最新结构
        std::fs::create_dir_all(root.join("c")).unwrap();
        cache.get_or_build(&paths, &config);
        assert!(cache.known_dir("c"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! library 端点：info / reindex / rescan / refresh_cache

use crate::api::envelope::{success, ApiError, Envelope, JsonBody, SuccessOnly};
use crate::api::SharedState;
use crate::core::events::LibraryEvents;
use crate::core::index::RefreshScope;
use crate::core::paths::unix_ms;
use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn routes() -> OpenApiRouter<SharedState> {
    OpenApiRouter::new()
        .routes(routes!(library_info))
        .routes(routes!(library_update))
        .routes(routes!(storage_mode_set))
        .routes(routes!(reindex))
        .routes(routes!(rescan))
        .routes(routes!(refresh_cache))
}

#[derive(Serialize, utoipa::ToSchema)]
pub(crate) struct LibraryInfo {
    name: String,
    path: String,
    modification_time: i64,
    application_version: &'static str,
    /// 元数据存储方案：database（.hawk/metadata.db）/ toml（.hawk/metadata/*.toml，网盘同步友好）
    storage_mode: &'static str,
}

/// 库信息：显示名取 config 的 name，缺省目录名
#[utoipa::path(
    get,
    path = "/api/v1/library/info",
    tags = ["library"],
    responses((status = 200, description = "OK", body = Envelope<LibraryInfo>))
)]
async fn library_info(State(state): State<SharedState>) -> Json<Envelope<LibraryInfo>> {
    Json(Envelope::ok(build_library_info(&state)))
}

/// 组装库信息（GET 响应与改名后回传共用；显示名每次读 config 快照）
fn build_library_info(state: &SharedState) -> LibraryInfo {
    let root = &state.paths.root;
    let name = state.config.current().name.clone().unwrap_or_else(|| {
        root.trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(root)
            .to_string()
    });
    let modification_time = std::fs::metadata(root)
        .and_then(|m| m.modified())
        .map(unix_ms)
        .unwrap_or(0);
    LibraryInfo {
        name,
        path: root.clone(),
        modification_time,
        application_version: env!("CARGO_PKG_VERSION"),
        storage_mode: match state.store.mode() {
            crate::core::metadata_store::StorageMode::Db => "database",
            crate::core::metadata_store::StorageMode::Toml => "toml",
        },
    }
}

#[derive(Deserialize, utoipa::ToSchema)]
struct StorageModeBody {
    /// database | toml
    mode: String,
}

/// 切换元数据存储方案：单写者内完成全量迁移（写新权威层 + 删旧文件），成功后调用方应重启进程
/// （打开库时按内容探测模式，见 metadata_store::detect_storage_mode）。已是目标模式时幂等成功
#[utoipa::path(
    post,
    path = "/api/v1/library/storage_mode",
    tags = ["library"],
    request_body = StorageModeBody,
    responses((status = 200, description = "OK", body = SuccessOnly))
)]
async fn storage_mode_set(
    State(state): State<SharedState>,
    JsonBody(body): JsonBody<StorageModeBody>,
) -> Result<Json<SuccessOnly>, ApiError> {
    let target = match body.mode.as_str() {
        "database" => crate::core::metadata_store::StorageMode::Db,
        "toml" => crate::core::metadata_store::StorageMode::Toml,
        other => return Err(ApiError::invalid_param(format!("非法存储方案: {other}（支持 database/toml）"))),
    };
    if state.store.mode() != target {
        state.pipeline.submit_storage_migrate(target).await.map_err(ApiError::internal)?;
    }
    Ok(success())
}

#[derive(Deserialize, utoipa::ToSchema)]
struct LibraryRenameBody {
    /// 新显示名；空白清除自定义名（回退库目录名）
    name: String,
}

/// 改库显示名：写库内 .hawk/config.toml 的 name 键（toml_edit 保注释，保存即热更）；
/// 只读 viewer 在 auth 层被拒（非 GET 且不在查询白名单）
#[utoipa::path(
    patch,
    path = "/api/v1/library/info",
    tags = ["library"],
    request_body = LibraryRenameBody,
    responses((status = 200, description = "OK", body = Envelope<LibraryInfo>))
)]
async fn library_update(
    State(state): State<SharedState>,
    JsonBody(body): JsonBody<LibraryRenameBody>,
) -> Result<Json<Envelope<LibraryInfo>>, ApiError> {
    state
        .config
        .update_name(Some(&body.name))
        .map_err(ApiError::internal)?;
    // 广播库信息（含新显示名）：所有连接的客户端（含 LAN 浏览器）就地对齐，无需重拉
    let info = build_library_info(&state);
    state
        .bus
        .publish(LibraryEvents::UPDATED, serde_json::to_value(&info).unwrap());
    Ok(Json(Envelope::ok(info)))
}

/// 全量重建索引：重算全部哈希，异步执行，立即返回
#[utoipa::path(
    post,
    path = "/api/v1/library/reindex",
    tags = ["library"],
    responses((status = 200, description = "OK", body = SuccessOnly))
)]
async fn reindex(State(state): State<SharedState>) -> Json<SuccessOnly> {
    state.pipeline.request_scan(true);
    success()
}

/// 刷新缓存：忽略快照强制遍历全部文件做复用判定（不读文件内容）。异步执行，立即返回
#[utoipa::path(
    post,
    path = "/api/v1/library/rescan",
    tags = ["library"],
    responses((status = 200, description = "OK", body = SuccessOnly))
)]
async fn rescan(State(state): State<SharedState>) -> Json<SuccessOnly> {
    state.pipeline.request_rescan();
    success()
}

#[derive(Deserialize, utoipa::ToSchema)]
struct RefreshCacheRequest {
    /// folder | category | tag | library
    #[serde(rename = "type")]
    scope_type: String,
    /// folder/category/tag 的名称（folder 为目录相对路径，空串 = 库根）；library 时忽略
    value: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
struct RefreshCacheResponse {
    /// 实际入队的修复任务数（in-flight 去重丢弃或源文件不在的不计）
    dispatched: usize,
    /// 消失对账移除的失效位置数（源文件已删但索引残留的卡片，经 SSE 推送收敛）
    removed: usize,
}

/// 范围内消失对账：索引位置的源文件明确不存在（NotFound）时移除——
/// watcher 删除事件丢失（网络盘/外接盘常见）时该位置会永远残留，本对账是手动收敛入口。
/// IO/权限错误（网络盘瞬断等）保守保留，不误删。移除走流水线单写者（notify_deleted → 索引摘除 + 事件广播）
async fn reconcile_scope_missing(state: &SharedState, scope: &RefreshScope) -> usize {
    // 范围内库内位置快照（folder 前缀含子目录，空串 = 全库；category/tag 按成员位置；library 含回收站）
    let rels: Vec<String> = match scope {
        RefreshScope::Folder(f) => {
            let prefix = if f.is_empty() { String::new() } else { format!("{f}/") };
            state.index.locations_under(&prefix)
        }
        RefreshScope::Category(_) | RefreshScope::Tag(_) => state
            .index
            .hashes_in_scope(scope)
            .into_iter()
            .flat_map(|h| state.index.item_locations(&h, Some(false)))
            .map(|l| l.path)
            .collect(),
        RefreshScope::Library => state.index.all_location_paths(),
    };
    let abs_list: Vec<String> = rels
        .iter()
        .filter_map(|rel| state.paths.to_absolute(rel))
        .collect();
    // stat 批量检查移出运行时线程（万级位置的元数据调用也是毫秒级，但不阻塞 reactor）
    let missing = tokio::task::spawn_blocking(move || {
        abs_list
            .into_iter()
            .filter(|abs| matches!(std::fs::metadata(abs), Err(e) if e.kind() == std::io::ErrorKind::NotFound))
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    let removed = missing.len();
    for abs in missing {
        state.pipeline.notify_deleted(abs);
    }
    removed
}

/// 按范围刷新派生缓存（补缺失模式）：对范围内全部 item 派发修复任务——
/// 补缺失宽高（0 × 0）+ 生成缺失尺寸缩略图 + 提炼缺失调色板，不重建已有文件。
/// 附带消失对账：范围内源文件已删除但索引残留的位置会被移除（watcher 漏事件的收敛入口）。
/// 用户遇到显示异常时的手动修复入口；异步执行立即返回，积压经 task.progress(thumbnail) 可见
#[utoipa::path(
    post,
    path = "/api/v1/library/refresh_cache",
    tags = ["library"],
    request_body = RefreshCacheRequest,
    responses((status = 200, description = "OK", body = Envelope<RefreshCacheResponse>))
)]
async fn refresh_cache(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<RefreshCacheRequest>,
) -> Result<Json<Envelope<RefreshCacheResponse>>, ApiError> {
    let scope = match req.scope_type.as_str() {
        "library" => RefreshScope::Library,
        "folder" => RefreshScope::Folder(req.value.unwrap_or_default()),
        "category" => match req.value.filter(|v| !v.is_empty()) {
            Some(v) => RefreshScope::Category(v),
            None => return Err(ApiError::invalid_param("category 需提供 value")),
        },
        "tag" => match req.value.filter(|v| !v.is_empty()) {
            Some(v) => RefreshScope::Tag(v),
            None => return Err(ApiError::invalid_param("tag 需提供 value")),
        },
        other => return Err(ApiError::invalid_param(format!("未知范围类型: {other}"))),
    };
    // 先消失对账（源文件已删的失效位置移除），再对存活项派发修复
    let removed = reconcile_scope_missing(&state, &scope).await;
    let hashes = state.index.hashes_in_scope(&scope);
    let mut dispatched = 0;
    for hash in &hashes {
        if let Some(abs) = state.index.main_source_abs(hash, &state.paths) {
            if state.worker.enqueue_thumbs(hash, &abs) {
                dispatched += 1;
            }
        }
    }
    Ok(Json(Envelope::ok(RefreshCacheResponse { dispatched, removed })))
}

//! global_filter 端点：全局列表隐藏项（文件夹/分类/标签）的查询与标记。
//! 隐藏集存于 .hawk/global_filter.toml（参与同步）；纯注册表读写（同 view 端点，不经过
//! 索引流水线）。变更广播 global_filter.changed（负载为完整快照），客户端据此重查列表。
//! 级联跟随（文件夹移动/删除、分类/标签改名删除）在索引流水线对应 Job 内完成。

use crate::api::envelope::{success, ApiError, Envelope, JsonBody, SuccessOnly};
use crate::api::SharedState;
use crate::core::global_filter::{publish_changed, GlobalFilterSnapshot};
use crate::core::paths::LibraryPaths;
use crate::core::taxonomy::normalize_category_name;
use axum::extract::State;
use axum::Json;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn routes() -> OpenApiRouter<SharedState> {
    OpenApiRouter::new()
        .routes(routes!(global_filter_list))
        .routes(routes!(global_filter_put))
}

/// 全部隐藏项（文件夹为库内相对路径，子树整体隐藏）
#[utoipa::path(
    get,
    path = "/api/v1/global_filter/list",
    tags = ["global_filter"],
    responses((status = 200, description = "OK", body = Envelope<GlobalFilterSnapshot>))
)]
async fn global_filter_list(State(state): State<SharedState>) -> Json<Envelope<GlobalFilterSnapshot>> {
    Json(Envelope::ok(state.global_filter.snapshot()))
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
struct GlobalFilterPutRequest {
    /// 维度：folder / category / tag
    kind: String,
    /// folder 为库内相对路径（如 "posters/2024"）；category/tag 为名称
    name: String,
    hidden: bool,
}

/// 标记/取消单个维度的全局列表隐藏（幂等；无变化时同样返回成功）
#[utoipa::path(
    put,
    path = "/api/v1/global_filter",
    tags = ["global_filter"],
    request_body = GlobalFilterPutRequest,
    responses((status = 200, description = "OK", body = SuccessOnly))
)]
async fn global_filter_put(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<GlobalFilterPutRequest>,
) -> Result<Json<SuccessOnly>, ApiError> {
    let changed = match req.kind.as_str() {
        "folder" => {
            if req.name.is_empty() || !LibraryPaths::is_valid_library_path(Some(&req.name)) {
                return Err(ApiError::invalid_param(format!("非法文件夹路径: {}", req.name)));
            }
            state.global_filter.set_folder_hidden(&req.name, req.hidden)
        }
        "category" => {
            let name = normalize_category_name(Some(&req.name))
                .ok_or_else(|| ApiError::invalid_param(format!("非法分类名称: {}", req.name)))?;
            state.global_filter.set_category_hidden(&name, req.hidden)
        }
        "tag" => {
            let name = req.name.trim();
            if name.is_empty() {
                return Err(ApiError::invalid_param("标签名称不能为空"));
            }
            state.global_filter.set_tag_hidden(name, req.hidden)
        }
        other => return Err(ApiError::invalid_param(format!("非法维度: {other}（支持 folder/category/tag）"))),
    };
    if changed {
        publish_changed(&state.bus, &state.global_filter.snapshot());
    }
    Ok(success())
}

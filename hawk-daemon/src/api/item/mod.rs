//! item 端点（14 个），按子域拆分文件，本文件聚合路由与公共辅助。
//! 写路径的真实文件操作在本层完成，随后提交索引流水线并等待完成；读取一律走索引锁内投影。

mod add;
mod delete;
mod file;
mod query;
mod replace;
mod update;
mod upload;

use crate::api::envelope::{ApiError, Envelope};
use crate::api::SharedState;
use crate::core::index::LocationSnapshot;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

// ---------- 子模块共用导入面（各文件 `use super::*` 即可取到） ----------
pub(crate) use crate::api::envelope::{success, JsonBody, SuccessOnly};
pub(crate) use crate::core::content_hash;
pub(crate) use crate::core::fs_util;
pub(crate) use crate::core::item::{ItemDto, ItemQuery, ItemSkeletonDto};
pub(crate) use crate::core::paths::LibraryPaths;
pub(crate) use crate::core::thumbnail::ThumbnailService;
pub(crate) use axum::extract::{Query, State};
pub(crate) use axum::response::Response;
pub(crate) use axum::Json;
pub(crate) use serde::{Deserialize, Serialize};

pub fn routes() -> OpenApiRouter<SharedState> {
    OpenApiRouter::new()
        .routes(routes!(query::item_list))
        .routes(routes!(query::item_skeleton))
        .routes(routes!(query::item_detail))
        .routes(routes!(query::item_count))
        .routes(routes!(add::item_add))
        .routes(routes!(upload::item_upload))
        .routes(routes!(update::item_update))
        .routes(routes!(update::item_batch_update))
        .routes(routes!(delete::item_delete))
        .routes(routes!(delete::item_restore))
        .routes(routes!(file::item_thumbnail))
        .routes(routes!(file::item_file))
        .routes(routes!(file::item_refresh_thumbnail))
        .routes(routes!(replace::item_replace))
}

// ---------- 公共类型与辅助 ----------

/// id 查询参数（detail/file 共用）；path 定位同内容多位置中的具体条目（detail 用）
#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct IdQuery {
    /// item id（内容 BLAKE3 哈希 hex）
    pub(crate) id: String,
    /// 可选：库内相对位置（缺省为主位置）
    pub(crate) path: Option<String>,
}

pub(crate) use add::ItemAddResponse;




fn find_location(
    state: &SharedState,
    id: &str,
    path: Option<&str>,
    want_trash: Option<bool>,
) -> Result<LocationSnapshot, ApiError> {
    state
        .index
        .find_location(id, path, want_trash)
        .ok_or_else(|| ApiError::item_not_found(path.unwrap_or(id)))
}

/// img_base64 解码（add 的 base64 导入与 replace 共用）
pub(crate) fn decode_base64(input: &str) -> Result<Vec<u8>, ApiError> {
    use base64::Engine;
    let cleaned: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(cleaned.as_bytes())
        .map_err(|_| ApiError::invalid_param("img_base64 不是合法的 Base64 数据"))
}

/// 分类名校验归一 + 去重（add/update/batch_update 共用）
pub(crate) fn normalize_categories(raw: Option<&[String]>) -> Result<Option<Vec<String>>, ApiError> {
    let Some(raw) = raw else { return Ok(None) };
    let mut out = Vec::new();
    for name in raw {
        let normalized = crate::core::taxonomy::normalize_category_name(Some(name))
            .ok_or_else(|| ApiError::invalid_param("包含非法分类名称"))?;
        if !out.contains(&normalized) {
            out.push(normalized);
        }
    }
    Ok(Some(out))
}

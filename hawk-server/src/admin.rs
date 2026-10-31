//! 管理面端点（租户管理）：仅环回监听（127.0.0.1:<admin-port>，默认 9110），与主端口分离。
//! 信任模型：环回访问权 ≈ 本机 shell 与数据目录访问权（同级信任，不再引入第三把凭证）；
//! 反代/防火墙不得暴露该端口。远程管理走 SSH 端口转发（`ssh -L 9110:127.0.0.1:9110`）。
//! 与 CLI 子命令（tenant add/list/revoke）写同一注册表（文件即事实源，两条路径即时互见）。

use crate::api::{ok, ApiError, JsonBody};
use crate::keystore::KeyStore;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn router(keys: Arc<KeyStore>) -> Router {
    Router::new()
        .route(
            "/api/v1/admin/tenants",
            post(create_tenant).get(list_tenants),
        )
        .route("/api/v1/admin/tenants/{id}", delete(revoke_tenant))
        .with_state(keys)
}

#[derive(Deserialize)]
struct CreateTenantBody {
    /// 租户标签（仅运维识别用）
    label: String,
}

/// `POST /api/v1/admin/tenants`：创建租户；明文 tenant_key 只在本次响应出现（不落盘）
async fn create_tenant(
    State(keys): State<Arc<KeyStore>>,
    JsonBody(body): JsonBody<CreateTenantBody>,
) -> Result<Json<Value>, ApiError> {
    let label = body.label.trim();
    if label.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "label 不能为空"));
    }
    let (id, tenant_key) = keys.tenant_add(label).map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("租户创建失败: {e}"),
        )
    })?;
    Ok(ok(
        json!({ "id": id, "label": label, "tenant_key": tenant_key }),
    ))
}

/// `GET /api/v1/admin/tenants`：列出租户（密钥本体不可恢复，只回 id/标签/创建时间）
async fn list_tenants(State(keys): State<Arc<KeyStore>>) -> Json<Value> {
    let tenants: Vec<Value> = keys
        .tenant_list()
        .iter()
        .map(|t| json!({ "id": t.id, "label": t.label, "created_at_ms": t.created_at_ms }))
        .collect();
    ok(json!({ "tenants": tenants }))
}

/// `DELETE /api/v1/admin/tenants/{id}`：吊销租户（TenantAdmin/Device Session 下一请求失效）
async fn revoke_tenant(
    State(keys): State<Arc<KeyStore>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let revoked = keys.tenant_revoke(&id).map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("租户吊销失败: {e}"),
        )
    })?;
    if !revoked {
        return Err(ApiError::not_found("租户不存在"));
    }
    Ok(crate::api::success())
}

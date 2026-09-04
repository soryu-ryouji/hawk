//! 统一信封（`{status, data}` / 错误信封）、API 错误码、ApiError → 响应的转换。
//! REST 信封与错误码（错误码集合即 REST 契约）。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// API 错误码（见 docs/backend/server-rest-api-v1.md）
pub mod codes {
    pub const INVALID_PARAM: &str = "INVALID_PARAM";
    pub const ITEM_NOT_FOUND: &str = "ITEM_NOT_FOUND";
    pub const FOLDER_NOT_FOUND: &str = "FOLDER_NOT_FOUND";
    pub const FILE_EXISTS: &str = "FILE_EXISTS";
    pub const UNSUPPORTED_FORMAT: &str = "UNSUPPORTED_FORMAT";
    pub const CATEGORY_NOT_FOUND: &str = "CATEGORY_NOT_FOUND";
    pub const CATEGORY_EXISTS: &str = "CATEGORY_EXISTS";
    pub const TAG_NOT_FOUND: &str = "TAG_NOT_FOUND";
    pub const NOT_READY: &str = "NOT_READY";
    pub const INTERNAL: &str = "INTERNAL";
    pub const UNAUTHORIZED: &str = "UNAUTHORIZED";
    pub const READ_ONLY: &str = "READ_ONLY";
}

/// 携带错误码与 HTTP 状态的业务错误，由 IntoResponse 统一转为错误信封
#[derive(Debug)]
pub struct ApiError {
    pub code: &'static str,
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn new(code: &'static str, status: StatusCode, message: impl Into<String>) -> ApiError {
        ApiError {
            code,
            status,
            message: message.into(),
        }
    }

    pub fn invalid_param(message: impl Into<String>) -> ApiError {
        ApiError::new(codes::INVALID_PARAM, StatusCode::BAD_REQUEST, message)
    }

    pub fn item_not_found(id: impl AsRef<str>) -> ApiError {
        ApiError::new(
            codes::ITEM_NOT_FOUND,
            StatusCode::NOT_FOUND,
            format!("item {} not found", id.as_ref()),
        )
    }

    pub fn folder_not_found(path: impl AsRef<str>) -> ApiError {
        ApiError::new(
            codes::FOLDER_NOT_FOUND,
            StatusCode::NOT_FOUND,
            format!("folder {} not found", path.as_ref()),
        )
    }

    pub fn file_exists(path: impl AsRef<str>) -> ApiError {
        ApiError::new(
            codes::FILE_EXISTS,
            StatusCode::CONFLICT,
            format!("file already exists: {}", path.as_ref()),
        )
    }

    pub fn unsupported_format(message: impl Into<String>) -> ApiError {
        ApiError::new(codes::UNSUPPORTED_FORMAT, StatusCode::BAD_REQUEST, message)
    }

    pub fn category_not_found(name: impl AsRef<str>) -> ApiError {
        ApiError::new(
            codes::CATEGORY_NOT_FOUND,
            StatusCode::NOT_FOUND,
            format!("category {} not found", name.as_ref()),
        )
    }

    pub fn category_exists(name: impl AsRef<str>) -> ApiError {
        ApiError::new(
            codes::CATEGORY_EXISTS,
            StatusCode::CONFLICT,
            format!("category already exists: {}", name.as_ref()),
        )
    }

    pub fn tag_not_found(name: impl AsRef<str>) -> ApiError {
        ApiError::new(
            codes::TAG_NOT_FOUND,
            StatusCode::NOT_FOUND,
            format!("tag {} not found", name.as_ref()),
        )
    }

    pub fn internal(message: impl Into<String>) -> ApiError {
        ApiError::new(codes::INTERNAL, StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}

/// 统一成功信封；data 为空时省略该字段
#[derive(Serialize, utoipa::ToSchema)]
pub struct Envelope<T: Serialize + utoipa::ToSchema> {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(inline = false)]
    pub data: Option<T>,
}

impl<T: Serialize + utoipa::ToSchema> Envelope<T> {
    pub fn ok(data: T) -> Envelope<T> {
        Envelope {
            status: "success",
            data: Some(data),
        }
    }
}

/// 无 data 的成功响应：`{"status":"success"}`
#[derive(Serialize, utoipa::ToSchema)]
pub struct SuccessOnly {
    pub status: &'static str,
}

/// 无 data 的成功响应：`{"status":"success"}`
pub fn success() -> axum::Json<SuccessOnly> {
    axum::Json(SuccessOnly { status: "success" })
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({
            "status": "error",
            "error": { "code": self.code, "message": self.message }
        });
        (self.status, axum::Json(body)).into_response()
    }
}

/// JSON 请求体抽取：解析失败统一 INVALID_PARAM
pub struct JsonBody<T>(pub T);

impl<T, S> axum::extract::FromRequest<S> for JsonBody<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(
        req: axum::extract::Request,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(JsonBody(value)),
            Err(rejection) => Err(ApiError::invalid_param(rejection.to_string())),
        }
    }
}

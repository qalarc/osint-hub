//! API error type — maps to `{"error": "..."}` with proper status codes.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use nexus_core::NexusError;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("internal: {0}")]
    Internal(String),
}

pub type ApiResult<T> = Result<T, ApiError>;

impl From<NexusError> for ApiError {
    fn from(e: NexusError) -> Self {
        match e {
            NexusError::NotFound(s) => ApiError::NotFound(s),
            NexusError::Invalid(s) => ApiError::Invalid(s),
            other => ApiError::Internal(other.to_string()),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self {
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::Invalid(_) => StatusCode::BAD_REQUEST,
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({"error": self.to_string()}))).into_response()
    }
}

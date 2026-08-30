use axum::{Json, http::StatusCode, response::IntoResponse};
use serde::Serialize;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("invalid username or password")]
    InvalidCredentials,
    #[error("rate limited: {0}")]
    RateLimited(String),
    #[error("service unavailable: {0}")]
    ServiceUnavailable(String),
    #[error("I/O error")]
    Io(#[from] std::io::Error),
    #[error("configuration error: {0}")]
    Config(String),
    #[error("internal error: {0}")]
    Internal(String),
}

#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub code: i32,
    pub msg: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            code: 0,
            msg: "success".into(),
            data: Some(data),
        }
    }
}

impl ApiResponse<()> {
    pub fn ok_empty() -> Self {
        Self {
            code: 0,
            msg: "success".into(),
            data: None,
        }
    }

    pub fn error(code: i32, msg: impl Into<String>) -> Self {
        Self {
            code,
            msg: msg.into(),
            data: None,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, code, message) = match self {
            AppError::BadRequest(message) => (StatusCode::OK, -1, message),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, -401, "unauthorized".into()),
            AppError::Forbidden(message) => (StatusCode::FORBIDDEN, -403, message),
            AppError::NotFound(message) => (StatusCode::NOT_FOUND, -404, message),
            AppError::Conflict(message) => (StatusCode::OK, -409, message),
            AppError::InvalidCredentials => {
                (StatusCode::OK, -2, "invalid username or password".into())
            }
            AppError::RateLimited(message) => (StatusCode::OK, -5, message),
            AppError::ServiceUnavailable(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                -503,
                "service unavailable".into(),
            ),
            AppError::Io(_) | AppError::Config(_) | AppError::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                -500,
                "internal server error".into(),
            ),
        };
        (status, Json(ApiResponse::<()>::error(code, message))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use serde_json::Value;

    use super::*;

    #[tokio::test]
    async fn internal_errors_do_not_disclose_details() {
        let response = AppError::Internal("sensitive path".into()).into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["msg"], "internal server error");
        assert!(!String::from_utf8_lossy(&body).contains("sensitive path"));
    }

    #[tokio::test]
    async fn service_unavailable_errors_do_not_disclose_details() {
        let response =
            AppError::ServiceUnavailable("private backend detail".into()).into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["msg"], "service unavailable");
        assert!(!String::from_utf8_lossy(&body).contains("private backend detail"));
    }
}

use axum::http::StatusCode;
use serde_json::json;

use crate::{auth::AuthError, ApiError, ApiErrorKind};

pub fn refusal(status: StatusCode, code: &'static str, message: &'static str) -> ApiError {
    ApiError {
        kind: ApiErrorKind::Expected,
        status,
        detail: json!({"code": code, "message": message}),
        retry_after: None,
    }
}

pub fn database_failure(state: Option<&str>) -> ApiError {
    let (status, code, message) = match state {
        Some("55P03") => (
            StatusCode::CONFLICT,
            "MARKET_BUSY",
            "The requested rows are busy.",
        ),
        Some("57014") => (
            StatusCode::SERVICE_UNAVAILABLE,
            "TRANSACTION_TIMEOUT",
            "The transaction exceeded its deadline.",
        ),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "DATABASE_ERROR",
            "The database operation could not complete.",
        ),
    };
    let mut error = refusal(status, code, message);
    error.kind = ApiErrorKind::Database;
    error
}

pub fn retry_exhausted() -> ApiError {
    let mut error = refusal(
        StatusCode::SERVICE_UNAVAILABLE,
        "TRANSACTION_RETRY_EXHAUSTED",
        "The transaction could not acquire a consistent state.",
    );
    error.retry_after = Some(1);
    error
}

pub fn authentication(error: AuthError) -> ApiError {
    let code = match error {
        AuthError::MissingBearer | AuthError::Credentials => "INVALID_CREDENTIALS",
        AuthError::PasswordChanged => "AUTH_PASSWORD_CHANGED",
        AuthError::SessionRevoked => "AUTH_SESSION_REVOKED",
        AuthError::Pending => "ACCOUNT_PENDING",
        AuthError::Rejected => "ACCOUNT_REJECTED",
        AuthError::PasswordChangeRequired => "PASSWORD_CHANGE_REQUIRED",
    };
    refusal(
        StatusCode::from_u16(error.status()).expect("declared auth status"),
        code,
        "The current account cannot authorize this operation.",
    )
}

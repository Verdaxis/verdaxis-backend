//! Structured errors and result bounds for Phase 5 public reads.

use crate::{ApiError, ApiErrorKind};
use axum::http::StatusCode;

fn error(
    status: StatusCode,
    code: &'static str,
    message: impl Into<String>,
    kind: ApiErrorKind,
    retry_after: Option<u32>,
) -> ApiError {
    ApiError {
        status,
        detail: serde_json::json!({"code": code, "message": message.into()}),
        kind,
        retry_after,
    }
}

pub fn unavailable() -> ApiError {
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "READ_UNAVAILABLE",
        "Market data is temporarily unavailable; retry shortly.",
        ApiErrorKind::Expected,
        Some(1),
    )
}

pub fn result_limit() -> ApiError {
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "READ_RESULT_LIMIT",
        "The result exceeds the supported read limit.",
        ApiErrorKind::Expected,
        Some(1),
    )
}

/// Supply a fixed validation explanation, not a raw parser or database error.
pub fn invalid_input(message: impl Into<String>) -> ApiError {
    error(
        StatusCode::UNPROCESSABLE_ENTITY,
        "INVALID_INPUT",
        message,
        ApiErrorKind::Expected,
        None,
    )
}

/// SQL fetches one extra row so the route can refuse an incomplete result.
/// The cap is a route policy constant, not a request-supplied SQL limit.
pub fn probe_limit(cap: u64) -> i64 {
    cap.checked_add(1)
        .and_then(|limit| i64::try_from(limit).ok())
        .expect("route result cap must leave room for a PostgreSQL sentinel row")
}

pub fn within_limit(count: u64, cap: u64) -> Result<(), ApiError> {
    if count > cap {
        Err(result_limit())
    } else {
        Ok(())
    }
}

/// Keep diagnostic classification in the shared logger; never format the error.
pub fn database_failure(stage: &'static str, source: &sqlx::Error) -> ApiError {
    crate::runtime::admission::log_sql_error(stage, source);
    let retryable = match source {
        sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed => true,
        sqlx::Error::Database(database) => matches!(
            database.code().as_deref(),
            Some("55P03" | "40P01" | "40001" | "57014")
        ),
        _ => false,
    };
    if retryable {
        let mut response = unavailable();
        response.kind = ApiErrorKind::Database;
        response
    } else {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "READ_FAILED",
            "Market data could not be read.",
            ApiErrorKind::Database,
            None,
        )
    }
}

//! Ordered request preparation for Phase 2 route adapters.
//!
//! Authentication is supplied by the caller so current-account validation can
//! stay inside `run_authenticated`; this module never loads or unlocks a user.
use std::future::Future;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::Value;

use super::{AuthError, limits::SharedLimits};
use crate::{ApiError, ApiErrorKind};

pub struct RequestAdmission {
    pub client: String,
    pub path: String,
    pub now_seconds: u64,
    pub limits: SharedLimits,
}

#[derive(Debug)]
pub enum RequestError {
    Authentication(AuthError),
    Validation(ApiError),
}

impl RequestError {
    pub fn status(&self) -> u16 {
        match self {
            Self::Authentication(error) => error.status(),
            Self::Validation(error) => error.status.as_u16(),
        }
    }
}

impl IntoResponse for RequestError {
    fn into_response(self) -> Response {
        match self {
            Self::Authentication(error) => {
                let mut response = (
                    StatusCode::from_u16(error.status()).expect("auth status"),
                    axum::Json(error.body()),
                )
                    .into_response();
                if let Some(value) = error.www_authenticate() {
                    response.headers_mut().insert(
                        "www-authenticate",
                        value.parse().expect("static auth header"),
                    );
                }
                response
            }
            Self::Validation(error) => error.into_response(),
        }
    }
}

/// Parse JSON syntax before authentication, then validate typed route input and
/// consume endpoint quota. The authentication future must retain its transaction
/// owner if the caller's validator or handler requires database access.
pub async fn admit_request<A, T>(
    raw: &[u8],
    request: &RequestAdmission,
    authenticate: impl FnOnce() -> A,
    validate: impl FnOnce(Value) -> Result<T, ApiError>,
) -> Result<T, RequestError>
where
    A: Future<Output = Result<(), AuthError>>,
{
    let body = parse_json_body(raw).map_err(RequestError::Validation)?;
    authenticate().await.map_err(RequestError::Authentication)?;
    validate_and_admit(body, request, validate).map_err(RequestError::Validation)
}

/// For database-backed route adapters: parse before `run_authenticated`, then
/// call this function only inside its admitted work closure, after user state
/// has been checked. This preserves one connection and one user-row lock.
pub fn validate_and_admit<T>(
    body: Value,
    request: &RequestAdmission,
    validate: impl FnOnce(Value) -> Result<T, ApiError>,
) -> Result<T, ApiError> {
    let value = validate(body)?;
    request
        .limits
        .check_endpoint(&request.client, &request.path, request.now_seconds)?;
    Ok(value)
}

fn json_error_position(raw: &[u8], error: &serde_json::Error) -> usize {
    // serde reports one-based byte columns; FastAPI reports an absolute
    // Unicode code-point offset. EOF points after the last input character.
    if error.is_eof() {
        return String::from_utf8_lossy(raw).chars().count();
    }
    let line_start = raw
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'\n')
        .take(error.line().saturating_sub(1))
        .map(|(index, _)| index + 1)
        .last()
        .unwrap_or(0);
    let byte_offset = (line_start + error.column().saturating_sub(1)).min(raw.len());
    String::from_utf8_lossy(&raw[..byte_offset]).chars().count()
}

pub fn parse_json_body(raw: &[u8]) -> Result<Value, ApiError> {
    serde_json::from_slice(raw).map_err(|error| ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        // Route oracle vectors own exact Python parser diagnostics. Keep this
        // syntax refusal separate from field-validation and authentication.
        detail: serde_json::json!([{
            "type":"json_invalid", "loc":["body",json_error_position(raw,&error)],
            "msg":"JSON decode error", "input":{}, "ctx":{"error":error.to_string()}
        }]),
        retry_after: None,
        kind: ApiErrorKind::Expected,
    })
}

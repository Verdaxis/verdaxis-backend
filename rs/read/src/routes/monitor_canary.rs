//! `POST /api/monitor/signup-canary-cleanup`: the external monitor removes the
//! synthetic identity its signup canary created (`app/routers/monitor.py`).
//!
//! FastAPI's order is kept: malformed JSON is refused first (the body is
//! parsed before any dependency runs), then the monitor token (404 when no
//! token is configured, 401 when the header is absent or wrong), then the
//! body model and the canary predicate (422), then the cleanup itself
//! (`cli::canary::cleanup`, 409 when retained KYC documents block it). The
//! token is compared in constant time and never logged or echoed.
use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde_json::{Value, json};
use sqlx::PgPool;

use crate::ApiError;
use crate::cli::canary::{CleanupError, RETAINED_CODE, RETAINED_MESSAGE, canary_domain, cleanup};

pub const PATH: &str = "/api/monitor/signup-canary-cleanup";
/// The body is one email address; anything near this size is not a canary.
const BODY_LIMIT_BYTES: usize = 16 * 1024;

struct CanaryRoute {
    token: Option<Arc<str>>,
    pool: PgPool,
}

/// The route with its token (`MONITOR_TOKEN`; `None` or empty disables it with
/// 404) and the application pool that deletes.
pub fn routes(token: Option<String>, pool: PgPool) -> Router {
    let token = token.filter(|token| !token.is_empty()).map(Arc::from);
    Router::new()
        .route(PATH, post(cleanup_route))
        .with_state(Arc::new(CanaryRoute { token, pool }))
}

/// A FastAPI `HTTPException` body. Python's monitor refusals carry no
/// `WWW-Authenticate` header, so the shared 401 helper is not used here.
fn refusal(status: StatusCode, detail: Value) -> Response {
    (status, axum::Json(json!({ "detail": detail }))).into_response()
}

async fn cleanup_route(State(route): State<Arc<CanaryRoute>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let Ok(raw) = axum::body::to_bytes(body, BODY_LIMIT_BYTES).await else {
        return refusal(
            StatusCode::PAYLOAD_TOO_LARGE,
            json!({"code": "BODY_TOO_LARGE", "message": "The request body is too large."}),
        );
    };
    let payload = if raw.is_empty() {
        Value::Null
    } else {
        match crate::auth::request::parse_json_body(&raw) {
            Ok(payload) => payload,
            Err(error) => return error.into_response(),
        }
    };
    let Some(expected) = route.token.as_deref() else {
        return refusal(StatusCode::NOT_FOUND, json!("Not found"));
    };
    if !crate::auth::email::monitor_admitted(Some(expected), &parts.headers) {
        return refusal(StatusCode::UNAUTHORIZED, json!("Unauthorized"));
    }
    if !payload.is_object() {
        return refusal(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!([crate::crud::watchlists::body_shape_error(&payload)]),
        );
    }
    let email = match crate::crud::referrals::invite::normalize_email(&payload) {
        Ok(email) => email,
        Err(error) => return error.into_response(),
    };
    let Some(domain) = canary_domain(&email) else {
        return refusal(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!("Only monitor canary emails can be cleaned up"),
        );
    };
    match cleanup(&route.pool, &email, domain).await {
        Ok(counts) => axum::Json(counts).into_response(),
        Err(CleanupError::Retained) => refusal(
            StatusCode::CONFLICT,
            json!({"code": RETAINED_CODE, "message": RETAINED_MESSAGE}),
        ),
        Err(CleanupError::Database) => ApiError::database().into_response(),
    }
}

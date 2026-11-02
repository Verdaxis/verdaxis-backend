//! Administrator obligation list and responsible-person reassignment component.
//!
//! The serving composition does not mount this component: it stays unrouted
//! until the offboarding release switch enables it (see
//! `runtime::market_write::reassignment_routes`). It lives outside
//! `routes/market_write/`, whose files are exactly the market-write switch
//! routes.

use crate::auth::{VerificationSettings, verify_authorization};
use crate::market_reassignment::{
    AuditContext, MarketErrorCode, MarketWriteError, ReassignmentCommand, ReassignmentInput,
    execute_reassignment,
};
use axum::{
    Router,
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::Utc;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Concurrent administrator reassignment commands, the same bound as freeze.
const ADMIN_COMMAND_PERMITS: usize = 4;

/// The request header that carries an assisted market-support context.
const SUPPORT_CONTEXT_HEADER: &str = "x-verdaxis-market-support-context";

pub const OBLIGATIONS_PATH: &str = "/api/auth/admin/users/{user_id}/offboarding-trades";
pub const REASSIGN_PATH: &str = "/api/auth/admin/trades/{trade_id}/reassign-responsible";

#[derive(Clone)]
pub struct ReassignmentState {
    pool: sqlx::PgPool,
    verification: Arc<VerificationSettings>,
    permits: Arc<Semaphore>,
}

impl ReassignmentState {
    pub fn new(pool: sqlx::PgPool, verification: VerificationSettings) -> Self {
        Self {
            pool,
            verification: Arc::new(verification),
            permits: Arc::new(Semaphore::new(ADMIN_COMMAND_PERMITS)),
        }
    }
}

/// Return a bare component; the serving owner supplies its shared ingress layers.
pub fn router(state: ReassignmentState) -> Router {
    Router::new()
        .route(OBLIGATIONS_PATH, get(list))
        .route(REASSIGN_PATH, post(reassign))
        .with_state(state)
}

/// An assisted support context never authorizes an administrator action on
/// a person's obligations.
fn carries_support_context(parts: &Parts) -> bool {
    parts
        .headers
        .get(SUPPORT_CONTEXT_HEADER)
        .is_some_and(|value| {
            !value.as_bytes().is_empty() && !value.as_bytes().iter().all(u8::is_ascii_whitespace)
        })
}

async fn dispatch(state: ReassignmentState, parts: Parts, input: ReassignmentInput) -> Response {
    if carries_support_context(&parts) {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({"detail":{"code":"MARKET_SUPPORT_MUTATION_NOT_ALLOWED","message":"This operation is not available in Market Support mode"}})),
        )
            .into_response();
    }
    let header = parts
        .headers
        .get("authorization")
        .and_then(|value| value.to_str().ok());
    let access = match verify_authorization(header, &state.verification, Utc::now()) {
        Ok(access) => access,
        Err(_) => {
            return MarketWriteError::new(MarketErrorCode::AuthCredentialsInvalid).into_response();
        }
    };
    let (client, correlation) = crate::runtime::http::audit_identity(&parts);
    let audit = match AuditContext::new(Some(client), Some(correlation)) {
        Ok(audit) => audit,
        Err(error) => return error.into_response(),
    };
    match execute_reassignment(
        state.pool,
        state.permits,
        access,
        ReassignmentCommand { input, audit },
    )
    .await
    {
        Ok(result) => (
            StatusCode::from_u16(result.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            axum::Json(result.body),
        )
            .into_response(),
        Err(error) => error.into_response(),
    }
}

async fn list(
    State(state): State<ReassignmentState>,
    Path(subject): Path<String>,
    RawQuery(query): RawQuery,
    parts: Parts,
) -> Response {
    dispatch(state, parts, ReassignmentInput::List { subject, query }).await
}

async fn reassign(
    State(state): State<ReassignmentState>,
    Path(trade): Path<String>,
    parts: Parts,
    body: Bytes,
) -> Response {
    dispatch(state, parts, ReassignmentInput::Reassign { trade, body }).await
}

#[cfg(feature = "contract-tests")]
pub fn contract_router(pool: sqlx::PgPool, key: &str) -> Router {
    let verification = VerificationSettings::new(
        key.to_owned(),
        None,
        "reassignment-tests".into(),
        "reassignment-tests".into(),
    );
    crate::runtime::http::middleware(router(ReassignmentState::new(pool, verification)), "test")
}

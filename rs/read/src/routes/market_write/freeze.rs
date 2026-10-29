//! Freeze administration component. Serving activation belongs to composition.

use crate::auth::{VerificationSettings, verify_authorization};
use crate::market_freeze::{
    AuditContext, FreezeCommand, FreezeInput, MarketErrorCode, MarketWriteError, execute_freeze,
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
use std::sync::Arc;
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct FreezeState {
    pool: sqlx::PgPool,
    verification: Arc<VerificationSettings>,
    permits: Arc<Semaphore>,
}
impl FreezeState {
    pub fn new(pool: sqlx::PgPool, verification: VerificationSettings) -> Self {
        Self {
            pool,
            verification: Arc::new(verification),
            permits: Arc::new(Semaphore::new(4)),
        }
    }
}
/// Return a bare component; the serving owner supplies its shared ingress layers.
pub fn router(state: FreezeState) -> Router {
    Router::new()
        .route("/api/admin/trades/frozen", get(list))
        .route("/api/admin/trades/{trade}/freeze", post(freeze))
        .route("/api/admin/trades/{trade}/unfreeze", post(unfreeze))
        .with_state(state)
}
async fn dispatch(state: FreezeState, parts: Parts, input: FreezeInput) -> Response {
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
    match execute_freeze(
        state.pool,
        state.permits,
        access,
        FreezeCommand { input, audit },
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
async fn freeze(
    State(state): State<FreezeState>,
    Path(trade): Path<String>,
    parts: Parts,
    body: Bytes,
) -> Response {
    dispatch(state, parts, FreezeInput::Freeze { trade, body }).await
}
async fn unfreeze(
    State(state): State<FreezeState>,
    Path(trade): Path<String>,
    parts: Parts,
    body: Bytes,
) -> Response {
    dispatch(state, parts, FreezeInput::Unfreeze { trade, body }).await
}
async fn list(
    State(state): State<FreezeState>,
    RawQuery(query): RawQuery,
    parts: Parts,
) -> Response {
    dispatch(state, parts, FreezeInput::List { query }).await
}

#[cfg(feature = "contract-tests")]
pub fn contract_router(pool: sqlx::PgPool, key: &str) -> Router {
    let verification = VerificationSettings::new(
        key.to_owned(),
        None,
        "freeze-tests".into(),
        "freeze-tests".into(),
    );
    crate::runtime::http::middleware(router(FreezeState::new(pool, verification)))
}

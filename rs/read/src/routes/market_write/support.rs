//! Support component routes. Serving composition is a separate owner.
//!
//! Handlers pass the raw body, path and query to the command, which decodes
//! them only after the caller is an approved administrator.
use crate::auth::{VerificationSettings, verify_authorization};
use crate::market_support::{
    AuditContext, SupportCommand, SupportRequest, SupportSettings, missing_credentials, run_support,
};
use crate::runtime::http::audit_identity;
use crate::{Config, MarketPolicy};
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
pub struct SupportState {
    pool: sqlx::PgPool,
    verification: Arc<VerificationSettings>,
    settings: SupportSettings,
    policy: Arc<MarketPolicy>,
    config: Arc<Config>,
    permits: Arc<Semaphore>,
}
impl SupportState {
    pub fn new(
        pool: sqlx::PgPool,
        verification: Arc<VerificationSettings>,
        settings: SupportSettings,
        policy: MarketPolicy,
        config: Config,
    ) -> Self {
        Self {
            pool,
            verification,
            settings,
            policy: Arc::new(policy),
            config: Arc::new(config),
            permits: Arc::new(Semaphore::new(4)),
        }
    }
}
pub fn router(state: SupportState) -> Router {
    Router::new()
        .route("/api/admin/market-support/capabilities", get(capabilities))
        .route(
            "/api/admin/market-support/capability-assignments",
            post(grant),
        )
        .route(
            "/api/admin/market-support/capability-assignments/{id}/revoke",
            post(revoke),
        )
        .route(
            "/api/admin/market-support/organizations",
            get(organizations),
        )
        .route(
            "/api/admin/market-support/organizations/{id}/authorizations",
            get(authorizations).post(retired),
        )
        .route(
            "/api/admin/market-support/organizations/{id}/listings",
            get(listings).post(retired),
        )
        .route(
            "/api/admin/market-support/organizations/{id}/listings/{order}/cancel",
            post(retired),
        )
        .route(
            "/api/admin/market-support/organizations/{id}/authorizations/{authorization}/revoke",
            post(retired),
        )
        .with_state(state)
}

fn header<'a>(parts: &'a Parts, name: &str) -> Option<&'a str> {
    parts
        .headers
        .get(name)
        .and_then(|value| value.to_str().ok())
}


async fn dispatch(state: SupportState, parts: Parts, request: SupportRequest) -> Response {
    let authorization = header(&parts, "authorization");
    let Ok(access) = verify_authorization(authorization, &state.verification, Utc::now()) else {
        return missing_credentials().into_response();
    };
    let (client, correlation) = audit_identity(&parts);
    let audit = match AuditContext::new(Some(client), Some(correlation)) {
        Ok(audit) => audit,
        Err(error) => return error.into_response(),
    };
    let command = SupportCommand {
        request,
        settings: state.settings,
        access: Arc::new(access),
        policy: state.policy,
        config: state.config,
        audit,
    };
    match run_support(state.pool, state.permits, command).await {
        Ok(value) => axum::Json(value).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn capabilities(State(state): State<SupportState>, parts: Parts) -> Response {
    dispatch(state, parts, SupportRequest::Capabilities).await
}
async fn grant(State(state): State<SupportState>, parts: Parts, body: Bytes) -> Response {
    let mut response = dispatch(state, parts, SupportRequest::Grant { body }).await;
    if response.status() == StatusCode::OK {
        *response.status_mut() = StatusCode::CREATED;
    }
    response
}
async fn revoke(
    State(state): State<SupportState>,
    Path(assignment): Path<String>,
    parts: Parts,
    body: Bytes,
) -> Response {
    dispatch(state, parts, SupportRequest::Revoke { assignment, body }).await
}
async fn organizations(
    State(state): State<SupportState>,
    RawQuery(query): RawQuery,
    parts: Parts,
) -> Response {
    dispatch(state, parts, SupportRequest::Organizations { query }).await
}
async fn authorizations(
    State(state): State<SupportState>,
    Path(organization): Path<String>,
    RawQuery(query): RawQuery,
    parts: Parts,
) -> Response {
    let request = SupportRequest::Authorizations {
        organization,
        query,
    };
    dispatch(state, parts, request).await
}
async fn listings(
    State(state): State<SupportState>,
    Path(organization): Path<String>,
    RawQuery(query): RawQuery,
    parts: Parts,
) -> Response {
    let request = SupportRequest::Listings {
        organization,
        query,
    };
    dispatch(state, parts, request).await
}
async fn retired(State(state): State<SupportState>, parts: Parts) -> Response {
    dispatch(state, parts, SupportRequest::Retired).await
}

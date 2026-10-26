//! Commission administrator routes, composed without activating serving routes.

use axum::{
    Router,
    body::to_bytes,
    extract::{ConnectInfo, Request, State},
    http::{StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response},
    routing::{get, put},
};
use chrono::Utc;
use serde_json::{Value, json};
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::auth::{VerificationSettings, limits::SharedLimits, verify_authorization};
use crate::commission::{
    AuditContext, CommissionCommand, CommissionOperation, run_commission_command,
};
use crate::{Config, MarketPolicy};

/// A patch carries a status, an invoice number, two dates and free-text notes.
/// One mebibyte leaves ample room for notes and bounds what an authenticated
/// caller can make the server buffer before the administrator check runs.
const PATCH_BODY_LIMIT_BYTES: usize = 1024 * 1024;

pub struct RouteContext {
    pub pool: sqlx::PgPool,
    pub permits: Arc<Semaphore>,
    pub settings: Arc<VerificationSettings>,
    pub limits: SharedLimits,
    pub policy: Arc<MarketPolicy>,
    pub config: Arc<Config>,
}
pub fn routes(context: Arc<RouteContext>) -> Router {
    Router::new()
        .route("/api/orders/admin/commissions", get(handle))
        .route("/api/orders/admin/commissions/summary", get(handle))
        .route("/api/orders/admin/commissions/{commission_id}", put(handle))
        .with_state(context)
}
async fn handle(State(context): State<Arc<RouteContext>>, request: Request) -> Response {
    let now = Utc::now();
    let authorization = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok());
    let access = match verify_authorization(authorization, &context.settings, now) {
        Ok(a) => a,
        Err(error) => {
            return (
                StatusCode::from_u16(error.status()).unwrap_or(StatusCode::UNAUTHORIZED),
                axum::Json(error.body()),
            )
                .into_response();
        }
    };
    if crate::auth::scope::support_present(request.headers()) {
        return refusal(
            403,
            "SUPPORT_CONTEXT_NOT_ALLOWED",
            "A support context is not accepted here",
        );
    }
    let client = crate::runtime::http::client_ip(
        request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|p| p.0.ip()),
        request
            .headers()
            .get("x-forwarded-for")
            .and_then(|h| h.to_str().ok()),
    );
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|h| h.to_str().ok())
        .map(str::to_owned);
    let audit = match AuditContext::new(Some(client.to_string()), request_id) {
        Ok(a) => a,
        Err(e) => return e.into_response(),
    };
    let path = request.uri().path();
    let operation = if request.method() == axum::http::Method::GET {
        if path.ends_with("/summary") {
            CommissionOperation::Summary
        } else {
            CommissionOperation::List
        }
    } else {
        let id = match path
            .rsplit('/')
            .next()
            .and_then(|s| Uuid::parse_str(s).ok())
        {
            Some(id) => id,
            None => return refusal(422, "VALIDATION_FAILED", "A commission UUID is required"),
        };
        let bytes = match to_bytes(request.into_body(), PATCH_BODY_LIMIT_BYTES).await {
            Ok(bytes) => bytes,
            Err(_) => return refusal(413, "BODY_TOO_LARGE", "The body is too large"),
        };
        let body: Value = match serde_json::from_slice(&bytes) {
            Ok(body) => body,
            Err(_) => return refusal(422, "VALIDATION_FAILED", "A JSON object is required"),
        };
        CommissionOperation::Patch { id, body }
    };
    let command = CommissionCommand {
        operation,
        policy: context.policy.clone(),
        config: context.config.clone(),
        limits: context.limits.clone(),
        client: client.to_string(),
        at: now,
        audit_context: audit,
    };
    match run_commission_command(
        context.pool.clone(),
        context.permits.clone(),
        access,
        command,
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
fn refusal(status: u16, code: &str, message: &str) -> Response {
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        axum::Json(json!({"detail":{"code":code,"message":message}})),
    )
        .into_response()
}

//! Commission administrator routes, composed without activating serving routes.

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{Method, StatusCode, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, put},
};
use chrono::Utc;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::auth::{VerificationSettings, limits::SharedLimits, verify_authorization};
use crate::commission::{
    AuditContext, CommissionCommand, CommissionOperation, CommissionPatch, run_commission_command,
};
use crate::runtime::http::audit_identity;
use crate::{Config, MarketPolicy};

/// Match the production edge's request-body ceiling, including ignored JSON keys.
const PATCH_BODY_LIMIT_BYTES: usize = 32_000_000;

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

fn header<'a>(parts: &'a Parts, name: &str) -> Option<&'a str> {
    parts
        .headers
        .get(name)
        .and_then(|value| value.to_str().ok())
}


async fn handle(State(context): State<Arc<RouteContext>>, request: Request) -> Response {
    let now = Utc::now();
    let (parts, body) = request.into_parts();
    let access = match verify_authorization(header(&parts, "authorization"), &context.settings, now)
    {
        Ok(access) => access,
        Err(error) => {
            return (
                StatusCode::from_u16(error.status()).unwrap_or(StatusCode::UNAUTHORIZED),
                axum::Json(error.body()),
            )
                .into_response();
        }
    };
    let (client, correlation) = audit_identity(&parts);
    let mut audit = AuditContext::default();
    let path = parts.uri.path();
    let operation = if parts.method == Method::GET {
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
        let bytes = match read_patch_body(body).await {
            Ok(bytes) => bytes,
            Err(response) => return response,
        };
        let body: Value = match serde_json::from_slice(&bytes) {
            Ok(body) => body,
            Err(_) => return refusal(422, "VALIDATION_FAILED", "A JSON object is required"),
        };
        if CommissionPatch::parse(body.clone()).is_err() {
            return refusal(
                422,
                "VALIDATION_FAILED",
                "The commission patch is not valid",
            );
        }
        audit = match AuditContext::new(Some(client.clone()), Some(correlation)) {
            Ok(audit) => audit,
            Err(error) => return error.into_response(),
        };
        CommissionOperation::Patch { id, body }
    };
    let command = CommissionCommand {
        operation,
        policy: context.policy.clone(),
        config: context.config.clone(),
        limits: context.limits.clone(),
        client,
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
async fn read_patch_body(body: Body) -> Result<Vec<u8>, Response> {
    let mut stream = body.into_data_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|_| refusal(400, "BODY_READ_FAILED", "The body could not be read"))?;
        if chunk.len() > PATCH_BODY_LIMIT_BYTES - bytes.len() {
            return Err(refusal(413, "BODY_TOO_LARGE", "The body is too large"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn refusal(status: u16, code: &str, message: &str) -> Response {
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        axum::Json(json!({"detail":{"code":code,"message":message}})),
    )
        .into_response()
}

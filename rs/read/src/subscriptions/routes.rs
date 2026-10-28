use std::sync::Arc;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use super::{
    fees::FeeConfig,
    store::{self, FeeUpdate, Page, SubscriptionUpdate},
};
use crate::{
    ApiError, AppState,
    auth::{
        AuthenticatedError, AuthenticatedRequest, CurrentActor, VerificationSettings,
        run_authenticated, verify_authorization,
    },
    business::{
        audit::{AuditFact, append_audit},
        error::{authentication, database_failure, refusal},
        transaction::{
            AttemptError, BusinessCommand, CommandFuture, OrganizationWritePlan,
            run_organization_write,
        },
    },
};

#[derive(Debug, Clone)]
pub enum Operation {
    Mine,
    List(Page),
    Get(Uuid),
    Update(Uuid, SubscriptionUpdate),
}

pub struct SubscriptionCommand {
    actor: Uuid,
    operation: Operation,
    now: DateTime<Utc>,
}

#[derive(Debug)]
pub struct CommandOutput {
    pub body: Value,
    pub truncated: Option<bool>,
}

impl SubscriptionCommand {
    pub fn new(actor: Uuid, operation: Operation, now: DateTime<Utc>) -> Self {
        Self {
            actor,
            operation,
            now,
        }
    }
}

fn admin_required() -> ApiError {
    refusal(
        StatusCode::FORBIDDEN,
        "ADMIN_REQUIRED",
        "Administrator access required",
    )
}

fn encode(value: impl serde::Serialize) -> Result<Value, AttemptError> {
    serde_json::to_value(value).map_err(|_| ApiError::internal().into())
}

impl BusinessCommand for SubscriptionCommand {
    type Output = CommandOutput;

    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
    ) -> CommandFuture<'a, OrganizationWritePlan, AttemptError> {
        Box::pin(async move {
            let org: Option<Uuid> = match &self.operation {
                Operation::Mine => {
                    sqlx::query_scalar("SELECT organization_id FROM users WHERE id=$1")
                        .bind(self.actor)
                        .fetch_optional(&mut *conn)
                        .await?
                        .flatten()
                }
                Operation::Get(org) | Operation::Update(org, _) => Some(*org),
                Operation::List(_) => None,
            };
            Ok(OrganizationWritePlan::new(
                org,
                [self.actor],
                [],
                json!({"organization":org}),
            ))
        })
    }

    fn execute<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: &'a CurrentActor,
    ) -> CommandFuture<'a, CommandOutput, AttemptError> {
        Box::pin(async move {
            if !matches!(self.operation, Operation::Mine) && actor.role.as_deref() != Some("ADMIN")
            {
                return Err(admin_required().into());
            }
            let mut truncated = None;
            let body = match &self.operation {
                Operation::Mine => {
                    let org = actor.organization_id.ok_or_else(|| {
                        refusal(
                            StatusCode::FORBIDDEN,
                            "ORGANIZATION_REQUIRED",
                            "Organization required",
                        )
                    })?;
                    encode(store::bootstrap(conn, org).await?)?
                }
                Operation::List(page) => {
                    let (rows, more) = store::list(conn, *page).await?;
                    truncated = Some(more);
                    encode(rows)?
                }
                Operation::Get(org) => {
                    store::require_organization(conn, *org).await?;
                    encode(store::find(conn, *org).await?.ok_or_else(|| {
                        refusal(
                            StatusCode::NOT_FOUND,
                            "SUBSCRIPTION_NOT_FOUND",
                            "Subscription not found",
                        )
                    })?)?
                }
                Operation::Update(org, patch) => {
                    store::require_organization(conn, *org).await?;
                    // Same graph as Python: organization/user locks are already
                    // held; acquire an existing subscription row before its audit.
                    sqlx::query("SELECT id FROM subscriptions WHERE org_id=$1 FOR UPDATE")
                        .bind(org)
                        .fetch_optional(&mut *conn)
                        .await?;
                    let previous = store::find(conn, *org).await?;
                    let updated = store::update(conn, *org, patch).await?;
                    let mut changes = json!({"org_id":org,"tier":{"from":previous.as_ref().map(|row|row.tier),"to":updated.tier}});
                    if !matches!(patch.fee, FeeUpdate::Omitted) {
                        changes["seller_fee_per_mt_usd"] = json!({"from":previous.as_ref().and_then(|row|row.seller_fee_per_mt_usd).map(|fee|fee.to_string()),"to":updated.seller_fee_per_mt_usd.map(|fee|fee.to_string())});
                    }
                    append_audit(
                        conn,
                        &AuditFact {
                            id: Uuid::new_v4(),
                            actor_id: Some(actor.id),
                            action: "subscription.updated".into(),
                            resource_type: "subscription".into(),
                            resource_id: Some(org.to_string()),
                            changes: Some(changes),
                            ip_address: None,
                            request_id: None,
                            at: self.now,
                        },
                    )
                    .await?;
                    encode(updated)?
                }
            };
            Ok(CommandOutput { body, truncated })
        })
    }
}

#[derive(Clone)]
struct RouteState {
    app: AppState,
    verification: Arc<VerificationSettings>,
    fees: FeeConfig,
}

/// Component router only. Serving activation belongs to the integration owner.
pub fn router(app: AppState, verification: Arc<VerificationSettings>, fees: FeeConfig) -> Router {
    Router::new()
        .route("/api/subscriptions/fees", get(schedule))
        .route("/api/subscriptions/me", get(mine))
        .route("/api/admin/subscriptions", get(list))
        .route("/api/admin/subscriptions/{org_id}", get(read).put(write))
        .with_state(RouteState {
            app,
            verification,
            fees,
        })
}

async fn schedule(State(state): State<RouteState>) -> Json<super::fees::FeeSchedule> {
    Json(state.fees.schedule())
}

async fn dispatch(
    state: RouteState,
    headers: HeaderMap,
    operation: Operation,
) -> Result<Response, ApiError> {
    let now = Utc::now();
    let access = verify_authorization(
        headers
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        &state.verification,
        now,
    )
    .map_err(authentication)?;
    let command = SubscriptionCommand::new(access.subject, operation, now);
    let result = run_organization_write(&state.app, access, command).await?;
    let mut response = Json(result.body).into_response();
    if let Some(truncated) = result.truncated {
        response.headers_mut().insert(
            "x-result-truncated",
            if truncated { "true" } else { "false" }
                .parse()
                .expect("literal header"),
        );
    }
    Ok(response)
}

/// Refuses a caller who is not an administrator before any input is parsed:
/// authentication (401), then role (403), then input validation (422), so an
/// unauthorized caller learns nothing about its input.
async fn require_administrator(
    state: &RouteState,
    headers: &HeaderMap,
    route_template: &str,
) -> Result<(), ApiError> {
    let access = verify_authorization(
        headers
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        &state.verification,
        Utc::now(),
    )
    .map_err(authentication)?;
    run_authenticated(
        &state.app,
        access,
        AuthenticatedRequest::new("GET", route_template, false),
        |tx| {
            Box::pin(async move {
                if tx.actor().role.as_deref() == Some("ADMIN") {
                    Ok(())
                } else {
                    Err(admin_required())
                }
            })
        },
    )
    .await
    .map_err(|error| match error {
        AuthenticatedError::Authentication(error) => authentication(error),
        AuthenticatedError::Database(error) => error,
        AuthenticatedError::AmbiguousCommit => database_failure(None),
    })
}

async fn mine(State(state): State<RouteState>, headers: HeaderMap) -> Result<Response, ApiError> {
    dispatch(state, headers, Operation::Mine).await
}
#[derive(Deserialize)]
struct PageQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}
async fn list(
    State(state): State<RouteState>,
    headers: HeaderMap,
    query: Result<Query<PageQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, ApiError> {
    require_administrator(&state, &headers, "/api/admin/subscriptions").await?;
    let Query(query) = query.map_err(|_| {
        refusal(
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_FAILED",
            "Invalid page",
        )
    })?;
    dispatch(
        state,
        headers,
        Operation::List(Page::new(
            query.limit.unwrap_or(100),
            query.offset.unwrap_or(0),
        )?),
    )
    .await
}
fn organization(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|_| {
        refusal(
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_FAILED",
            "Invalid organization identifier",
        )
    })
}
async fn read(
    State(state): State<RouteState>,
    headers: HeaderMap,
    Path(org): Path<String>,
) -> Result<Response, ApiError> {
    require_administrator(&state, &headers, "/api/admin/subscriptions/{org_id}").await?;
    dispatch(state, headers, Operation::Get(organization(&org)?)).await
}
async fn write(
    State(state): State<RouteState>,
    headers: HeaderMap,
    Path(org): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    require_administrator(&state, &headers, "/api/admin/subscriptions/{org_id}").await?;
    let value = serde_json::from_slice(&body).map_err(|_| {
        refusal(
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_FAILED",
            "Invalid JSON body",
        )
    })?;
    dispatch(
        state,
        headers,
        Operation::Update(organization(&org)?, SubscriptionUpdate::parse(value)?),
    )
    .await
}

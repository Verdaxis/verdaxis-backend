//! Signed-in buyer conversations and administrator evidence/control boundaries.
use std::{future::Future, pin::Pin, sync::Arc};

use axum::{
    Router,
    body::to_bytes,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

use super::{
    audit::{self, AuditActor, AuditEvent, AuditKind},
    tools::ToolContext,
    turn::{self, AdmissionLimits, ModelClient, TurnDeps, TurnError, TurnLimits, UntrustedBlock},
};
use crate::{
    ApiError, ApiErrorKind, AppState,
    auth::{
        AuthenticatedError, AuthenticatedRequest, AuthenticatedTransaction, CurrentActor,
        request::RequestError, run_authenticated, verify_authorization,
    },
    business::membership::require_admin,
};

const CONVERSATIONS: &str = "/api/agent/conversations";
const CONVERSATION: &str = "/api/agent/conversations/{conversation_id}";
const MESSAGES: &str = "/api/agent/conversations/{conversation_id}/messages";
const AUDIT: &str = "/api/auth/admin/agent/audit";
const VERIFY: &str = "/api/auth/admin/agent/audit/verify";
const CONTROLS: &str = "/api/auth/admin/agent/controls";
// JSON escaping can use six bytes per character across all allowed blocks.
const BODY_BYTES: usize = 400 * 1024;

#[derive(Clone)]
pub struct RouteState {
    pub app: AppState,
    pub model_client: Arc<dyn ModelClient>,
}

pub fn user_routes(state: RouteState) -> Router {
    Router::new()
        .route(CONVERSATIONS, post(create_conversation))
        .route(CONVERSATION, get(history))
        .route(MESSAGES, post(send_message))
        .with_state(state)
}

/// The runtime applies its shared administrator quota to this subrouter.
pub fn admin_routes(state: RouteState) -> Router {
    Router::new()
        .route(AUDIT, get(list_audit))
        .route(VERIFY, get(verify_audit))
        .route(CONTROLS, get(list_controls).put(set_control))
        .with_state(state)
}

fn refusal(status: StatusCode, code: &str, message: &str) -> ApiError {
    ApiError {
        kind: ApiErrorKind::Expected,
        status,
        detail: json!({"code":code,"message":message}),
        retry_after: None,
    }
}
fn invalid() -> ApiError {
    refusal(
        StatusCode::UNPROCESSABLE_ENTITY,
        "INVALID_AGENT_REQUEST",
        "The assistant request is invalid.",
    )
}
fn missing() -> ApiError {
    refusal(
        StatusCode::NOT_FOUND,
        "CONVERSATION_NOT_FOUND",
        "The conversation was not found.",
    )
}
fn turn_error(error: TurnError) -> ApiError {
    let (status, code) = match &error {
        TurnError::Busy => (StatusCode::SERVICE_UNAVAILABLE, "AGENT_BUSY"),
        TurnError::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "AGENT_RATE_LIMITED"),
        TurnError::DailyBudgetExceeded => {
            (StatusCode::TOO_MANY_REQUESTS, "AGENT_DAILY_BUDGET_EXCEEDED")
        }
        TurnError::Paused(_) => (StatusCode::LOCKED, "AGENT_PAUSED"),
        TurnError::ContextUnavailable => return missing(),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "AGENT_UNAVAILABLE"),
    };
    let mut result = refusal(status, code, &error.to_string());
    if let TurnError::Paused(reason) = &error {
        result.detail["reason"] = json!(reason);
    }
    if error == TurnError::Busy {
        result.retry_after = Some(5);
    }
    result
}
fn api_response(error: ApiError) -> Response {
    if error.status == StatusCode::LOCKED {
        return (
            error.status,
            axum::Json(
                json!({"detail":"The assistant is paused.","reason":error.detail["reason"]}),
            ),
        )
            .into_response();
    }
    if error.detail["code"] == "AGENT_DAILY_BUDGET_EXCEEDED" {
        return (
            error.status,
            axum::Json(json!({"detail":"Daily assistant limit reached."})),
        )
            .into_response();
    }
    error.into_response()
}
fn auth_failure(error: AuthenticatedError) -> Response {
    match error {
        AuthenticatedError::Authentication(error) => {
            RequestError::Authentication(error).into_response()
        }
        AuthenticatedError::Database(error) => api_response(error),
        AuthenticatedError::AmbiguousCommit => refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            "COMMIT_OUTCOME_UNKNOWN",
            "The transaction outcome is unknown.",
        )
        .into_response(),
    }
}

async fn authenticated<T: Send + 'static>(
    state: &RouteState,
    headers: &HeaderMap,
    method: &str,
    path: &str,
    admin: bool,
    work: impl for<'a> FnOnce(
        &'a mut AuthenticatedTransaction<'_>,
    ) -> Pin<Box<dyn Future<Output = Result<T, ApiError>> + Send + 'a>>
    + Send
    + 'static,
) -> Result<T, Response> {
    let Some(settings) = state.app.phase2.as_ref() else {
        return Err(refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            "AGENT_UNAVAILABLE",
            "The assistant is unavailable.",
        )
        .into_response());
    };
    let access = verify_authorization(
        headers
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        &settings.verification,
        Utc::now(),
    )
    .map_err(|error| RequestError::Authentication(error).into_response())?;
    let enabled = state.app.config.phase2.agent.enabled;
    run_authenticated(
        &state.app,
        access,
        AuthenticatedRequest::new(method, path, false),
        move |tx| {
            Box::pin(async move {
                if admin {
                    require_admin(tx.actor())?;
                } else {
                    if tx.actor().role.as_deref() != Some("BUYER") {
                        return Err(refusal(
                            StatusCode::FORBIDDEN,
                            "BUYER_REQUIRED",
                            "Buyer access is required.",
                        ));
                    }
                    if !enabled {
                        return Err(refusal(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "AGENT_DISABLED",
                            "The assistant is disabled.",
                        ));
                    }
                }
                work(tx).await
            })
        },
    )
    .await
    .map_err(auth_failure)
}
fn response(result: Result<Value, Response>) -> Response {
    match result {
        Ok(value) => axum::Json(value).into_response(),
        Err(error) => error,
    }
}
async fn body<T: DeserializeOwned>(request: Request) -> Result<T, ApiError> {
    if !request
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(';')
                .next()
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
        })
    {
        return Err(invalid());
    }
    let bytes = to_bytes(request.into_body(), BODY_BYTES)
        .await
        .map_err(|_| invalid())?;
    serde_json::from_slice(&bytes).map_err(|_| invalid())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyBody {}

async fn ensure_available(conn: &mut PgConnection, actor: &CurrentActor) -> Result<(), ApiError> {
    if let Some(reason) = turn::paused_reason(conn, actor.id, actor.organization_id)
        .await
        .map_err(turn_error)?
    {
        return Err(turn_error(TurnError::Paused(reason)));
    }
    Ok(())
}
async fn owned_conversation(
    conn: &mut PgConnection,
    actor: &CurrentActor,
    id: Uuid,
) -> Result<Value, ApiError> {
    sqlx::query_scalar("SELECT jsonb_build_object('id',id,'status',status,'created_at',created_at,'updated_at',updated_at) FROM public.agent_conversations WHERE id=$1 AND user_id=$2 AND organization_id IS NOT DISTINCT FROM $3")
        .bind(id).bind(actor.id).bind(actor.organization_id).fetch_optional(conn).await.map_err(|_| ApiError::database())?.ok_or_else(missing)
}
async fn create_conversation(State(state): State<RouteState>, request: Request) -> Response {
    let headers = request.headers().clone();
    let parsed = body::<EmptyBody>(request).await;
    let result = authenticated(&state, &headers, "POST", CONVERSATIONS, false, move |tx| Box::pin(async move {
        parsed?;
        let actor = tx.actor().clone();
        ensure_available(tx.connection(), &actor).await?;
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO public.agent_conversations(id,user_id,organization_id) VALUES($1,$2,$3)")
            .bind(id).bind(actor.id).bind(actor.organization_id).execute(tx.connection()).await.map_err(|_| ApiError::database())?;
        owned_conversation(tx.connection(), &actor, id).await
    })).await;
    match result {
        Ok(value) => (StatusCode::CREATED, axum::Json(value)).into_response(),
        Err(error) => error,
    }
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryQuery {
    limit: Option<u16>,
    before_seq: Option<i32>,
}
async fn history(
    State(state): State<RouteState>,
    Path(id): Path<Uuid>,
    query: Result<Query<HistoryQuery>, axum::extract::rejection::QueryRejection>,
    headers: HeaderMap,
) -> Response {
    response(authenticated(&state, &headers, "GET", CONVERSATION, false, move |tx| Box::pin(async move {
        let actor = tx.actor().clone();
        let mut conversation = owned_conversation(tx.connection(), &actor, id).await?;
        ensure_available(tx.connection(), &actor).await?;
        let Query(query) = query.map_err(|_| invalid())?;
        let limit = query.limit.unwrap_or(100);
        if !(1..=200).contains(&limit) || query.before_seq.is_some_and(|value| value <= 0) { return Err(invalid()); }
        // Construct the public JSON in SQL. Never return stored content wholesale.
        let rows: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('seq',seq,'role',role,'created_at',created_at,'content',CASE WHEN role='user' THEN jsonb_build_object('text',content->'text') ELSE jsonb_build_object('reply',content->'reply','citations',content->'citations','drafts',content->'drafts','question',content->'question','assistant_seq',seq,'stopped',content->'stopped') END) FROM public.agent_messages WHERE conversation_id=$1 AND role IN ('user','assistant') AND ($2::integer IS NULL OR seq<$2) ORDER BY seq DESC LIMIT $3")
            .bind(id).bind(query.before_seq).bind(i64::from(limit)).fetch_all(tx.connection()).await.map_err(|_| ApiError::database())?;
        let messages: Vec<Value> = rows.into_iter().rev().map(safe_message).collect();
        let drafts: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',id,'side',side,'market_product',market_product,'delivery_point',delivery_point,'availability_window',availability_window,'quantity_mt',quantity_mt::text,'limit_price_usd',limit_price_usd::text,'expires_at',expires_at,'status',status,'created_at',created_at,'updated_at',updated_at) FROM public.order_drafts WHERE conversation_id=$1 AND user_id=$2 ORDER BY created_at DESC,id DESC LIMIT 200")
            .bind(id).bind(actor.id).fetch_all(tx.connection()).await.map_err(|_| ApiError::database())?;
        conversation["messages"] = json!(messages);
        conversation["drafts"] = json!(drafts);
        Ok(conversation)
    })).await)
}

fn safe_message(mut message: Value) -> Value {
    if message["role"] == "assistant" {
        let source = &message["content"];
        let citations: Vec<Value> = source["citations"].as_array().into_iter().flatten().filter_map(|item| {
            Some(json!({"tool_call":item["tool_call"].as_u64()?,"field":item["field"].as_str()?}))
        }).collect();
        let drafts: Vec<Value> = source["drafts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| {
                let id = item["id"].as_str()?.parse::<Uuid>().ok()?;
                Some(json!({"id":id,"status":"draft"}))
            })
            .collect();
        let question = source["question"]["question"]
            .as_str()
            .map(|text| json!({"question":text}));
        let stopped = source["stopped"].as_str().filter(|value| {
            matches!(
                *value,
                "tool_limit" | "contract_errors" | "timeout" | "model_failure"
            )
        });
        message["content"] = json!({"reply":source["reply"].as_str(),"citations":citations,"drafts":drafts,"question":question,"assistant_seq":message["seq"],"stopped":stopped});
    } else {
        message["content"] = json!({"text":message["content"]["text"].as_str()});
    }
    message
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageBody {
    text: String,
    #[serde(default)]
    untrusted: Vec<UntrustedBlock>,
}
impl MessageBody {
    fn validate(&self) -> Result<(), ApiError> {
        let valid = |text: &str, max: usize| {
            !text.trim().is_empty() && text.chars().count() <= max && !text.contains('\0')
        };
        if !valid(&self.text, 4_000) || self.untrusted.len() > 3 {
            return Err(invalid());
        }
        for block in &self.untrusted {
            if !valid(&block.label, 60) || !valid(&block.text, 20_000) {
                return Err(invalid());
            }
        }
        Ok(())
    }
}
async fn send_message(
    State(state): State<RouteState>,
    Path(id): Path<Uuid>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let parsed = body::<MessageBody>(request).await;
    let result = authenticated(&state, &headers, "POST", MESSAGES, false, move |tx| {
        Box::pin(async move {
            let actor = tx.actor().clone();
            let conversation = owned_conversation(tx.connection(), &actor, id).await?;
            if conversation["status"] != "open" {
                return Err(missing());
            }
            ensure_available(tx.connection(), &actor).await?;
            let body = parsed?;
            body.validate()?;
            Ok((actor, body))
        })
    })
    .await;
    let (actor, body) = match result {
        Ok(value) => value,
        Err(error) => return error,
    };
    let options = &state.app.config.phase2.agent;
    let deps = TurnDeps {
        model_client: state.model_client.clone(),
        model: options.model.clone(),
        limits: TurnLimits::default(),
    };
    let limits = AdmissionLimits {
        messages: options.rate_limit_messages,
        window_seconds: options.rate_limit_window_seconds,
        daily_tokens: options.daily_token_budget,
    };
    let context = ToolContext {
        state: state.app.clone(),
        user_id: actor.id,
        organization_id: actor.organization_id,
        conversation_id: id,
    };
    match turn::run_turn_with_admission(&deps, context, body.text, body.untrusted, limits).await {
        Ok(outcome) => axum::Json(outcome).into_response(),
        Err(error) => {
            let rate_limited = error == TurnError::RateLimited;
            let mut error = turn_error(error);
            if rate_limited {
                error.retry_after = u32::try_from(options.rate_limit_window_seconds).ok();
            }
            api_response(error)
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuditQuery {
    user_id: Option<Uuid>,
    organization_id: Option<Uuid>,
    conversation_id: Option<Uuid>,
    kind: Option<String>,
    before_id: Option<i64>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    limit: Option<u16>,
}
async fn list_audit(
    State(state): State<RouteState>,
    query: Result<Query<AuditQuery>, axum::extract::rejection::QueryRejection>,
    headers: HeaderMap,
) -> Response {
    response(authenticated(&state, &headers, "GET", AUDIT, true, move |tx| Box::pin(async move {
        let Query(query) = query.map_err(|_| invalid())?;
        let limit = query.limit.unwrap_or(100);
        if !(1..=200).contains(&limit) || query.before_id.is_some_and(|v| v<=0) || matches!((query.from,query.to),(Some(a),Some(b)) if a>b) || query.kind.as_ref().is_some_and(|kind| !matches!(kind.as_str(),"model_request_intent"|"model_request_receipt"|"tool_call_intent"|"tool_call_receipt"|"draft_created"|"control_changed")) { return Err(invalid()); }
        let events: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',id,'at',at,'user_id',user_id,'organization_id',organization_id,'conversation_id',conversation_id,'kind',kind,'payload',payload,'prev_hash',prev_hash,'hash',hash) FROM public.agent_audit_events WHERE ($1::uuid IS NULL OR user_id=$1) AND ($2::uuid IS NULL OR organization_id=$2) AND ($3::uuid IS NULL OR conversation_id=$3) AND ($4::text IS NULL OR kind=$4) AND ($5::bigint IS NULL OR id<$5) AND ($6::timestamptz IS NULL OR at>=$6) AND ($7::timestamptz IS NULL OR at<=$7) ORDER BY id DESC LIMIT $8")
            .bind(query.user_id).bind(query.organization_id).bind(query.conversation_id).bind(query.kind).bind(query.before_id).bind(query.from).bind(query.to).bind(i64::from(limit)).fetch_all(tx.connection()).await.map_err(|_| ApiError::database())?;
        Ok(json!({"events":events}))
    })).await)
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifyQuery {
    from_id: Option<i64>,
    to_id: Option<i64>,
}
async fn verify_audit(
    State(state): State<RouteState>,
    query: Result<Query<VerifyQuery>, axum::extract::rejection::QueryRejection>,
    headers: HeaderMap,
) -> Response {
    let bounds = authenticated(&state, &headers, "GET", VERIFY, true, move |_tx| {
        Box::pin(async move {
            let Query(query) = query.map_err(|_| invalid())?;
            if query.from_id.is_some_and(|v| v <= 0)
                || query.to_id.is_some_and(|v| v <= 0)
                || matches!((query.from_id,query.to_id),(Some(a),Some(b)) if a>b)
            {
                return Err(invalid());
            }
            Ok(query)
        })
    })
    .await;
    let bounds = match bounds {
        Ok(bounds) => bounds,
        Err(error) => return error,
    };
    match audit::verify_chain(&state.app.pools.read, bounds.from_id, bounds.to_id).await {
        Ok(report) => axum::Json(json!({"checked":report.checked,"first_broken_id":report.first_broken_id,"valid":report.first_broken_id.is_none()})).into_response(),
        Err(_) => refusal(StatusCode::SERVICE_UNAVAILABLE,"AGENT_AUDIT_UNAVAILABLE","The assistant evidence is unavailable.").into_response(),
    }
}
async fn list_controls(State(state): State<RouteState>, headers: HeaderMap) -> Response {
    response(authenticated(&state, &headers, "GET", CONTROLS, true, move |tx| Box::pin(async move {
        let controls: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(c) FROM public.agent_controls c ORDER BY scope,scope_id NULLS FIRST")
            .fetch_all(tx.connection()).await.map_err(|_| ApiError::database())?;
        Ok(json!({"controls":controls}))
    })).await)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlBody {
    scope: String,
    scope_id: Option<Uuid>,
    paused: bool,
    reason: String,
}
impl ControlBody {
    fn validate(&self) -> Result<(), ApiError> {
        if !matches!(self.scope.as_str(), "global" | "organization" | "user")
            || (self.scope == "global") != self.scope_id.is_none()
            || self.reason.trim().is_empty()
            || self.reason.chars().count() > 200
            || self.reason.contains('\0')
        {
            return Err(invalid());
        }
        Ok(())
    }
}
async fn set_control(State(state): State<RouteState>, request: Request) -> Response {
    let headers = request.headers().clone();
    let parsed = body::<ControlBody>(request).await;
    response(authenticated(&state, &headers, "PUT", CONTROLS, true, move |tx| Box::pin(async move {
        let body = parsed?;
        body.validate()?;
        let actor = tx.actor().clone();
        // Serialize absent-row upserts as well as existing rows, before taking
        // the global evidence-chain lock. The outer auth transaction is RW.
        sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(i64::from_be_bytes(*b"AGENTCTL")).execute(tx.connection()).await.map_err(|_| ApiError::database())?;
        let previous: Option<Value> = sqlx::query_scalar("SELECT to_jsonb(c) FROM public.agent_controls c WHERE scope=$1 AND scope_id IS NOT DISTINCT FROM $2 FOR UPDATE")
            .bind(&body.scope).bind(body.scope_id).fetch_optional(tx.connection()).await.map_err(|_| ApiError::database())?;
        if let Some(previous) = &previous {
            if previous["paused"] == body.paused && previous["reason"] == body.reason { return Ok(previous.clone()); }
        }
        let mut savepoint = tx.connection().begin().await.map_err(|_| ApiError::database())?;
        let control: Value = sqlx::query_scalar("INSERT INTO public.agent_controls(scope,scope_id,paused,reason,updated_by) VALUES($1,$2,$3,$4,$5) ON CONFLICT(scope,scope_id) DO UPDATE SET paused=EXCLUDED.paused,reason=EXCLUDED.reason,updated_by=EXCLUDED.updated_by,updated_at=now() RETURNING to_jsonb(agent_controls)")
            .bind(&body.scope).bind(body.scope_id).bind(body.paused).bind(&body.reason).bind(actor.id).fetch_one(&mut *savepoint).await.map_err(|_| ApiError::database())?;
        audit::append_in(&mut savepoint, AuditEvent { kind:AuditKind::ControlChanged, actor:AuditActor {user_id:Some(actor.id),organization_id:actor.organization_id,conversation_id:None},payload:json!({"scope":body.scope,"scope_id":body.scope_id,"paused":body.paused,"reason":body.reason,"previous":previous}) }).await.map_err(|_| refusal(StatusCode::SERVICE_UNAVAILABLE,"AGENT_AUDIT_UNAVAILABLE","The assistant evidence could not be saved."))?;
        savepoint.commit().await.map_err(|_| ApiError::database())?;
        Ok(control)
    })).await)
}

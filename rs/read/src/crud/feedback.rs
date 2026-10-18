//! F01 `POST /api/feedback` and F02 `GET /api/admin/feedback`.
//!
//! Port of `app/routers/feedback.py` for Phase 2 Task 10. The observed order is
//! JSON syntax, bearer authentication, current account state, role, typed
//! input, then endpoint quota. All database work happens inside the
//! authenticated transaction that owns the user-row lock.
//!
//! Two source rules are load-bearing and easy to lose:
//! - A message is stripped with Python's whitespace set and must not be blank;
//!   `page` metadata that is not a bare application path is dropped, never
//!   rejected.
//! - The admin view is identified support data: it joins the submitting user
//!   and their organization and never hides a null as an omitted key.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Extension, Router,
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::auth::limits::SharedLimits;
use crate::auth::request::{RequestAdmission, parse_json_body, validate_and_admit};
use crate::auth::{AuthError, AuthenticatedRequest, VerificationSettings, verify_authorization};
use crate::auth::{AuthenticatedError, AuthenticatedTransaction, run_authenticated};
use crate::routes::ports::pydantic_datetime;
use crate::runtime::http::{QueryFields, QueryKind, client_ip};
use crate::{ApiError, ApiErrorKind, AppState, Json};

/// `FeedbackSubmission.message` bounds, in Unicode code points.
const MAX_MESSAGE_CHARS: usize = 2000;
/// `FeedbackSubmission.page` bound, in Unicode code points.
const MAX_PAGE_CHARS: usize = 200;
/// Default and maximum page size of the admin listing.
const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;

/// A body so large that Starlette would have buffered it is refused rather than
/// read without bound. This is defensive only; Python imposes no route limit.
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// The instant the route uses for `created_at` and for the quota window.
///
/// Production uses the wall clock. Tests and the disposable harness inject one
/// fixed instant so a committed row and its response are reproducible.
#[derive(Clone)]
pub enum Clock {
    Wall,
    Fixed(DateTime<Utc>),
}

impl Clock {
    fn now(&self) -> DateTime<Utc> {
        match self {
            Self::Wall => Utc::now(),
            Self::Fixed(value) => *value,
        }
    }
}

/// Everything the two routes need that `AppState` does not carry: token
/// verification settings, the shared endpoint quota authority, and the clock.
///
/// `AppState` deliberately keeps no verification settings or rate-limit
/// authority, so the integration owner supplies this per-mount context. Task 22
/// must supply one process-wide `SharedLimits` for the `/api/admin/` prefix.
pub struct RouteContext {
    pub settings: Arc<VerificationSettings>,
    pub limits: SharedLimits,
    pub clock: Clock,
}

/// Mount F01 and F02 with the wall clock.
pub fn routes(settings: Arc<VerificationSettings>, limits: SharedLimits) -> Router<AppState> {
    routes_with_clock(settings, limits, Clock::Wall)
}

/// Mount F01 and F02 with an explicit clock, for deterministic contract runs.
pub fn routes_with_clock(
    settings: Arc<VerificationSettings>,
    limits: SharedLimits,
    clock: Clock,
) -> Router<AppState> {
    routes_with_context(Arc::new(RouteContext {
        settings,
        limits,
        clock,
    }))
}

/// Mount F01 and F02 from a prepared context.
pub fn routes_with_context(context: Arc<RouteContext>) -> Router<AppState> {
    Router::new()
        .route("/api/feedback", post(submit_feedback))
        .route("/api/admin/feedback", get(list_feedback))
        .layer(Extension(context))
}

/// A validated `FeedbackSubmission`.
#[derive(Debug, PartialEq, Eq)]
pub struct Submission {
    pub message: String,
    pub page: Option<String>,
}

/// A validated `limit`/`offset` pair for the admin listing.
#[derive(Debug, PartialEq, Eq)]
pub struct Pagination {
    pub limit: i64,
    pub offset: i64,
}

async fn submit_feedback(State(state): State<AppState>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let Some(context) = parts.extensions.get::<Arc<RouteContext>>().cloned() else {
        return ApiError::internal().into_response();
    };
    let now = context.clock.now();
    let client = client_ip(
        parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|peer| peer.0.ip()),
        parts
            .headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok()),
    );

    // Rule 1: JSON syntax is decided before authentication, exactly as
    // FastAPI parses the body before it solves the security dependency.
    let raw = match axum::body::to_bytes(body, MAX_BODY_BYTES).await {
        Ok(raw) => raw,
        Err(_) => return ApiError::internal().into_response(),
    };
    let payload = if raw.is_empty() {
        Value::Null
    } else {
        match parse_json_body(&raw) {
            Ok(payload) => payload,
            Err(error) => return error.into_response(),
        }
    };

    // Rule 2: bearer verification. No account row is read yet.
    let access = match verify_authorization(bearer(&parts.headers), &context.settings, now) {
        Ok(access) => access,
        Err(error) => return auth_error_response(error),
    };

    let admission = RequestAdmission {
        client,
        path: "/api/feedback".into(),
        now_seconds: now.timestamp().max(0) as u64,
        limits: context.limits.clone(),
    };
    let support_present = crate::auth::scope::support_present(&parts.headers);
    let result = run_authenticated(
        &state,
        access,
        AuthenticatedRequest::new("POST", "/api/feedback", support_present),
        move |transaction| {
            Box::pin(async move {
                // Rule 3 and 4: account state, then typed input, then quota.
                let submission = validate_and_admit(payload, &admission, validate_submission)?;
                insert_feedback(transaction, submission, now).await
            })
        },
    )
    .await;
    match result {
        Ok(response) => response,
        Err(error) => authenticated_error_response(error),
    }
}

async fn list_feedback(State(state): State<AppState>, request: Request) -> Response {
    let (parts, _body) = request.into_parts();
    let Some(context) = parts.extensions.get::<Arc<RouteContext>>().cloned() else {
        return ApiError::internal().into_response();
    };
    let now = context.clock.now();
    let client = client_ip(
        parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|peer| peer.0.ip()),
        parts
            .headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok()),
    );

    let access = match verify_authorization(bearer(&parts.headers), &context.settings, now) {
        Ok(access) => access,
        Err(error) => return auth_error_response(error),
    };

    let admission = RequestAdmission {
        client,
        path: "/api/admin/feedback".into(),
        now_seconds: now.timestamp().max(0) as u64,
        limits: context.limits.clone(),
    };
    let query = parts.uri.query().unwrap_or_default().to_owned();
    let support_present = crate::auth::scope::support_present(&parts.headers);
    let result = run_authenticated(
        &state,
        access,
        AuthenticatedRequest::new("GET", "/api/admin/feedback", support_present),
        move |transaction| {
            Box::pin(async move {
                // `require_role` reads current state before the endpoint's own
                // query parameters are validated, so the role check comes first.
                if transaction.actor().role.as_deref() != Some("ADMIN") {
                    return Err(forbidden());
                }
                // A GET has no body; the shared seam still enforces typed
                // validation before endpoint quota.
                let pagination =
                    validate_and_admit(Value::Null, &admission, |_| validate_query(&query))?;
                select_feedback(transaction, pagination).await
            })
        },
    )
    .await;
    match result {
        Ok(response) => response,
        Err(error) => authenticated_error_response(error),
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
}

fn forbidden() -> ApiError {
    ApiError {
        status: StatusCode::FORBIDDEN,
        detail: json!("Forbidden"),
        retry_after: None,
        kind: ApiErrorKind::Expected,
    }
}

fn validation_error(entries: Vec<Value>) -> ApiError {
    ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        detail: Value::Array(entries),
        retry_after: None,
        kind: ApiErrorKind::Expected,
    }
}

fn auth_error_response(error: AuthError) -> Response {
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

/// Convert the authenticated-transaction outcome into the route response.
///
/// Section 5 keeps the CRUD database envelope: an ambiguous commit becomes the
/// same 500 `{"detail":"Database operation failed."}` as any other database
/// failure. The market retry mapping never applies on this path. Exposed to the
/// contract test so the ambiguous-commit envelope is asserted without a fault
/// injector.
pub fn authenticated_error_response(error: AuthenticatedError) -> Response {
    match error {
        AuthenticatedError::Authentication(error) => auth_error_response(error),
        AuthenticatedError::Database(error) => error.into_response(),
        AuthenticatedError::AmbiguousCommit => ApiError::database().into_response(),
    }
}

/// Validate `FeedbackSubmission` in Pydantic v2 order.
///
/// Exposed to the contract test so the validation matrix runs without a
/// database: Pydantic validates the body before the endpoint's quota and
/// before any row is written.
pub fn validate_submission(payload: Value) -> Result<Submission, ApiError> {
    // FastAPI treats a missing or JSON-null required body as a missing field.
    if payload.is_null() {
        return Err(validation_error(vec![json!({
            "type": "missing", "loc": ["body"],
            "msg": "Field required", "input": Value::Null,
        })]));
    }
    let Some(object) = payload.as_object() else {
        return Err(validation_error(vec![json!({
            "type": "model_attributes_type",
            "loc": ["body"],
            "msg": "Input should be a valid dictionary or object to extract fields from",
            "input": payload,
        })]));
    };

    let mut errors = Vec::new();
    // Pydantic reports a missing field against the whole parent object as its
    // `input`, so the parent travels down with the field lookup.
    let message = validate_message(object.get("message"), &payload, &mut errors);
    let page = validate_page(object.get("page"), &mut errors);
    if !errors.is_empty() {
        return Err(validation_error(errors));
    }
    Ok(Submission {
        // `message` is always present when no error was recorded.
        message: message.expect("validated message"),
        page,
    })
}

fn validate_message(
    value: Option<&Value>,
    parent: &Value,
    errors: &mut Vec<Value>,
) -> Option<String> {
    let Some(value) = value else {
        errors.push(json!({
            "type": "missing", "loc": ["body", "message"],
            "msg": "Field required", "input": parent,
        }));
        return None;
    };
    let Some(text) = value.as_str() else {
        errors.push(json!({
            "type": "string_type", "loc": ["body", "message"],
            "msg": "Input should be a valid string", "input": value,
        }));
        return None;
    };
    let length = text.chars().count();
    if length < 1 {
        errors.push(json!({
            "type": "string_too_short", "loc": ["body", "message"],
            "msg": "String should have at least 1 character",
            "input": value, "ctx": {"min_length": 1},
        }));
        return None;
    }
    if length > MAX_MESSAGE_CHARS {
        errors.push(json!({
            "type": "string_too_long", "loc": ["body", "message"],
            "msg": format!("String should have at most {MAX_MESSAGE_CHARS} characters"),
            "input": value, "ctx": {"max_length": MAX_MESSAGE_CHARS},
        }));
        return None;
    }
    // The field validator runs after the constraints and returns the stripped
    // value, so the stored message never carries outer whitespace.
    let stripped = text.trim_matches(crate::market::whitespace::is_python_whitespace);
    if stripped.is_empty() {
        errors.push(json!({
            "type": "value_error", "loc": ["body", "message"],
            "msg": "Value error, message must not be blank",
            "input": value, "ctx": {"error": {}},
        }));
        return None;
    }
    Some(stripped.to_owned())
}

fn validate_page(value: Option<&Value>, errors: &mut Vec<Value>) -> Option<String> {
    let value = match value {
        None | Some(Value::Null) => return None,
        Some(value) => value,
    };
    let Some(text) = value.as_str() else {
        errors.push(json!({
            "type": "string_type", "loc": ["body", "page"],
            "msg": "Input should be a valid string", "input": value,
        }));
        return None;
    };
    if text.chars().count() > MAX_PAGE_CHARS {
        errors.push(json!({
            "type": "string_too_long", "loc": ["body", "page"],
            "msg": format!("String should have at most {MAX_PAGE_CHARS} characters"),
            "input": value, "ctx": {"max_length": MAX_PAGE_CHARS},
        }));
        return None;
    }
    // A full URL, query string, fragment, or doubled slash is dropped metadata,
    // never a reason to reject the message itself.
    if !text.starts_with('/') || text.contains('?') || text.contains('#') || text.contains("//") {
        return None;
    }
    Some(text.to_owned())
}

/// Validate `limit`/`offset` the way FastAPI validates `Query` parameters.
pub fn validate_query(query: &str) -> Result<Pagination, ApiError> {
    let fields = QueryFields::parse(query);
    let mut errors = Vec::new();
    let limit = match fields.validate(
        "limit",
        QueryKind::Integer {
            minimum: 1,
            maximum: MAX_LIMIT,
        },
        false,
    ) {
        Ok(Some(crate::runtime::http::QueryValue::Integer(value))) => value,
        Ok(_) => DEFAULT_LIMIT,
        Err(error) => {
            errors.push(first_error(error));
            DEFAULT_LIMIT
        }
    };
    let offset = match fields.validate(
        "offset",
        QueryKind::Integer {
            minimum: 0,
            maximum: i64::MAX,
        },
        false,
    ) {
        Ok(Some(crate::runtime::http::QueryValue::Integer(value))) => value,
        Ok(_) => 0,
        Err(error) => {
            errors.push(first_error(error));
            0
        }
    };
    if !errors.is_empty() {
        return Err(validation_error(errors));
    }
    Ok(Pagination { limit, offset })
}

/// `QueryFields::validate` already builds the Pydantic-shaped error; recover
/// its single entry so two failing parameters report both, in declaration
/// order, inside one body.
fn first_error(error: ApiError) -> Value {
    match error.detail {
        Value::Array(mut entries) if !entries.is_empty() => entries.remove(0),
        other => other,
    }
}

async fn insert_feedback(
    transaction: &mut AuthenticatedTransaction<'_>,
    submission: Submission,
    created_at: DateTime<Utc>,
) -> Result<Response, ApiError> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO feedback_entries (id, user_id, message, page, created_at) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(transaction.actor().id)
    .bind(&submission.message)
    .bind(&submission.page)
    .bind(created_at)
    .execute(transaction.connection())
    .await
    .map_err(|error| query_error("/api/feedback", "POST", &error))?;
    Ok((
        StatusCode::CREATED,
        axum::Json(json!({
            "id": id,
            "created_at": pydantic_datetime(created_at),
        })),
    )
        .into_response())
}

#[derive(sqlx::FromRow)]
struct FeedbackRow {
    id: Uuid,
    created_at: DateTime<Utc>,
    message: String,
    page: Option<String>,
    user_email: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
    org_name: Option<String>,
}

async fn select_feedback(
    transaction: &mut AuthenticatedTransaction<'_>,
    pagination: Pagination,
) -> Result<Response, ApiError> {
    // The count and the page share one join shape, so a foreign key that cannot
    // resolve still contributes its row to both.
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ( \
             SELECT feedback_entries.id \
             FROM feedback_entries \
             LEFT OUTER JOIN users ON feedback_entries.user_id = users.id \
             LEFT OUTER JOIN organizations ON users.organization_id = organizations.id \
         ) AS collected",
    )
    .fetch_one(transaction.connection())
    .await
    .map_err(|error| query_error("/api/admin/feedback", "GET", &error))?;

    let rows = sqlx::query_as::<_, FeedbackRow>(
        "SELECT feedback_entries.id AS id, \
                feedback_entries.created_at AS created_at, \
                feedback_entries.message AS message, \
                feedback_entries.page AS page, \
                users.email AS user_email, \
                users.first_name AS first_name, \
                users.last_name AS last_name, \
                organizations.name AS org_name \
         FROM feedback_entries \
         LEFT OUTER JOIN users ON feedback_entries.user_id = users.id \
         LEFT OUTER JOIN organizations ON users.organization_id = organizations.id \
         ORDER BY feedback_entries.created_at DESC \
         LIMIT $1 OFFSET $2",
    )
    .bind(pagination.limit)
    .bind(pagination.offset)
    .fetch_all(transaction.connection())
    .await
    .map_err(|error| query_error("/api/admin/feedback", "GET", &error))?;

    let items: Vec<Json> = rows.iter().map(entry_response).collect();
    Ok(axum::Json(json!({"items": items, "total": total})).into_response())
}

/// Project one admin row. Field order follows `AdminFeedbackEntry`, because a
/// recorded oracle body is compared key by key.
fn entry_response(row: &FeedbackRow) -> Json {
    json!({
        "id": row.id,
        "created_at": pydantic_datetime(row.created_at),
        "message": row.message,
        "page": row.page,
        "user_email": row.user_email,
        "user_name": display_name(row.first_name.as_deref(), row.last_name.as_deref()),
        "org_name": row.org_name,
    })
}

/// `" ".join(part for part in (first_name, last_name) if part) or None`. An
/// empty string is falsy in Python and is therefore skipped; a whitespace-only
/// name is truthy and is kept.
fn display_name(first: Option<&str>, last: Option<&str>) -> Option<String> {
    let parts: Vec<&str> = [first, last]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

fn query_error(path: &str, method: &str, error: &sqlx::Error) -> ApiError {
    crate::runtime::admission::log_sql_error("work", error);
    let sqlstate = match error {
        sqlx::Error::Database(database) => database.code().map(|code| code.into_owned()),
        _ => None,
    };
    crate::database_error(path, method, sqlstate.as_deref())
}

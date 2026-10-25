//! N01–N04: the authenticated notification reads and read-state writes.
//!
//! Python source: `app/routers/notifications.py` (`get_notifications`,
//! `get_unread_count`, `mark_as_read`, `mark_all_as_read`) and
//! `NotificationResponse` in the same file.
//!
//! Ported behaviour, in FastAPI's own order:
//!
//! 1. No request body is declared, so the body is never read and malformed JSON
//!    is ignored. Only `notification_id` is typed input.
//! 2. The `get_current_user` dependency runs before the endpoint's own path and
//!    query parameters, so a refused bearer is `401` even when the path or query
//!    is also invalid. Verification therefore happens before
//!    `run_authenticated`, and typed validation happens inside its work closure.
//! 3. `skip` is `Query(0, ge=0)` and `limit` is `Query(50, ge=1, le=100)`, both
//!    validated in declaration order and reported together.
//!
//! The list and the count are recipient-bound and skip rows whose `data`
//! carries a truthy `_email_only` flag; the two writes are recipient-bound but
//! deliberately include those rows, because Python's mark queries never look at
//! `data`. `_email_only` and every other top-level `_email_` key is redacted
//! from the response `data` object only; nested keys are preserved, `null` and
//! `{}` stay distinct, and no other column is hidden.
//!
//! Known boundaries are recorded in `tests/rust_contract/phase2/notifications.json`.

use std::sync::Arc;

use axum::{
    Extension, Router,
    extract::{Path, Request as AxumRequest, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, patch},
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

#[path = "notification_authority.rs"]
mod authority;
use crate::{
    ApiError, ApiErrorKind, AppState,
    auth::AuthenticatedRequest,
    auth::request::validate_and_admit,
    auth::run_authenticated,
    auth::scope::support_present,
    database_error,
    runtime::admission::log_sql_error,
    runtime::http::{QueryFields, QueryKind, QueryValue},
};
pub use authority::NotificationAuthority;
use authority::{NotificationError, client_of};

const LIST_PATH: &str = "/api/notifications";
const COUNT_PATH: &str = "/api/notifications/unread-count";
const MARK_PATH: &str = "/api/notifications/{notification_id}/read";
const MARK_ALL_PATH: &str = "/api/notifications/read-all";

/// The `data ->>` key and the coalesce fallback are parameters in the Python
/// statement, so they are parameters here too and the statement text matches.
const EMAIL_ONLY_KEY: &str = "_email_only";

/// `select(Notification).filter(recipient_id, email_only is false)`
/// `.order_by(created_at.desc()).offset(skip).limit(limit)`
///
/// The column list is the mapper's column order, because the fixture pins the
/// Python-compiled statement. `notifications.recipient_id` is selected and not
/// projected, exactly as the Python response model drops it.
///
/// `data::text` is the one textual difference from Python: this workspace's
/// sqlx build has no `json` feature, so the column is read as text and parsed
/// with `serde_json` instead of being decoded by the driver. The cast changes
/// no value, no order and no null.
pub const LIST_SQL: &str = "\
SELECT notifications.id, notifications.recipient_id, notifications.type, \
       notifications.title, notifications.message, notifications.data::text, \
       notifications.is_read, notifications.created_at
FROM notifications
WHERE notifications.recipient_id = $1::uuid AND coalesce(CAST((notifications.data ->> $2) AS BOOLEAN), $3) IS false
ORDER BY notifications.created_at DESC
LIMIT $4 OFFSET $5";

/// `select(count()).select_from(Notification).filter(recipient_id, not read,
/// email_only is false)`
pub const COUNT_SQL: &str = "\
SELECT count(*) AS count_1
FROM notifications
WHERE notifications.recipient_id = $1::uuid AND notifications.is_read = false AND coalesce(CAST((notifications.data ->> $2) AS BOOLEAN), $3) IS false";

/// `select(Notification).filter(id, recipient_id).first()`
///
/// `first()` fetches one row without adding `LIMIT`, and the ORM load maps every
/// column, so this statement keeps the full column list too.
pub const FIND_SQL: &str = "\
SELECT notifications.id, notifications.recipient_id, notifications.type, \
       notifications.title, notifications.message, notifications.data::text, \
       notifications.is_read, notifications.created_at
FROM notifications
WHERE notifications.id = $1::uuid AND notifications.recipient_id = $2::uuid";

/// The ORM flush of `notification.is_read = True`, which updates the changed
/// column keyed by the primary key. The new value is a bound parameter, exactly
/// as SQLAlchemy's unit of work sends it.
pub const MARK_SQL: &str =
    "UPDATE notifications SET is_read = $1 WHERE notifications.id = $2::uuid";

/// `update(Notification).where(recipient_id, not read).values(is_read=True)`
///
/// `.values()` and `== False` render differently in SQLAlchemy: the assigned
/// value is a parameter and the comparison is the literal `false`. Keep both.
pub const MARK_ALL_SQL: &str = "\
UPDATE notifications SET is_read = $1
WHERE notifications.recipient_id = $2::uuid AND notifications.is_read = false";

/// N01–N04 in one route table, carrying this group's composition authority.
///
/// The integration owner merges the result into the serving router and adds the
/// four paths to `runtime::http::MOUNTED_GET_PATHS` where a private path needs
/// Starlette's trailing-slash redirect and its `405 Allow` list.
pub fn routes(authority: NotificationAuthority) -> Router<AppState> {
    Router::new()
        .route(LIST_PATH, get(list))
        .route(COUNT_PATH, get(count))
        .route(MARK_PATH, patch(mark_read))
        .route(MARK_ALL_PATH, patch(mark_all_read))
        .layer(Extension(Arc::new(authority)))
}

/// N01 query after Python-compatible validation.
#[derive(Debug, Clone, Copy)]
pub struct NotificationQuery {
    pub skip: i64,
    pub limit: i64,
}

/// One `notifications` row as the response projects it.
///
/// `data` is the raw `json` column text, parsed in [`redact_email_state`]; see
/// [`LIST_SQL`] for why the driver does not decode it.
#[derive(sqlx::FromRow)]
pub struct NotificationRow {
    pub id: Uuid,
    #[sqlx(rename = "type")]
    pub notification_type: String,
    pub title: String,
    pub message: String,
    pub data: Option<String>,
    pub is_read: bool,
    pub created_at: DateTime<Utc>,
}

/// Validate `skip` then `limit` exactly as FastAPI declares them.
///
/// Both parameters are validated before the endpoint runs, so both failures are
/// reported in one `detail` array in declaration order. `skip` has no declared
/// maximum; `i64::MAX` is the widest offset the PostgreSQL protocol can carry,
/// so a larger value is refused here instead of reaching the driver.
pub fn notification_query(raw_query: &str) -> Result<NotificationQuery, ApiError> {
    let fields = QueryFields::parse(raw_query);
    let skip = integer(&fields, "skip", 0, i64::MAX, 0);
    let limit = integer(&fields, "limit", 1, 100, 50);
    let mut errors = Vec::new();
    for result in [&skip, &limit] {
        if let Err(error) = result
            && let Some(entries) = error.detail.as_array()
        {
            errors.extend(entries.iter().cloned());
        }
    }
    if !errors.is_empty() {
        return Err(ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            detail: Value::Array(errors),
            retry_after: None,
            kind: ApiErrorKind::Expected,
        });
    }
    Ok(NotificationQuery {
        skip: skip?,
        limit: limit?,
    })
}

fn integer(
    fields: &QueryFields,
    name: &str,
    minimum: i64,
    maximum: i64,
    default: i64,
) -> Result<i64, ApiError> {
    match fields.validate(name, QueryKind::Integer { minimum, maximum }, false)? {
        None => Ok(default),
        Some(QueryValue::Integer(value)) => Ok(value),
        Some(QueryValue::Uuid(_)) => Err(ApiError::internal()),
    }
}

/// Validate the typed path parameter `notification_id: uuid.UUID`.
///
/// FastAPI locates a path-parameter failure at `["path", name]`; the message and
/// context are pydantic's, whose UUID diagnostics the shared validator already
/// reproduces for query parameters.
pub fn parse_notification_id(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|cause| {
        let cause = cause.to_string();
        // Pydantic's UUID parser reports character positions as one-based.
        let cause = if cause.starts_with("invalid character:") {
            match cause.rsplit_once(" at ") {
                Some((message, position)) => position
                    .parse::<usize>()
                    .map(|position| format!("{message} at {}", position + 1))
                    .unwrap_or(cause),
                None => cause,
            }
        } else {
            cause
        };
        ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            detail: json!([{
                "type": "uuid_parsing",
                "loc": ["path", "notification_id"],
                "msg": format!("Input should be a valid UUID, {cause}"),
                "input": raw,
                "ctx": {"error": cause},
            }]),
            retry_after: None,
            kind: ApiErrorKind::Expected,
        }
    })
}

/// Project one row to `NotificationResponse`, in the model's field order.
///
/// A stored value that is not valid JSON cannot happen for a `json` column, but
/// it is reported as the same database failure the Python driver would raise
/// rather than a panic or a silently dropped field.
pub fn notification_response(row: &NotificationRow) -> Result<Value, ApiError> {
    Ok(json!({
        "id": row.id,
        "type": row.notification_type,
        "title": row.title,
        "message": row.message,
        "data": redact_email_state(row.data.as_deref())?,
        "is_read": row.is_read,
        "created_at": crate::routes::ports::pydantic_datetime(row.created_at),
    }))
}

/// Python's `hide_email_delivery_state` validator, applied to the response only.
///
/// `None` stays `null`, an object loses its top-level `_email_` keys and keeps
/// the order of the rest. Other shapes fail response validation because the
/// Pydantic field is `dict | None`, not a general JSON value.
pub fn redact_email_state(data: Option<&str>) -> Result<Value, ApiError> {
    let Some(text) = data else {
        return Ok(Value::Null);
    };
    let value: Value = serde_json::from_str(text).map_err(|_| ApiError::database())?;
    Ok(match value {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .filter(|(key, _)| !key.starts_with("_email_"))
                .collect(),
        ),
        Value::Null => Value::Null,
        _ => return Err(ApiError::internal()),
    })
}

/// N01: the recipient's visible notifications, newest first.
pub async fn select_notifications(
    connection: &mut PgConnection,
    recipient: Uuid,
    query: NotificationQuery,
) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query_as::<_, NotificationRow>(LIST_SQL)
        .bind(recipient)
        .bind(EMAIL_ONLY_KEY)
        .bind(false)
        .bind(query.limit)
        .bind(query.skip)
        .fetch_all(&mut *connection)
        .await
        .map_err(|error| query_error(LIST_PATH, "GET", &error))?;
    rows.iter().map(notification_response).collect()
}

/// N02: the recipient's unread notifications that carry no email-only state.
pub async fn select_unread_count(
    connection: &mut PgConnection,
    recipient: Uuid,
) -> Result<i64, ApiError> {
    sqlx::query_scalar::<_, i64>(COUNT_SQL)
        .bind(recipient)
        .bind(EMAIL_ONLY_KEY)
        .bind(false)
        .fetch_one(&mut *connection)
        .await
        .map_err(|error| query_error(COUNT_PATH, "GET", &error))
}

/// N03: mark one recipient-owned notification read.
///
/// A missing row and a row owned by another user are both `404` with Python's
/// detail string, and the update is keyed by the primary key only.
pub async fn mark_notification_read(
    connection: &mut PgConnection,
    recipient: Uuid,
    notification: Uuid,
) -> Result<(), ApiError> {
    let found = sqlx::query_as::<_, NotificationRow>(FIND_SQL)
        .bind(notification)
        .bind(recipient)
        .fetch_optional(&mut *connection)
        .await
        .map_err(|error| query_error(MARK_PATH, "PATCH", &error))?;
    if found.is_none() {
        return Err(ApiError {
            status: StatusCode::NOT_FOUND,
            detail: json!("Notification not found"),
            retry_after: None,
            kind: ApiErrorKind::Expected,
        });
    }
    sqlx::query(MARK_SQL)
        .bind(true)
        .bind(notification)
        .execute(&mut *connection)
        .await
        .map_err(|error| query_error(MARK_PATH, "PATCH", &error))?;
    Ok(())
}

/// N04: mark every unread notification of the recipient read.
///
/// Email-only rows are included: Python's update filters on the recipient and
/// `is_read` only, and already-read rows are left alone.
pub async fn mark_all_notifications_read(
    connection: &mut PgConnection,
    recipient: Uuid,
) -> Result<(), ApiError> {
    sqlx::query(MARK_ALL_SQL)
        .bind(true)
        .bind(recipient)
        .execute(&mut *connection)
        .await
        .map_err(|error| query_error(MARK_ALL_PATH, "PATCH", &error))?;
    Ok(())
}

fn success() -> Response {
    axum::Json(json!({"status": "success"})).into_response()
}

async fn list(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<NotificationAuthority>>,
    request: AxumRequest,
) -> Result<Response, NotificationError> {
    let access = authority
        .authenticate(request.headers())
        .map_err(NotificationError::Authentication)?;
    let admission = authority.request_admission(LIST_PATH, client_of(&request));
    let authenticated =
        AuthenticatedRequest::new("GET", LIST_PATH, support_present(request.headers()));
    let raw = request.uri().query().unwrap_or_default().to_owned();
    let rows = run_authenticated(&state, access, authenticated, move |transaction| {
        Box::pin(async move {
            let query =
                validate_and_admit(Value::Null, &admission, transaction.actor().id, |_| {
                    notification_query(&raw)
                })?;
            let recipient = transaction.actor().id;
            select_notifications(transaction.connection(), recipient, query).await
        })
    })
    .await?;
    Ok(axum::Json(Value::Array(rows)).into_response())
}

async fn count(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<NotificationAuthority>>,
    request: AxumRequest,
) -> Result<Response, NotificationError> {
    let access = authority
        .authenticate(request.headers())
        .map_err(NotificationError::Authentication)?;
    let admission = authority.request_admission(COUNT_PATH, client_of(&request));
    let authenticated =
        AuthenticatedRequest::new("GET", COUNT_PATH, support_present(request.headers()));
    let count = run_authenticated(&state, access, authenticated, move |transaction| {
        Box::pin(async move {
            validate_and_admit(Value::Null, &admission, transaction.actor().id, Ok)?;
            let recipient = transaction.actor().id;
            select_unread_count(transaction.connection(), recipient).await
        })
    })
    .await?;
    Ok(axum::Json(json!({"count": count})).into_response())
}

async fn mark_read(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<NotificationAuthority>>,
    Path(notification): Path<String>,
    request: AxumRequest,
) -> Result<Response, NotificationError> {
    let access = authority
        .authenticate(request.headers())
        .map_err(NotificationError::Authentication)?;
    let admission = authority.request_admission(MARK_PATH, client_of(&request));
    let authenticated =
        AuthenticatedRequest::new("PATCH", MARK_PATH, support_present(request.headers()));
    let response = run_authenticated(&state, access, authenticated, move |transaction| {
        Box::pin(async move {
            let notification =
                validate_and_admit(Value::Null, &admission, transaction.actor().id, |_| {
                    parse_notification_id(&notification)
                })?;
            let recipient = transaction.actor().id;
            mark_notification_read(transaction.connection(), recipient, notification).await?;
            Ok(success())
        })
    })
    .await?;
    Ok(response)
}

async fn mark_all_read(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<NotificationAuthority>>,
    request: AxumRequest,
) -> Result<Response, NotificationError> {
    let access = authority
        .authenticate(request.headers())
        .map_err(NotificationError::Authentication)?;
    let admission = authority.request_admission(MARK_ALL_PATH, client_of(&request));
    let authenticated =
        AuthenticatedRequest::new("PATCH", MARK_ALL_PATH, support_present(request.headers()));
    let response = run_authenticated(&state, access, authenticated, move |transaction| {
        Box::pin(async move {
            validate_and_admit(Value::Null, &admission, transaction.actor().id, Ok)?;
            let recipient = transaction.actor().id;
            mark_all_notifications_read(transaction.connection(), recipient).await?;
            Ok(success())
        })
    })
    .await?;
    Ok(response)
}

/// The route modules share this adapter: bounded log fields, then the shared
/// database-error contract with the request's own method.
fn query_error(path: &str, method: &str, error: &sqlx::Error) -> ApiError {
    log_sql_error("work", error);
    let sqlstate = error
        .as_database_error()
        .and_then(|database| database.code())
        .map(|code| code.into_owned());
    database_error(path, method, sqlstate.as_deref())
}

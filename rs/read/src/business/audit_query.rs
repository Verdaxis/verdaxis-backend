//! N03: the administrator audit-log read.
//!
//! Source: `app/routers/audit.py`. The action values it filters on are
//! Python's dotted registry (`app/services/audit_actions.py`); the response
//! keeps Python's `ip_address`, `request_id` and null columns exactly, and
//! the route writes nothing.
//!
//! The route inventory pins two explicit contract changes over the source:
//! `limit` is 1..=200 (a negative or zero limit is a request refusal, never a
//! PostgreSQL error) and `offset` is capped at 10000.

use std::sync::Arc;

use axum::{
    Router,
    extract::{ConnectInfo, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::{
    ApiError, AppState,
    auth::{
        AuthenticatedError, AuthenticatedRequest, VerificationSettings,
        limits::{BusinessLimitKeys, SharedLimits},
        run_authenticated, verify_authorization,
    },
    business::error::{authentication, database_failure, refusal},
    runtime::http::client_ip,
};

/// N03's exact path.
pub const AUDIT_PATH: &str = "/api/admin/audit-logs";

/// The route's pinned limit bounds: default 50, 1..=200.
pub const LIMIT_DEFAULT: i64 = 50;
pub const LIMIT_MINIMUM: i64 = 1;
pub const LIMIT_MAXIMUM: i64 = 200;
/// The pinned offset cap. A deeper scan needs narrower filtering, not an
/// unbounded ordered walk.
pub const OFFSET_MAXIMUM: i64 = 10_000;

/// The validated N03 query. Every bound is checked before SQL runs, so an
/// invalid parameter is a request refusal and never a driver error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditQuery {
    action: Option<String>,
    resource_type: Option<String>,
    user_id: Option<Uuid>,
    limit: i64,
    offset: i64,
}

fn validation(entries: Vec<Value>) -> ApiError {
    ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        detail: Value::Array(entries),
        retry_after: None,
        kind: crate::ApiErrorKind::Expected,
    }
}

impl AuditQuery {
    /// Build the query from decoded pairs, for callers and tests that hold
    /// the parameters already.
    pub fn from_pairs(pairs: &[(&str, &str)]) -> Result<Self, ApiError> {
        Self::from_query(
            &pairs
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join("&"),
        )
    }

    /// Parse and validate the query string the way FastAPI validates the
    /// declared `Query` parameters, with the pinned bounds.
    pub fn from_query(query: &str) -> Result<Self, ApiError> {
        use crate::runtime::http::{QueryFields, QueryKind, QueryValue};
        let fields = QueryFields::parse(query);
        let mut errors: Vec<Value> = Vec::new();
        let limit = Self::integer(
            &fields,
            "limit",
            LIMIT_MINIMUM,
            LIMIT_MAXIMUM,
            LIMIT_DEFAULT,
            &mut errors,
        );
        let offset = Self::integer(&fields, "offset", 0, OFFSET_MAXIMUM, 0, &mut errors);
        let user_id = match fields.validate("user_id", QueryKind::Uuid, false) {
            Ok(Some(QueryValue::Uuid(value))) => Some(value),
            Ok(_) => None,
            Err(error) => {
                errors.extend(error.detail.as_array().cloned().unwrap_or_default());
                None
            }
        };
        if !errors.is_empty() {
            return Err(validation(errors));
        }
        // Python's truthiness test: only a non-empty string filters.
        let action = fields
            .raw("action")
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let resource_type = fields
            .raw("resource_type")
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        Ok(Self {
            action,
            resource_type,
            user_id,
            limit,
            offset,
        })
    }

    fn integer(
        fields: &crate::runtime::http::QueryFields,
        name: &str,
        minimum: i64,
        maximum: i64,
        default: i64,
        errors: &mut Vec<Value>,
    ) -> i64 {
        match fields.validate(
            name,
            crate::runtime::http::QueryKind::Integer { minimum, maximum },
            false,
        ) {
            Ok(Some(crate::runtime::http::QueryValue::Integer(value))) => value,
            Ok(_) => default,
            Err(error) => {
                errors.extend(error.detail.as_array().cloned().unwrap_or_default());
                default
            }
        }
    }

    pub fn limit(&self) -> i64 {
        self.limit
    }

    pub fn offset(&self) -> i64 {
        self.offset
    }

    pub fn action(&self) -> Option<&str> {
        self.action.as_deref()
    }

    pub fn resource_type(&self) -> Option<&str> {
        self.resource_type.as_deref()
    }

    pub fn user_id(&self) -> Option<Uuid> {
        self.user_id
    }
}

/// One projected audit row. `changes` is the raw `jsonb` text and is parsed
/// here, so the driver's JSON handling never rewrites the stored value; the
/// timestamp is formatted in Rust so the wire value is Pydantic's, not the
/// session's time-zone spelling.
#[derive(sqlx::FromRow)]
pub struct AuditRow {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub changes: Option<String>,
    pub ip_address: Option<String>,
    pub request_id: Option<String>,
    pub timestamp: DateTime<Utc>,
}

impl AuditRow {
    /// `AuditLogResponse`, in the model's field order. A null column stays
    /// `null`, never an empty string.
    pub fn response(&self) -> Result<Value, ApiError> {
        let changes: Option<Value> = match &self.changes {
            Some(text) => Some(serde_json::from_str(text).map_err(|_| database_failure(None))?),
            None => None,
        };
        Ok(serde_json::json!({
            "id": self.id,
            "user_id": self.user_id,
            "action": self.action,
            "resource_type": self.resource_type,
            "resource_id": self.resource_id,
            "changes": changes,
            "ip_address": self.ip_address,
            "request_id": self.request_id,
            "timestamp": crate::routes::ports::pydantic_datetime(self.timestamp),
        }))
    }
}

/// Read one page in Python's descending timestamp order, newest first. The
/// deterministic tie-break keeps a page of equal timestamps stable, which the
/// source's single-key order does not guarantee.
pub async fn load_page(
    connection: &mut PgConnection,
    query: &AuditQuery,
) -> Result<Vec<Value>, ApiError> {
    const SQL: &str = "SELECT id,user_id,action,resource_type,resource_id,changes::text AS changes,\
                ip_address,request_id,timestamp\
         FROM audit_logs\
         WHERE ($1::text IS NULL OR action=$1)\
           AND ($2::text IS NULL OR resource_type=$2)\
           AND ($3::uuid IS NULL OR user_id=$3)\
         ORDER BY timestamp DESC, id\
         OFFSET $4 LIMIT $5";
    let rows: Vec<AuditRow> = sqlx::query_as::<_, AuditRow>(SQL)
    .bind(&query.action)
    .bind(&query.resource_type)
    .bind(query.user_id)
    .bind(query.offset)
    .bind(query.limit)
    // The prepared text is printed only on failure, and only in this debug
    // round; it contains no bound value.
    .fetch_all(connection)
    .await
    .map_err(|error| {
        eprintln!("SQL_DEBUG audit {error}\nSQL_TEXT audit <<{SQL}>>");
        database_failure(None)
    })?;
    rows.iter().map(AuditRow::response).collect()
}

#[derive(Clone)]
struct RouteState {
    app: AppState,
    verification: Arc<VerificationSettings>,
    limits: SharedLimits,
}

/// Component router only. The serving binary does not mount these routes.
pub fn router(app: AppState, verification: VerificationSettings, limits: SharedLimits) -> Router {
    Router::new()
        .route(AUDIT_PATH, get(audit_route))
        .with_state(RouteState {
            app,
            verification: Arc::new(verification),
            limits,
        })
}

/// N03's adapter. The inventory declares no endpoint quota for this route;
/// admission is still token, current account and ADMIN, in that order,
/// before any query parsing or read.
async fn audit_route(State(state): State<RouteState>, request: Request) -> Response {
    let now = Utc::now();
    let authorization = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let access = match verify_authorization(authorization.as_deref(), &state.verification, now) {
        Ok(access) => access,
        Err(error) => return authentication(error).into_response(),
    };
    let peer = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|peer| peer.0.ip());
    let client = client_ip(
        peer,
        request
            .headers()
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok()),
    );
    let raw_query = request.uri().query().unwrap_or_default().to_owned();
    let limits = state.limits.clone();
    let token = authorization.clone().unwrap_or_default();
    let quota_client = client.clone();
    let result = run_authenticated(
        &state.app,
        access,
        AuthenticatedRequest::new("GET", AUDIT_PATH, false),
        move |transaction| {
            Box::pin(async move {
                let actor = transaction.actor();
                if actor.role.as_deref() != Some("ADMIN") {
                    return Err(refusal(
                        StatusCode::FORBIDDEN,
                        "ADMIN_REQUIRED",
                        "Administrator access is required.",
                    ));
                }
                // The pinned inventory declares no endpoint quota for N03, so
                // this validates route ownership and charges nothing. A future
                // declared limit is applied here without touching the order.
                let user = actor.id.to_string();
                limits.check_business(
                    "GET",
                    AUDIT_PATH,
                    BusinessLimitKeys {
                        client: &quota_client,
                        user: Some(&user),
                        organization: None,
                        token: Some(&token),
                    },
                    now.timestamp().max(0) as u64,
                )?;
                let query = AuditQuery::from_query(&raw_query)?;
                load_page(transaction.connection(), &query).await
            })
        },
    )
    .await;
    match result {
        Ok(rows) => axum::Json(rows).into_response(),
        Err(AuthenticatedError::Authentication(error)) => authentication(error).into_response(),
        Err(AuthenticatedError::Database(error)) => error.into_response(),
        Err(AuthenticatedError::AmbiguousCommit) => ApiError::database().into_response(),
    }
}

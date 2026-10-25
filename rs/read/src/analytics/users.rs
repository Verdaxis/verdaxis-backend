//! P04: the identified administrator user listing.
//!
//! This is operator management, not product analytics. It names real accounts,
//! so it stays an administrator-only surface and is never reused as a payload
//! for an aggregate tab: no metric here is a product-analytics definition.
//!
//! Two source rules are load-bearing:
//!
//! * Administrator accounts are never listed (`role <> 'ADMIN'`, which in SQL
//!   also excludes a NULL role — the same rows the source query returns).
//! * A field the account does not have is `null` (organization, org type,
//!   provenance, last login) or `false` (has this organization ordered), never
//!   an invented zero or empty label. `role` keeps the source's empty string
//!   for a legacy account without a role.
//!
//! Runtime composition stays with the integration owner.

use std::sync::Arc;

use axum::{
    Router,
    extract::{ConnectInfo, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
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
    routes::ports::pydantic_datetime,
    runtime::http::{QueryFields, QueryKind, QueryValue, client_ip},
};

pub const USERS_PATH: &str = "/api/admin/analytics/users";

const LIMIT_DEFAULT: i64 = 50;
const LIMIT_MINIMUM: i64 = 1;
const LIMIT_MAXIMUM: i64 = 100;
const OFFSET_MINIMUM: i64 = 0;
const OFFSET_MAXIMUM: i64 = 10_000;
/// The route contract's search cap: longer input is refused, not truncated.
const SEARCH_MAX_CHARACTERS: usize = 200;

/// The closed account-status set. `ALL` (any case) and an absent value mean no
/// filter; anything else outside the set is a client error.
const STATUSES: [&str; 3] = ["PENDING", "APPROVED", "REJECTED"];

pub struct UsersAuthority {
    verification: Arc<VerificationSettings>,
    limits: SharedLimits,
}

impl UsersAuthority {
    pub fn new(verification: Arc<VerificationSettings>, limits: SharedLimits) -> Self {
        Self {
            verification,
            limits,
        }
    }
}

#[derive(Clone)]
struct RouteState {
    app: AppState,
    authority: Arc<UsersAuthority>,
}

/// Component router only; the serving binary does not mount this route.
pub fn router(
    app: AppState,
    verification: Arc<VerificationSettings>,
    limits: SharedLimits,
) -> Router {
    Router::new()
        .route(USERS_PATH, get(list))
        .with_state(RouteState {
            app,
            authority: Arc::new(UsersAuthority::new(verification, limits)),
        })
}

/// One validated page request.
pub struct UsersQuery {
    status: Option<String>,
    search: Option<String>,
    limit: i64,
    offset: i64,
}

impl UsersQuery {
    /// Validate the query in one pass so a request with two bad parameters
    /// reports both, the way the typed framework does.
    pub fn from_query(query: &str) -> Result<Self, ApiError> {
        let fields = QueryFields::parse(query);
        let mut errors: Vec<Value> = Vec::new();

        // The status arm records the refusal but does not return it yet: the
        // typed framework validates every parameter before the handler runs,
        // so a query with a bad status *and* a bad typed parameter must get
        // the typed 422 envelope, not the handler's 400.
        let mut invalid_status = false;
        let status = match fields.raw("status") {
            None => None,
            Some("") => None,
            Some(raw) if raw.eq_ignore_ascii_case("all") => None,
            Some(raw) => {
                let upper = raw.to_ascii_uppercase();
                if STATUSES.contains(&upper.as_str()) {
                    Some(upper)
                } else {
                    invalid_status = true;
                    None
                }
            }
        };

        let search = match fields.raw("search") {
            None => None,
            Some("") => None,
            Some(raw) if raw.chars().count() > SEARCH_MAX_CHARACTERS => {
                errors.push(json!({
                    "type": "string_too_long",
                    "loc": ["query", "search"],
                    "msg": format!(
                        "String should have at most {SEARCH_MAX_CHARACTERS} characters"
                    ),
                    "input": raw,
                    "ctx": {"max_length": SEARCH_MAX_CHARACTERS},
                }));
                None
            }
            Some(raw) => Some(raw.to_owned()),
        };

        let limit = integer(
            &fields,
            "limit",
            LIMIT_MINIMUM,
            LIMIT_MAXIMUM,
            LIMIT_DEFAULT,
            &mut errors,
        );
        let offset = integer(
            &fields,
            "offset",
            OFFSET_MINIMUM,
            OFFSET_MAXIMUM,
            0,
            &mut errors,
        );

        if !errors.is_empty() {
            return Err(ApiError {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                detail: Value::Array(errors),
                retry_after: None,
                kind: crate::ApiErrorKind::Expected,
            });
        }
        if invalid_status {
            // The source treats an unknown status as a client error, not as
            // an empty page; reached only when typed validation passed.
            return Err(refusal(
                StatusCode::BAD_REQUEST,
                "INVALID_STATUS_FILTER",
                "The status filter is not an account status.",
            ));
        }
        Ok(Self {
            status,
            search,
            limit,
            offset,
        })
    }
}

fn integer(
    fields: &QueryFields,
    name: &str,
    minimum: i64,
    maximum: i64,
    default: i64,
    errors: &mut Vec<Value>,
) -> i64 {
    match fields.validate(name, QueryKind::Integer { minimum, maximum }, false) {
        Ok(Some(QueryValue::Integer(value))) => value,
        Ok(_) => default,
        Err(error) => {
            errors.extend(error.detail.as_array().cloned().unwrap_or_default());
            default
        }
    }
}

/// One projected account. `status` and `role` are nullable text in the table so
/// the source can fall back to an empty string, exactly as it does.
#[derive(sqlx::FromRow)]
struct UserRow {
    id: Uuid,
    email: String,
    first_name: Option<String>,
    last_name: Option<String>,
    role: Option<String>,
    status: Option<String>,
    created_at: DateTime<Utc>,
    organization_id: Option<Uuid>,
    org_name: Option<String>,
    org_type: Option<String>,
    org_provenance: Option<String>,
    email_verified: bool,
    must_change_password: bool,
    last_login: Option<DateTime<Utc>>,
    org_has_orders: bool,
}

impl UserRow {
    /// `AdminUserEntry`, in the model's field order.
    fn response(&self) -> Value {
        json!({
            "id": self.id.to_string(),
            "email": self.email,
            "first_name": self.first_name,
            "last_name": self.last_name,
            "role": self.role.clone().unwrap_or_default(),
            "status": self.status.clone().unwrap_or_default(),
            "created_at": pydantic_datetime(self.created_at),
            "organization_id": self.organization_id.map(|id| id.to_string()),
            "org_name": self.org_name,
            "org_type": self.org_type,
            "org_provenance": self.org_provenance,
            "email_verified": self.email_verified,
            "must_change_password": self.must_change_password,
            "last_login": self.last_login.map(pydantic_datetime),
            "org_has_orders": self.org_has_orders,
        })
    }
}

/// The filter fragment both statements share, so the page and its total can
/// never disagree about which rows match.
const FILTERS: &str = "FROM users u \
     LEFT JOIN organizations o ON o.id = u.organization_id \
     WHERE u.role <> 'ADMIN' \
       AND ($1::text IS NULL OR u.status::text = $1) \
       AND ($2::text IS NULL OR u.email ILIKE $2 OR u.first_name ILIKE $2 OR u.last_name ILIKE $2)";

const PROJECTION: &str = "SELECT u.id, u.email, u.first_name, u.last_name, \
       u.role::text AS role, u.status::text AS status, u.created_at, u.organization_id, \
       o.name AS org_name, o.type::text AS org_type, o.provenance::text AS org_provenance, \
       u.email_verified, u.must_change_password, u.last_login, \
       EXISTS (SELECT 1 FROM orderbook_orders ob WHERE ob.organization_id = u.organization_id) \
         AS org_has_orders";

pub struct UsersPage {
    pub items: Vec<Value>,
    pub total: i64,
}

/// Read one page and its total on the caller's connection.
///
/// Ordering is `(created_at, id)` descending: the source orders by creation
/// time only, which leaves the offset window undefined for equal timestamps.
pub async fn read_page(
    connection: &mut PgConnection,
    query: &UsersQuery,
) -> Result<UsersPage, ApiError> {
    let pattern = query.search.as_ref().map(|term| format!("%{term}%"));
    let total_sql = format!("SELECT count(*) {FILTERS}");
    let total: i64 = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(total_sql))
        .bind(query.status.as_deref())
        .bind(pattern.as_deref())
        .fetch_one(&mut *connection)
        .await
        .map_err(|_| database_failure(None))?;

    let page_sql =
        format!("{PROJECTION} {FILTERS} ORDER BY u.created_at DESC, u.id DESC LIMIT $3 OFFSET $4");
    let rows: Vec<UserRow> = sqlx::query_as::<_, UserRow>(sqlx::AssertSqlSafe(page_sql))
        .bind(query.status.as_deref())
        .bind(pattern.as_deref())
        .bind(query.limit)
        .bind(query.offset)
        .fetch_all(&mut *connection)
        .await
        .map_err(|_| database_failure(None))?;
    Ok(UsersPage {
        items: rows.iter().map(UserRow::response).collect(),
        total,
    })
}

async fn list(State(state): State<RouteState>, request: Request) -> Response {
    let now = Utc::now();
    let authorization = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let access =
        match verify_authorization(authorization.as_deref(), &state.authority.verification, now) {
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
    let token = authorization.unwrap_or_default();
    let authority = state.authority.clone();
    let result = run_authenticated(
        &state.app,
        access,
        AuthenticatedRequest::new("GET", USERS_PATH, false),
        move |transaction| {
            Box::pin(async move {
                let actor = transaction.actor();
                // Admission order: authenticated and revalidated, administrator,
                // typed query, quota, then the identified page.
                if actor.role.as_deref() != Some("ADMIN") {
                    return Err(refusal(
                        StatusCode::FORBIDDEN,
                        "ADMIN_REQUIRED",
                        "Administrator access is required.",
                    ));
                }
                let query = UsersQuery::from_query(&raw_query)?;
                let user = actor.id.to_string();
                authority.limits.check_business(
                    "GET",
                    USERS_PATH,
                    BusinessLimitKeys {
                        client: &client,
                        user: Some(&user),
                        organization: None,
                        token: Some(&token),
                    },
                    now.timestamp().max(0) as u64,
                )?;
                let page = read_page(transaction.connection(), &query).await?;
                Ok(json!({"items": page.items, "total": page.total}))
            })
        },
    )
    .await;
    match result {
        Ok(body) => axum::Json(body).into_response(),
        Err(AuthenticatedError::Authentication(error)) => authentication(error).into_response(),
        Err(AuthenticatedError::Database(error)) => error.into_response(),
        Err(AuthenticatedError::AmbiguousCommit) => ApiError::database().into_response(),
    }
}

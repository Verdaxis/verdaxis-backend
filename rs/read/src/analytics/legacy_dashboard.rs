//! P02 and P03: the legacy administrator dashboard.
//!
//! These two reads answer "how much business has the platform done" from the
//! operational tables. They are deliberately **not** the product-analytics
//! tabs: there is no behavioral provider, no coverage or suppression model and
//! no identified person in the payload. Their definition of a market
//! participant is the source rule — a user row whose role is `BUYER` or
//! `SUPPLIER` — not stored order or trade provenance, so the numbers keep
//! meaning what the administrator already reads today.
//!
//! Every bucket is a UTC calendar day and the dense series always has one
//! entry per requested day, with a genuine zero for a day that had no activity.
//! Nothing here is unavailable, so nothing is `null`.
//!
//! Runtime composition stays with the integration owner; this module mounts no
//! route in the serving binary.

use std::sync::Arc;

use axum::{
    Router,
    extract::{ConnectInfo, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde_json::{Value, json};
use sqlx::PgConnection;

use crate::{
    ApiError, AppState,
    auth::{
        AuthenticatedError, AuthenticatedRequest, VerificationSettings,
        limits::{BusinessLimitKeys, SharedLimits},
        run_authenticated, verify_authorization,
    },
    business::error::{authentication, database_failure, refusal},
    runtime::http::{QueryFields, QueryKind, QueryValue, client_ip},
};

pub const OVERVIEW_PATH: &str = "/api/admin/analytics/overview";
pub const DAILY_PATH: &str = "/api/admin/analytics/daily";

/// The dashboard's market-participant rule: an account that can trade.
const MARKET_ROLES: &str = "('BUYER','SUPPLIER')";

/// Statuses that count as an economically executed trade, in the source order.
const CONFIRMED_TRADE_STATUSES: &str = "('CONFIRMED','DELIVERED','PAID')";

/// Statuses that are still live, subject to the expiry check.
const OPEN_ORDER_STATUSES: &str = "('OPEN','PARTIALLY_FILLED')";

const ACTIVE_WINDOW: Duration = Duration::days(7);
const DAYS_DEFAULT: i64 = 30;
const DAYS_MINIMUM: i64 = 1;
const DAYS_MAXIMUM: i64 = 365;

pub struct AnalyticsAuthority {
    verification: Arc<VerificationSettings>,
    limits: SharedLimits,
}

impl AnalyticsAuthority {
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
    authority: Arc<AnalyticsAuthority>,
}

/// Component router only; the serving binary does not mount these routes.
pub fn router(
    app: AppState,
    verification: Arc<VerificationSettings>,
    limits: SharedLimits,
) -> Router {
    Router::new()
        .route(OVERVIEW_PATH, get(overview))
        .route(DAILY_PATH, get(daily))
        .with_state(RouteState {
            app,
            authority: Arc::new(AnalyticsAuthority::new(verification, limits)),
        })
}

// ---------------------------------------------------------------------------
// P02 — platform-wide counters.
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct OverviewRow {
    total_users: i64,
    active_users_7d: i64,
    total_organizations: i64,
    total_orders: i64,
    open_orders: i64,
    total_trades: i64,
    confirmed_trades: i64,
    total_volume_mt: f64,
    total_revenue_usd: f64,
    total_gmv_usd: f64,
}

impl OverviewRow {
    /// `OverviewResponse`, in the model's field order, as client-read numbers.
    fn response(&self) -> Value {
        json!({
            "total_users": self.total_users,
            "active_users_7d": self.active_users_7d,
            "total_organizations": self.total_organizations,
            "total_orders": self.total_orders,
            "open_orders": self.open_orders,
            "total_trades": self.total_trades,
            "confirmed_trades": self.confirmed_trades,
            "total_volume_mt": self.total_volume_mt,
            "total_revenue_usd": self.total_revenue_usd,
            "total_gmv_usd": self.total_gmv_usd,
        })
    }
}

/// One statement, one snapshot: every counter sees the same market population.
fn overview_sql() -> String {
    format!(
        "WITH market AS (\
            SELECT DISTINCT organization_id AS id FROM users \
             WHERE organization_id IS NOT NULL AND role IN {roles}\
         ) \
         SELECT \
           (SELECT count(*) FROM users WHERE role IN {roles}) AS total_users, \
           (SELECT count(*) FROM users WHERE role IN {roles} AND last_login >= $1) AS active_users_7d, \
           (SELECT count(*) FROM market) AS total_organizations, \
           (SELECT count(*) FROM orderbook_orders WHERE organization_id IN (SELECT id FROM market)) AS total_orders, \
           (SELECT count(*) FROM orderbook_orders WHERE organization_id IN (SELECT id FROM market) \
              AND status IN {open} AND (expires_at IS NULL OR expires_at > $2)) AS open_orders, \
           (SELECT count(*) FROM trades WHERE buyer_id IN (SELECT id FROM market) \
              AND seller_id IN (SELECT id FROM market)) AS total_trades, \
           (SELECT count(*) FROM trades WHERE buyer_id IN (SELECT id FROM market) \
              AND seller_id IN (SELECT id FROM market) \
              AND status IN {confirmed}) AS confirmed_trades, \
           (SELECT COALESCE(SUM(quantity_mt),0)::double precision FROM trades \
              WHERE buyer_id IN (SELECT id FROM market) AND seller_id IN (SELECT id FROM market) \
              AND status IN {confirmed}) AS total_volume_mt, \
           (SELECT COALESCE(SUM(commission_amount_usd),0)::double precision FROM trades \
              WHERE buyer_id IN (SELECT id FROM market) AND seller_id IN (SELECT id FROM market) \
              AND status = 'PAID') AS total_revenue_usd, \
           (SELECT COALESCE(SUM(final_total_usd),0)::double precision FROM trades \
              WHERE buyer_id IN (SELECT id FROM market) AND seller_id IN (SELECT id FROM market) \
              AND status = 'PAID') AS total_gmv_usd",
        roles = MARKET_ROLES,
        open = OPEN_ORDER_STATUSES,
        confirmed = CONFIRMED_TRADE_STATUSES,
    )
}

pub async fn read_overview(
    connection: &mut PgConnection,
    now: DateTime<Utc>,
) -> Result<Value, ApiError> {
    let row: OverviewRow = sqlx::query_as::<_, OverviewRow>(sqlx::AssertSqlSafe(overview_sql()))
        .bind(now - ACTIVE_WINDOW)
        .bind(now)
        .fetch_one(connection)
        .await
        .map_err(|_| database_failure(None))?;
    Ok(row.response())
}

// ---------------------------------------------------------------------------
// P03 — the dense UTC daily series.
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct OrderDay {
    day: NaiveDate,
    count: i64,
}

#[derive(sqlx::FromRow)]
struct TradeDay {
    day: NaiveDate,
    count: i64,
    volume_mt: f64,
    gmv_usd: f64,
    commission_usd: f64,
}

/// One point of `DailyStat`; missing days are genuine zeros.
#[derive(Clone, Default)]
struct DayTotals {
    orders_placed: i64,
    trades_executed: i64,
    volume_mt: f64,
    gmv_usd: f64,
    commission_usd: f64,
}

pub struct DailyQuery {
    days: i64,
}

impl DailyQuery {
    /// FastAPI's `Query(30, ge=1, le=365)`, including its refusal shape.
    pub fn from_query(query: &str) -> Result<Self, ApiError> {
        let fields = QueryFields::parse(query);
        let days = match fields.validate(
            "days",
            QueryKind::Integer {
                minimum: DAYS_MINIMUM,
                maximum: DAYS_MAXIMUM,
            },
            false,
        )? {
            Some(QueryValue::Integer(value)) => value,
            _ => DAYS_DEFAULT,
        };
        Ok(Self { days })
    }
}

/// The series always starts at midnight UTC of the first requested day, so a
/// bucket boundary never depends on the host's local time.
pub async fn read_daily(
    connection: &mut PgConnection,
    query: &DailyQuery,
    now: DateTime<Utc>,
) -> Result<Value, ApiError> {
    let today = now.date_naive();
    let start = today - Duration::days(query.days - 1);
    let start_at = start
        .and_hms_opt(0, 0, 0)
        .expect("midnight is a valid time")
        .and_utc();
    let end_at = start_at + Duration::days(query.days);

    let orders_sql = format!(
        "SELECT (created_at AT TIME ZONE 'UTC')::date AS day, count(*) AS count \
         FROM orderbook_orders \
         WHERE created_at >= $1 AND created_at < $2 \
           AND organization_id IN (SELECT DISTINCT organization_id FROM users \
                                    WHERE organization_id IS NOT NULL AND role IN {roles}) \
         GROUP BY 1",
        roles = MARKET_ROLES,
    );
    let orders: Vec<OrderDay> = sqlx::query_as::<_, OrderDay>(sqlx::AssertSqlSafe(orders_sql))
        .bind(start_at)
        .bind(end_at)
        .fetch_all(&mut *connection)
        .await
        .map_err(|_| database_failure(None))?;

    let trades_sql = format!(
        "SELECT (created_at AT TIME ZONE 'UTC')::date AS day, count(*) AS count, \
                COALESCE(SUM(quantity_mt),0)::double precision AS volume_mt, \
                COALESCE(SUM(final_total_usd),0)::double precision AS gmv_usd, \
                COALESCE(SUM(commission_amount_usd),0)::double precision AS commission_usd \
         FROM trades \
         WHERE created_at >= $1 AND created_at < $2 \
           AND status IN {confirmed} \
           AND buyer_id IN (SELECT DISTINCT organization_id FROM users \
                             WHERE organization_id IS NOT NULL AND role IN {roles}) \
           AND seller_id IN (SELECT DISTINCT organization_id FROM users \
                              WHERE organization_id IS NOT NULL AND role IN {roles}) \
         GROUP BY 1",
        confirmed = CONFIRMED_TRADE_STATUSES,
        roles = MARKET_ROLES,
    );
    let trades: Vec<TradeDay> = sqlx::query_as::<_, TradeDay>(sqlx::AssertSqlSafe(trades_sql))
        .bind(start_at)
        .bind(end_at)
        .fetch_all(&mut *connection)
        .await
        .map_err(|_| database_failure(None))?;

    let mut by_day: std::collections::BTreeMap<NaiveDate, DayTotals> =
        std::collections::BTreeMap::new();
    for row in orders {
        by_day.entry(row.day).or_default().orders_placed = row.count;
    }
    for row in trades {
        let totals = by_day.entry(row.day).or_default();
        totals.trades_executed = row.count;
        totals.volume_mt = row.volume_mt;
        totals.gmv_usd = row.gmv_usd;
        totals.commission_usd = row.commission_usd;
    }

    let series: Vec<Value> = (0..query.days)
        .map(|offset| {
            let day = start + Duration::days(offset);
            let totals = by_day.get(&day).cloned().unwrap_or_default();
            json!({
                "date": day.format("%Y-%m-%d").to_string(),
                "orders_placed": totals.orders_placed,
                "trades_executed": totals.trades_executed,
                "volume_mt": totals.volume_mt,
                "gmv_usd": totals.gmv_usd,
                "commission_usd": totals.commission_usd,
            })
        })
        .collect();
    Ok(Value::Array(series))
}

// ---------------------------------------------------------------------------
// Adapters.
// ---------------------------------------------------------------------------

async fn overview(State(state): State<RouteState>, request: Request) -> Response {
    dispatch(state, request, OVERVIEW_PATH, None).await
}

async fn daily(State(state): State<RouteState>, request: Request) -> Response {
    let raw_query = request.uri().query().unwrap_or_default().to_owned();
    dispatch(state, request, DAILY_PATH, Some(raw_query)).await
}

/// Admission order for every read here: verify the bearer, revalidate the
/// current account under its row lock, require ADMIN, validate the typed query,
/// consume the route quota, and only then read.
///
/// The query is validated before the quota on purpose: a refused request never
/// spends the caller's minute.
async fn dispatch(
    state: RouteState,
    request: Request,
    template: &'static str,
    raw_query: Option<String>,
) -> Response {
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
    let token = format!("usr:{}", access.subject);
    let authority = state.authority.clone();
    let result = run_authenticated(
        &state.app,
        access,
        AuthenticatedRequest::new("GET", template, false),
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
                // Typed validation, then quota, then the read.
                let query = raw_query
                    .as_deref()
                    .map(DailyQuery::from_query)
                    .transpose()?;
                let user = actor.id.to_string();
                authority.limits.check_business(
                    "GET",
                    template,
                    BusinessLimitKeys {
                        client: &client,
                        user: Some(&user),
                        organization: None,
                        token: Some(&token),
                    },
                    now.timestamp().max(0) as u64,
                )?;
                match query {
                    Some(query) => read_daily(transaction.connection(), &query, now).await,
                    None => read_overview(transaction.connection(), now).await,
                }
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

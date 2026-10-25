//! P01 `GET /api/admin/analytics/product-usage`.
//!
//! The authoritative counts are Rust-owned database facts. The behavioral
//! aggregate is an optional provider component: when the provider is
//! unavailable the authoritative counts and the 200 response remain, and every
//! unavailable behavioral value keeps Python's required default rather than a
//! made-up zero or an empty list.
//!
//! Runtime composition (mounting this router) remains with the integration
//! owner. This module mounts nothing by itself.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Router};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

use super::policy::RegisteredEvent;
use super::types::{AggregateCell, AnalyticsDiagnostic, BehavioralAvailability, BehavioralWindow};
use crate::auth::limits::{BusinessLimitKeys, SharedLimits};
use crate::auth::{
    AuthenticatedError, AuthenticatedRequest, CurrentActor, VerificationSettings,
    run_authenticated, verify_authorization,
};
use crate::business::error::{authentication, database_failure, refusal};
use crate::{ApiError, AppState};

pub const PATH: &str = "/api/admin/analytics/product-usage";

/// Canonical synthetic market organizations. Their orders never count as
/// market activity, exactly as the Python route excludes them.
pub const DEMO_MARKET_ORGANIZATION_IDS: [Uuid; 14] = [
    Uuid::from_u128(0x4da7b285_34ee_5443_9406_f96b4ed1a251),
    Uuid::from_u128(0x0dbce576_2026_5925_ab66_674d505e98ad),
    Uuid::from_u128(0x3cd0bc8a_0e92_5eb8_9ae3_3c87a7460a6a),
    Uuid::from_u128(0x277491df_cb0d_5f2d_a2cf_5746829c6da6),
    Uuid::from_u128(0x3b302066_d65c_5c3e_8fcc_70b3da3bcafd),
    Uuid::from_u128(0x79609f48_0a3e_560e_a1e1_63d90601d84a),
    Uuid::from_u128(0x2c4e387e_de22_5adb_ad88_9274ba84ebe1),
    Uuid::from_u128(0x612953c7_567a_58b3_bc42_ee817d2bbe74),
    Uuid::from_u128(0x93ccda09_54b3_53ee_afc0_759d3048161f),
    Uuid::from_u128(0x82426590_0963_5486_9b05_f81e97afe6ef),
    Uuid::from_u128(0xacc3f20a_fe94_4463_9029_a55e35634eb7),
    Uuid::from_u128(0xc9c1ccbf_66fe_4a1b_b171_fe4f7ddc31a4),
    Uuid::from_u128(0x7cc77115_0a9f_4ec4_8c74_05aa10050111),
    Uuid::from_u128(0xd1e43e55_3fb0_4b5e_9f0b_93aa10050222),
];

/// Provider collections keep the legacy P01 bounds.
const DAILY_VISITOR_MAX: usize = 92;
const EVENT_SERIES_MAX: usize = 2_500;
const RANK_MAX: usize = 20;

/// A registration, login or ordering organization is counted only in this
/// half-open UTC window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageWindow {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl UsageWindow {
    pub fn days(days: UsageDays, end: DateTime<Utc>) -> Self {
        Self {
            start: end - Duration::days(i64::from(days.value())),
            end,
        }
    }
}

/// The behavioral aggregate is optional; an unavailable provider never fails
/// the authoritative tab.
pub type BehavioralFuture<'a> = Pin<Box<dyn Future<Output = BehavioralWindow> + Send + 'a>>;

pub trait BehavioralUsageSource: Send + Sync {
    fn window(&self, window: UsageWindow) -> BehavioralFuture<'_>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageDays {
    Seven,
    Thirty,
    Ninety,
}

impl UsageDays {
    /// Only the three declared periods are accepted; the default is thirty.
    pub fn parse(raw: Option<&str>) -> Result<Self, ApiError> {
        match raw {
            None | Some("30") => Ok(Self::Thirty),
            Some("7") => Ok(Self::Seven),
            Some("90") => Ok(Self::Ninety),
            Some(_) => Err(refusal(
                StatusCode::UNPROCESSABLE_ENTITY,
                "ANALYTICS_QUERY_INVALID",
                "The period must be 7, 30 or 90 days.",
            )),
        }
    }

    pub fn value(self) -> u8 {
        match self {
            Self::Seven => 7,
            Self::Thirty => 30,
            Self::Ninety => 90,
        }
    }
}

fn registered_event(event: &str) -> bool {
    RegisteredEvent::from_name(event).is_some()
}

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct AuthoritativeUsage {
    pub registrations: u64,
    pub users_logging_in: u64,
    pub order_placing_organizations: u64,
}

/// Half-open UTC counts of real market users and organizations. Demo orders
/// never count as market activity.
pub async fn authoritative_counts(
    connection: &mut PgConnection,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<AuthoritativeUsage, ApiError> {
    #[derive(sqlx::FromRow)]
    struct Counts {
        registrations: i64,
        users_logging_in: i64,
        order_placing_organizations: i64,
    }
    let counts = sqlx::query_as::<_, Counts>(
        "SELECT \
         (SELECT count(*) FROM users WHERE role IN ('BUYER','SUPPLIER') \
            AND created_at >= $1 AND created_at < $2)::bigint AS registrations, \
         (SELECT count(*) FROM users WHERE role IN ('BUYER','SUPPLIER') \
            AND last_login >= $1 AND last_login < $2)::bigint AS users_logging_in, \
         (SELECT count(DISTINCT organization_id) FROM orderbook_orders \
            WHERE created_at >= $1 AND created_at < $2 \
            AND organization_id IN (SELECT organization_id FROM users \
                WHERE organization_id IS NOT NULL AND role IN ('BUYER','SUPPLIER')) \
            AND organization_id <> ALL($3::uuid[]))::bigint AS order_placing_organizations",
    )
    .bind(start)
    .bind(end)
    .bind(DEMO_MARKET_ORGANIZATION_IDS.to_vec())
    .fetch_one(connection)
    .await
    .map_err(|_| database_failure(None))?;
    let bounded = |value: i64| u64::try_from(value).unwrap_or(0);
    Ok(AuthoritativeUsage {
        registrations: bounded(counts.registrations),
        users_logging_in: bounded(counts.users_logging_in),
        order_placing_organizations: bounded(counts.order_placing_organizations),
    })
}

#[derive(Debug, Serialize)]
pub struct ProductUsageResponse {
    pub days: u8,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub behavioral_status: BehavioralAvailability,
    pub diagnostic: Option<AnalyticsDiagnostic>,
    pub observed_at: DateTime<Utc>,
    pub behavioral: BehavioralUsage,
    pub authoritative: AuthoritativeUsage,
    pub funnel: Vec<FunnelStage>,
}

#[derive(Debug, Serialize)]
pub struct BehavioralUsage {
    pub visitors: Option<u64>,
    pub visits: Option<u64>,
    pub pageviews: Option<u64>,
    pub total_time_seconds: Option<u64>,
    pub average_session_duration_seconds: Option<f64>,
    pub event_totals: Option<BTreeMap<String, Option<u64>>>,
    pub event_series: Option<Vec<DailyEventPoint>>,
    pub daily_visitors: Option<Vec<DailyVisitorPoint>>,
    pub top_entries: Option<Vec<MetricEntry>>,
    pub top_referrers: Option<Vec<MetricEntry>>,
}

#[derive(Debug, Serialize)]
pub struct DailyVisitorPoint {
    pub date: chrono::NaiveDate,
    pub value: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct DailyEventPoint {
    pub date: chrono::NaiveDate,
    pub event: String,
    pub value: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct MetricEntry {
    pub name: String,
    pub value: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct FunnelStage {
    pub name: &'static str,
    pub value: Option<u64>,
    pub conversion_from_previous_pct: Option<f64>,
}

fn one_decimal(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn conversion(value: Option<u64>, previous: Option<u64>) -> Option<f64> {
    match (value, previous) {
        (Some(value), Some(previous)) if previous > 0 => {
            Some(one_decimal(value as f64 / previous as f64 * 100.0))
        }
        _ => None,
    }
}

fn visible(cell: &AggregateCell) -> Option<u64> {
    if cell.suppressed { None } else { cell.count }
}

impl BehavioralUsage {
    fn from_window(window: &BehavioralWindow) -> Self {
        let core = window.core.data();
        let visitors = super::policy::suppress(core.and_then(|core| core.visitors), None);
        let visits = super::policy::suppress(core.and_then(|core| core.visits), None);
        let pageviews = super::policy::suppress(core.and_then(|core| core.pageviews), None);
        // Session time belongs to the visit cohort. A hidden denominator must
        // not remain recoverable from its total or average duration.
        let total_time = if visits.suppressed {
            None
        } else {
            core.and_then(|core| core.total_time_seconds)
        };
        let visits = visible(&visits);
        Self {
            visitors: visible(&visitors),
            visits,
            pageviews: visible(&pageviews),
            total_time_seconds: total_time,
            average_session_duration_seconds: match (total_time, visits) {
                (Some(total), Some(visits)) if visits > 0 => {
                    Some(one_decimal(total as f64 / visits as f64))
                }
                _ => None,
            },
            event_totals: window.event_totals.data().map(|rows| {
                rows.iter()
                    .filter(|row| registered_event(&row.event))
                    .map(|row| (row.event.clone(), visible(&row.count)))
                    .collect()
            }),
            daily_visitors: window.daily_visitors.data().map(|rows| {
                rows.iter()
                    .take(DAILY_VISITOR_MAX)
                    .map(|row| DailyVisitorPoint {
                        date: row.date,
                        value: visible(&row.visitors),
                    })
                    .collect()
            }),
            event_series: window.event_series.data().map(|rows| {
                let rows: Vec<_> = rows
                    .iter()
                    .filter(|row| registered_event(&row.event))
                    .collect();
                rows.iter()
                    .skip(rows.len().saturating_sub(EVENT_SERIES_MAX))
                    .map(|row| DailyEventPoint {
                        date: row.date,
                        event: row.event.clone(),
                        value: visible(&row.count),
                    })
                    .collect()
            }),
            top_entries: window.top_entries.data().map(|rows| {
                rows.iter()
                    .take(RANK_MAX)
                    .map(|row| MetricEntry {
                        name: serde_json::to_value(row.entry)
                            .expect("closed entry enum")
                            .as_str()
                            .expect("entry label")
                            .to_owned(),
                        value: visible(&row.count),
                    })
                    .collect()
            }),
            top_referrers: window.top_referrers.data().map(|rows| {
                rows.iter()
                    .take(RANK_MAX)
                    .map(|row| MetricEntry {
                        name: serde_json::to_value(row.referrer)
                            .expect("closed referrer enum")
                            .as_str()
                            .expect("referrer label")
                            .to_owned(),
                        value: visible(&row.count),
                    })
                    .collect()
            }),
        }
    }
}

/// Compose the response family the client reads. No raw provider value reaches
/// the response and no identifier or commercial field is derived.
pub fn compose(
    days: UsageDays,
    period_start: DateTime<Utc>,
    period_end: DateTime<Utc>,
    authoritative: AuthoritativeUsage,
    window: &BehavioralWindow,
) -> ProductUsageResponse {
    let behavioral = BehavioralUsage::from_window(window);
    let stages = [
        ("visitors", behavioral.visitors),
        (
            "signup_started",
            behavioral
                .event_totals
                .as_ref()
                .and_then(|events| events.get("signup_started").copied().flatten()),
        ),
        ("registrations", Some(authoritative.registrations)),
        ("users_logging_in", Some(authoritative.users_logging_in)),
        (
            "order_placing_organizations",
            Some(authoritative.order_placing_organizations),
        ),
    ];
    let funnel = stages
        .iter()
        .enumerate()
        .map(|(index, (name, value))| FunnelStage {
            name,
            value: *value,
            conversion_from_previous_pct: if index == 0 {
                None
            } else {
                conversion(*value, stages[index - 1].1)
            },
        })
        .collect();
    ProductUsageResponse {
        days: days.value(),
        period_start,
        period_end,
        behavioral_status: window.availability,
        diagnostic: window.diagnostic,
        observed_at: window.observed_at,
        behavioral,
        authoritative,
        funnel,
    }
}

/// The bucket identity of the presented bearer token.
///
/// The header is normalized exactly as `verify_authorization` normalizes it —
/// case-insensitive scheme, then every leading space removed — so re-spelling
/// one token cannot open a second quota bucket. The key is the full 32-byte
/// SHA-256 digest of the normalized token, never a truncated prefix.
pub fn token_identity(authorization: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    let raw = authorization.unwrap_or("");
    let (scheme, token) = raw.split_once(' ').unwrap_or((raw, ""));
    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
        // Authentication refuses this header before any quota is charged; one
        // shared fallback bucket keeps the function total without inventing an
        // identity for an unauthenticated caller.
        return "client".into();
    }
    let digest = Sha256::digest(token.trim_start_matches(' ').as_bytes());
    format!("tok:{}", hex(&digest))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Administrator authority, refused before any quota is spent.
pub fn require_admin(actor: &CurrentActor) -> Result<(), ApiError> {
    if actor.role.as_deref() != Some("ADMIN") {
        return Err(refusal(
            StatusCode::FORBIDDEN,
            "ADMIN_REQUIRED",
            "Administrator access is required.",
        ));
    }
    Ok(())
}

/// The declared 30/minute token quota. It is charged after authority and
/// input validation, so a refused request spends none of the bucket.
pub fn check_quota(
    limits: &SharedLimits,
    actor: &CurrentActor,
    authorization: Option<&str>,
    now: u64,
) -> Result<(), ApiError> {
    let identity = token_identity(authorization);
    let user = actor.id.to_string();
    limits.check_business(
        "GET",
        PATH,
        BusinessLimitKeys {
            client: &identity,
            user: Some(&user),
            organization: None,
            token: Some(&identity),
        },
        now,
    )
}

pub struct UsageAuthority {
    verification: VerificationSettings,
    limits: SharedLimits,
    source: Option<Arc<dyn BehavioralUsageSource>>,
}

impl UsageAuthority {
    pub fn new(
        verification: VerificationSettings,
        limits: SharedLimits,
        source: Option<Arc<dyn BehavioralUsageSource>>,
    ) -> Self {
        Self {
            verification,
            limits,
            source,
        }
    }
}

pub fn routes(authority: UsageAuthority) -> Router<AppState> {
    Router::new()
        .route(PATH, get(product_usage))
        .layer(Extension(Arc::new(authority)))
}

async fn product_usage(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<UsageAuthority>>,
    request: Request,
) -> Result<Response, ApiError> {
    let now = Utc::now();
    let authorization = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let access = verify_authorization(authorization.as_deref(), &authority.verification, now)
        .map_err(authentication)?;
    let query = request.uri().query().unwrap_or("").to_owned();
    let authenticated = AuthenticatedRequest::new("GET", PATH, false);
    let quota_identity = authorization.clone();
    let limits = authority.limits.clone();
    // The database facts and every refusal resolve inside the authenticated
    // transaction. The optional provider call happens after it, so no
    // connection, row lock or admission permit is held across provider I/O.
    let (days, window, authoritative) =
        run_authenticated(&state, access, authenticated, move |transaction| {
            Box::pin(async move {
                require_admin(transaction.actor())?;
                let days = UsageDays::parse(days_parameter(&query).as_deref())?;
                check_quota(
                    &limits,
                    transaction.actor(),
                    quota_identity.as_deref(),
                    now.timestamp().max(0) as u64,
                )?;
                let window = UsageWindow::days(days, now);
                let authoritative =
                    authoritative_counts(transaction.connection(), window.start, window.end)
                        .await?;
                Ok((days, window, authoritative))
            })
        })
        .await
        .map_err(|error| match error {
            AuthenticatedError::Authentication(error) => authentication(error),
            AuthenticatedError::Database(error) => error,
            AuthenticatedError::AmbiguousCommit => refusal(
                StatusCode::SERVICE_UNAVAILABLE,
                "COMMIT_OUTCOME_UNKNOWN",
                "The request outcome is unknown.",
            ),
        })?;
    let observed = match authority.source.clone() {
        Some(source) => source.window(window).await,
        // No provider is configured: the tab stays authoritative.
        None => BehavioralWindow::unavailable(
            window.start,
            window.end,
            Utc::now(),
            AnalyticsDiagnostic::Disabled,
        ),
    };
    let response = compose(days, window.start, window.end, authoritative, &observed);
    Ok(axum::Json(response).into_response())
}

/// The single accepted query key. Duplicate `days` values are refused rather
/// than silently taking one; unknown parameters are ignored.
fn days_parameter(query: &str) -> Option<String> {
    let mut found = None;
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key != "days" {
            continue;
        }
        let value = percent_decode(value);
        if found.is_some() {
            return Some(String::new());
        }
        found = Some(value);
    }
    found
}

fn percent_decode(value: &str) -> String {
    percent_encoding::percent_decode_str(value)
        .decode_utf8_lossy()
        .into_owned()
}

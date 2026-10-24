//! Grouped, bounded SQL fact loading for the authoritative analytics tabs.
//!
//! Every loader issues a fixed number of statements; none loops over users,
//! organizations, days or provider rows. Membership and provenance predicates
//! are the Python service's own rules (`app/services/product_analytics.py`):
//! demo organizations are always excluded from market membership, and order
//! and trade provenance is the stored provenance plus the demo identity set,
//! never inferred from organization names.
//!
//! Unavailable or uncovered values stay `None`; a genuine zero stays `0`.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, NaiveDate, TimeDelta, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::types::{
    AggregateCell, AnalyticsDataQuality, AnalyticsDiagnostic, AnalyticsQuery,
    AnalyticsSourceCoverage, AnalyticsSourceStatus, BehavioralWindow, MetricValue,
};

/// Synthetic market organizations (`app/demo_identities.py`). Stored provenance
/// alone cannot classify these historical rows, so the exact identity set is
/// part of the analytics predicate.
pub const DEMO_MARKET_ORG_IDS: [Uuid; 14] = [
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

/// Three is the smallest population that can be published; 1 and 2 suppress.
/// The decision itself lives in `types::suppress`.
pub const MAX_COHORTS: usize = 54;

fn demo_ids() -> Vec<Uuid> {
    DEMO_MARKET_ORG_IDS.to_vec()
}

fn count(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Suppression and shaping
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct QualityTracker {
    pub legacy_timestamp_fallback_count: u64,
    pub missing_paid_at_count: u64,
    pub missing_commission_payment_date_count: u64,
    pub suppressed_cell_count: u64,
    pub cohort_complete: bool,
}

impl Default for QualityTracker {
    fn default() -> Self {
        Self {
            legacy_timestamp_fallback_count: 0,
            missing_paid_at_count: 0,
            missing_commission_payment_date_count: 0,
            suppressed_cell_count: 0,
            cohort_complete: true,
        }
    }
}

impl QualityTracker {
    /// Positive basis 1 or 2 suppresses; zero stays visible. A missing count is
    /// unavailable, not suppressed. The decision itself is the shared CF4_03
    /// `suppress` scalar; this wrapper only keeps the quality counters.
    pub fn cell(&mut self, key: &str, count: Option<u64>, entities: Option<u64>) -> AggregateCell {
        let mut cell = super::types::suppress(count, entities);
        cell.key = key.to_owned();
        if cell.suppressed {
            self.suppressed_cell_count += 1;
        }
        cell
    }

    /// An entity-basis metric with no comparison period.
    pub fn segmented_metric(&mut self, value: Option<u64>) -> MetricValue {
        let cell = self.cell("", value, None);
        MetricValue {
            value: cell.count,
            previous: None,
            suppressed: cell.suppressed,
        }
    }

    pub fn to_schema(&self) -> AnalyticsDataQuality {
        AnalyticsDataQuality {
            legacy_timestamp_fallback_count: self.legacy_timestamp_fallback_count,
            missing_paid_at_count: self.missing_paid_at_count,
            missing_commission_payment_date_count: self.missing_commission_payment_date_count,
            suppressed_cell_count: self.suppressed_cell_count,
            cohort_complete: self.cohort_complete,
        }
    }
}

/// Rate percentage rounded half away from zero; `None` for a non-positive
/// denominator, which is never rendered as `0`.
pub fn rate_pct(numerator: u64, denominator: u64) -> Option<Decimal> {
    if denominator == 0 {
        return None;
    }
    Some(
        (Decimal::from(numerator) / Decimal::from(denominator) * Decimal::ONE_HUNDRED)
            .round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero),
    )
}

/// Median duration in hours, one decimal place (banker's rounding as Python).
pub fn median_hours(durations: &[TimeDelta]) -> Option<Decimal> {
    if durations.is_empty() {
        return None;
    }
    let mut seconds: Vec<i64> = durations
        .iter()
        .filter_map(TimeDelta::num_microseconds)
        .collect();
    seconds.sort_unstable();
    let middle = seconds.len() / 2;
    let micros = if seconds.len() % 2 == 0 {
        Decimal::from(seconds[middle - 1] + seconds[middle]) / Decimal::TWO
    } else {
        Decimal::from(seconds[middle])
    };
    Some(
        (micros / Decimal::from(1_000_000) / Decimal::from(3600))
            .round_dp_with_strategy(1, RoundingStrategy::MidpointNearestEven),
    )
}

/// Dense UTC daily series for an instant window: every calendar date touched
/// by `[start, end)`, capped at 366 points. Zero is a genuine zero.
pub fn dense_series(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    values: &BTreeMap<NaiveDate, u64>,
) -> Vec<super::types::SeriesPoint> {
    let mut points = Vec::new();
    let mut cursor = start.date_naive();
    while (cursor < end.date_naive()
        || (cursor == end.date_naive() && end.time() != chrono::NaiveTime::MIN))
        && points.len() < 366
    {
        points.push(super::types::SeriesPoint {
            date: cursor,
            value: Some(values.get(&cursor).copied().unwrap_or(0)),
        });
        cursor += TimeDelta::days(1);
    }
    points
}

/// Half-open date bounds for date-only facts projected to 00:00:00Z. A midday
/// start excludes that date; a midday end includes it (Python `_date_window`).
pub fn date_window(
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> Option<(NaiveDate, NaiveDate)> {
    let (start, end) = (start?, end?);
    let mut lower = start.date_naive();
    if start != midnight(lower) {
        lower += TimeDelta::days(1);
    }
    let mut upper = end.date_naive();
    if end != midnight(upper) {
        upper += TimeDelta::days(1);
    }
    Some((lower, upper))
}

pub fn midnight(date: NaiveDate) -> DateTime<Utc> {
    date.and_hms_opt(0, 0, 0)
        .expect("midnight is a valid time")
        .and_utc()
}

/// A fact source fully covers the request only when it starts at or before the
/// requested period, and at or before the comparison period when comparing.
pub fn fact_coverage(
    coverage_start: Option<DateTime<Utc>>,
    query: &AnalyticsQuery,
    observed_at: DateTime<Utc>,
) -> AnalyticsSourceCoverage {
    let Some(coverage_start) = coverage_start else {
        return AnalyticsSourceCoverage {
            coverage_start: None,
            coverage_end: None,
            observed_at: None,
            status: AnalyticsSourceStatus::Unavailable,
            diagnostic: Some(AnalyticsDiagnostic::InsufficientCoverage),
        };
    };
    let fully_covered = coverage_start <= query.start
        && query
            .previous_start()
            .is_none_or(|previous_start| coverage_start <= previous_start);
    AnalyticsSourceCoverage {
        coverage_start: Some(coverage_start),
        coverage_end: None,
        observed_at: Some(observed_at),
        status: if fully_covered {
            AnalyticsSourceStatus::Available
        } else {
            AnalyticsSourceStatus::Partial
        },
        diagnostic: if fully_covered {
            None
        } else {
            Some(AnalyticsDiagnostic::InsufficientCoverage)
        },
    }
}

// ---------------------------------------------------------------------------
// Behavioral (provider) composition helpers
// ---------------------------------------------------------------------------

pub fn behavioral_coverage(window: Option<&BehavioralWindow>) -> AnalyticsSourceCoverage {
    let Some(window) = window else {
        return AnalyticsSourceCoverage {
            coverage_start: None,
            coverage_end: None,
            observed_at: None,
            status: AnalyticsSourceStatus::Unavailable,
            diagnostic: Some(AnalyticsDiagnostic::Upstream),
        };
    };
    AnalyticsSourceCoverage {
        coverage_start: window.coverage.coverage_start,
        coverage_end: window.coverage.coverage_end,
        observed_at: Some(window.observed_at),
        status: window.availability,
        diagnostic: window.diagnostic,
    }
}

pub fn behavioral_available(window: Option<&BehavioralWindow>) -> bool {
    window.is_some_and(|window| window.availability != AnalyticsSourceStatus::Unavailable)
}

/// Duplicate labels are aggregated before suppression. A registered event
/// absent from an available response is a genuine zero; an unavailable
/// component is `None`, never zero.
pub fn behavioral_event_cell(window: Option<&BehavioralWindow>, event: &str) -> AggregateCell {
    let Some(window) = window else {
        return AggregateCell {
            key: event.to_owned(),
            count: None,
            suppressed: false,
        };
    };
    let Some(rows) = window.event_totals.data() else {
        return AggregateCell {
            key: event.to_owned(),
            count: None,
            suppressed: false,
        };
    };
    let total: u64 = rows
        .iter()
        .filter(|row| row.event == event)
        .map(|row| row.count.count.unwrap_or(0))
        .sum();
    let mut cell = super::types::suppress(Some(total), None);
    cell.key = event.to_owned();
    cell
}

pub fn behavioral_metric(
    window: Option<&BehavioralWindow>,
    previous: Option<&BehavioralWindow>,
    field: fn(&super::types::CoreMetrics) -> Option<u64>,
) -> MetricValue {
    let current = window.and_then(|window| window.core.data()).and_then(field);
    let before = previous
        .and_then(|window| window.core.data())
        .and_then(field);
    let current_cell = super::types::suppress(current, None);
    let previous_cell = super::types::suppress(before, None);
    MetricValue {
        value: current_cell.count,
        previous: previous_cell.count,
        suppressed: current_cell.suppressed || previous_cell.suppressed,
    }
}

/// Dense daily visitor series; 1/2 values are suppressed to `None`, genuine
/// zeros stay visible, and an unavailable component yields no series at all.
pub fn behavioral_daily_series(
    window: Option<&BehavioralWindow>,
    query: &AnalyticsQuery,
) -> Vec<super::types::SeriesPoint> {
    let Some(window) = window else {
        return Vec::new();
    };
    let Some(rows) = window.daily_visitors.data() else {
        return Vec::new();
    };
    let mut values: BTreeMap<NaiveDate, u64> = BTreeMap::new();
    for row in rows {
        let total = values.entry(row.date).or_insert(0);
        *total += row.visitors.count.unwrap_or(0);
    }
    let mut points = Vec::new();
    let mut cursor = query.start.date_naive();
    while cursor < query.end.date_naive() && points.len() < 366 {
        let value = values.get(&cursor).copied().unwrap_or(0);
        points.push(super::types::SeriesPoint {
            date: cursor,
            value: super::types::suppress(Some(value), None).count,
        });
        cursor += TimeDelta::days(1);
    }
    points
}

/// Property breakdown rows for one (event, property) pair. `None` means the
/// provider did not answer for that pair; callers must not treat it as zero.
pub fn behavioral_property_rows<'a>(
    window: Option<&'a BehavioralWindow>,
    event: &str,
    property: &str,
) -> Option<&'a [super::types::AllowedPropertyAggregate]> {
    let window = window?;
    let availability = window
        .property_results
        .iter()
        .find(|item| item.event == event && item.property == property)?;
    availability.result.data().map(|rows| rows.as_slice())
}

// ---------------------------------------------------------------------------
// Fact containers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoleCounts {
    pub buyer_current: u64,
    pub buyer_previous: u64,
    pub supplier_current: u64,
    pub supplier_previous: u64,
}

impl RoleCounts {
    pub fn current(&self) -> u64 {
        self.buyer_current + self.supplier_current
    }

    pub fn previous(&self) -> u64 {
        self.buyer_previous + self.supplier_previous
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DropOffFacts {
    pub rejected: u64,
    pub unverified: u64,
    pub pending_approval: u64,
    pub organization_incomplete: u64,
    pub never_logged_in: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProvenanceRow {
    pub provenance: String,
    pub orders_current: u64,
    pub orders_previous: u64,
    pub bids_current: u64,
    pub asks_current: u64,
    pub orgs_current: u64,
    pub orgs_previous: u64,
    pub bid_orgs_current: u64,
    pub ask_orgs_current: u64,
    pub executed_current: u64,
    pub executed_previous: u64,
    pub still_open_current: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TradeFacts {
    pub provenance: String,
    pub confirmed_current_strict: u64,
    pub confirmed_previous_strict: u64,
    pub legacy_current: u64,
    pub legacy_previous: u64,
    pub volume_current_strict: Decimal,
    pub volume_current_legacy: Decimal,
    pub volume_previous: Decimal,
    pub gmv_current: Decimal,
    pub gmv_previous: Decimal,
    pub missing_paid_at_current: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivityFacts {
    pub retained_current: u64,
    pub retained_previous: u64,
    pub reactivated_current: u64,
    pub reactivated_previous: u64,
    pub trading_current: u64,
    pub trading_previous: u64,
    pub active_days_1: u64,
    pub active_days_2: u64,
    pub active_days_3_plus: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoginFacts {
    pub coverage_start: Option<NaiveDate>,
    pub active_current: u64,
    pub active_previous: u64,
    pub dau: u64,
    pub wau: u64,
    pub mau: u64,
    pub daily: BTreeMap<NaiveDate, u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusFacts {
    pub approved_current: u64,
    pub approved_previous: u64,
    pub qualified_end: Option<u64>,
    pub qualified_previous_end: Option<u64>,
    pub coverage_start: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoginRollupRow {
    pub first_login_at: DateTime<Utc>,
    pub first_login_date: NaiveDate,
    pub in_current: bool,
    pub in_previous: bool,
    pub registered_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OverviewFacts {
    pub registered: RoleCounts,
    pub drop_off: DropOffFacts,
    pub by_provenance: Vec<ProvenanceRow>,
    pub trades: Vec<TradeFacts>,
    pub activity: ActivityFacts,
    pub login: LoginFacts,
    pub status: StatusFacts,
    pub daily_orders: BTreeMap<NaiveDate, u64>,
    pub daily_trades: BTreeMap<NaiveDate, u64>,
}

impl OverviewFacts {
    pub fn orders(&self, provenance: &str) -> Option<&ProvenanceRow> {
        self.by_provenance
            .iter()
            .find(|row| row.provenance == provenance)
    }

    pub fn trades(&self, provenance: &str) -> Option<&TradeFacts> {
        self.trades.iter().find(|row| row.provenance == provenance)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivationFacts {
    pub registered: RoleCounts,
    pub drop_off: DropOffFacts,
    /// First live order instant and the organization's creation instant.
    pub first_live_orders: Vec<(DateTime<Utc>, DateTime<Utc>)>,
    pub status: StatusFacts,
    pub logins: Vec<LoginRollupRow>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngagementFacts {
    pub login: LoginFacts,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetentionFacts {
    pub activity: ActivityFacts,
    pub drop_off: DropOffFacts,
    pub rollup: Vec<LoginRollupRow>,
    /// Distinct member login dates per user, before the as-of instant.
    pub member_days: Vec<(Uuid, NaiveDate)>,
    /// Distinct live organization activity dates, before the as-of instant.
    pub organization_days: Vec<(Uuid, NaiveDate)>,
}

// ---------------------------------------------------------------------------
// Loaders
// ---------------------------------------------------------------------------

fn window_bounds(
    query: &AnalyticsQuery,
) -> (
    Option<NaiveDate>,
    Option<NaiveDate>,
    Option<NaiveDate>,
    Option<NaiveDate>,
) {
    let (current_lower, current_upper) = date_window(Some(query.start), Some(query.end))
        .unwrap_or((query.start.date_naive(), query.end.date_naive()));
    match (query.previous_start(), query.previous_end())
        .and_then(|(start, end)| date_window(Some(start), Some(end)))
    {
        Some((lower, upper)) => (
            Some(current_lower),
            Some(current_upper),
            Some(lower),
            Some(upper),
        ),
        None => (Some(current_lower), Some(current_upper), None, None),
    }
}

pub async fn load_users_facts(
    conn: &mut PgConnection,
    query: &AnalyticsQuery,
) -> Result<(RoleCounts, DropOffFacts), sqlx::Error> {
    let demo = demo_ids();
    let rows = sqlx::query(
        "WITH member_users AS (
             SELECT role, status, email_verified, organization_id, created_at, last_login
             FROM users
             WHERE role IN ('BUYER','SUPPLIER')
               AND (organization_id IS NULL OR organization_id <> ALL($5::uuid[]))
         )
         SELECT 'registered' AS kind, role::text AS key,
                COUNT(*) FILTER (WHERE created_at >= $1 AND created_at < $2)::bigint AS current,
                COUNT(*) FILTER (
                    WHERE $3::timestamptz IS NOT NULL AND created_at >= $3 AND created_at < $4
                )::bigint AS previous
         FROM member_users
         WHERE created_at >= COALESCE($3::timestamptz, $1::timestamptz) AND created_at < $2
         GROUP BY role
         UNION ALL
         SELECT 'drop_off', 'rejected',
                COUNT(*) FILTER (WHERE status = 'REJECTED')::bigint, 0::bigint
         FROM member_users
         UNION ALL
         SELECT 'drop_off', 'unverified',
                COUNT(*) FILTER (WHERE status <> 'REJECTED' AND NOT email_verified)::bigint, 0::bigint
         FROM member_users
         UNION ALL
         SELECT 'drop_off', 'pending_approval',
                COUNT(*) FILTER (
                    WHERE status <> 'REJECTED' AND email_verified AND status = 'PENDING'
                )::bigint, 0::bigint
         FROM member_users
         UNION ALL
         SELECT 'drop_off', 'organization_incomplete',
                COUNT(*) FILTER (
                    WHERE status = 'APPROVED' AND email_verified AND organization_id IS NULL
                )::bigint, 0::bigint
         FROM member_users
         UNION ALL
         SELECT 'drop_off', 'never_logged_in',
                COUNT(*) FILTER (
                    WHERE status = 'APPROVED' AND email_verified AND organization_id IS NOT NULL
                      AND last_login IS NULL
                )::bigint, 0::bigint
         FROM member_users",
    )
    .bind(query.start)
    .bind(query.end)
    .bind(query.previous_start())
    .bind(query.previous_end())
    .bind(&demo)
    .fetch_all(&mut *conn)
    .await?;
    let mut registered = RoleCounts::default();
    let mut drop_off = DropOffFacts::default();
    for row in rows {
        let kind: String = row.get("kind");
        let key: String = row.get("key");
        let current = count(row.get::<i64, _>("current"));
        let previous = count(row.get::<i64, _>("previous"));
        match (kind.as_str(), key.as_str()) {
            ("registered", "BUYER") => {
                registered.buyer_current = current;
                registered.buyer_previous = previous;
            }
            ("registered", "SUPPLIER") => {
                registered.supplier_current = current;
                registered.supplier_previous = previous;
            }
            ("drop_off", "rejected") => drop_off.rejected = current,
            ("drop_off", "unverified") => drop_off.unverified = current,
            ("drop_off", "pending_approval") => drop_off.pending_approval = current,
            ("drop_off", "organization_incomplete") => drop_off.organization_incomplete = current,
            ("drop_off", "never_logged_in") => drop_off.never_logged_in = current,
            _ => {}
        }
    }
    Ok((registered, drop_off))
}

pub async fn load_drop_off(conn: &mut PgConnection) -> Result<DropOffFacts, sqlx::Error> {
    let demo = demo_ids();
    let row = sqlx::query(
        "SELECT
            COUNT(*) FILTER (WHERE status = 'REJECTED')::bigint AS rejected,
            COUNT(*) FILTER (WHERE status <> 'REJECTED' AND NOT email_verified)::bigint AS unverified,
            COUNT(*) FILTER (
                WHERE status <> 'REJECTED' AND email_verified AND status = 'PENDING'
            )::bigint AS pending_approval,
            COUNT(*) FILTER (
                WHERE status = 'APPROVED' AND email_verified AND organization_id IS NULL
            )::bigint AS organization_incomplete,
            COUNT(*) FILTER (
                WHERE status = 'APPROVED' AND email_verified AND organization_id IS NOT NULL
                  AND last_login IS NULL
            )::bigint AS never_logged_in
         FROM users
         WHERE role IN ('BUYER','SUPPLIER')
           AND (organization_id IS NULL OR organization_id <> ALL($1::uuid[]))",
    )
    .bind(&demo)
    .fetch_one(&mut *conn)
    .await?;
    Ok(DropOffFacts {
        rejected: count(row.get("rejected")),
        unverified: count(row.get("unverified")),
        pending_approval: count(row.get("pending_approval")),
        organization_incomplete: count(row.get("organization_incomplete")),
        never_logged_in: count(row.get("never_logged_in")),
    })
}

pub async fn load_orders_facts(
    conn: &mut PgConnection,
    query: &AnalyticsQuery,
) -> Result<Vec<ProvenanceRow>, sqlx::Error> {
    let demo = demo_ids();
    let rows = sqlx::query(
        "SELECT CASE WHEN o.provenance = 'DEMO' OR o.organization_id = ANY($5::uuid[]) THEN 'demo'
                     WHEN o.provenance = 'REAL' THEN 'live' ELSE 'unknown' END AS provenance,
                COUNT(*) FILTER (WHERE o.created_at >= $1 AND o.created_at < $2)::bigint AS orders_current,
                COUNT(*) FILTER (WHERE o.created_at >= $3 AND o.created_at < $4)::bigint AS orders_previous,
                COUNT(*) FILTER (
                    WHERE o.created_at >= $1 AND o.created_at < $2 AND o.side = 'BID'
                )::bigint AS bids_current,
                COUNT(*) FILTER (
                    WHERE o.created_at >= $1 AND o.created_at < $2 AND o.side = 'ASK'
                )::bigint AS asks_current,
                COUNT(DISTINCT CASE WHEN o.created_at >= $1 AND o.created_at < $2
                    THEN o.organization_id END)::bigint AS orgs_current,
                COUNT(DISTINCT CASE WHEN o.created_at >= $3 AND o.created_at < $4
                    THEN o.organization_id END)::bigint AS orgs_previous,
                COUNT(DISTINCT CASE WHEN o.created_at >= $1 AND o.created_at < $2 AND o.side = 'BID'
                    THEN o.organization_id END)::bigint AS bid_orgs_current,
                COUNT(DISTINCT CASE WHEN o.created_at >= $1 AND o.created_at < $2 AND o.side = 'ASK'
                    THEN o.organization_id END)::bigint AS ask_orgs_current,
                COUNT(*) FILTER (WHERE o.created_at >= $1 AND o.created_at < $2 AND EXISTS (
                    SELECT 1 FROM trades t
                    WHERE (t.bid_order_id = o.id OR t.ask_order_id = o.id)
                      AND t.status IN ('CONFIRMED','DELIVERED','PAID')
                      AND t.confirmed_at IS NOT NULL AND t.confirmed_at < $2
                ))::bigint AS executed_current,
                COUNT(*) FILTER (WHERE $3::timestamptz IS NOT NULL
                    AND o.created_at >= $3 AND o.created_at < $4 AND EXISTS (
                    SELECT 1 FROM trades t
                    WHERE (t.bid_order_id = o.id OR t.ask_order_id = o.id)
                      AND t.status IN ('CONFIRMED','DELIVERED','PAID')
                      AND t.confirmed_at IS NOT NULL AND t.confirmed_at < $4
                ))::bigint AS executed_previous,
                COUNT(*) FILTER (
                    WHERE o.created_at >= $1 AND o.created_at < $2
                      AND o.status IN ('OPEN','PARTIALLY_FILLED')
                      AND o.remaining_quantity_mt > 0
                      AND (o.expires_at IS NULL OR o.expires_at >= $2)
                )::bigint AS still_open_current
         FROM orderbook_orders o
         WHERE o.created_at >= COALESCE($3::timestamptz, $1::timestamptz) AND o.created_at < $2
         GROUP BY 1",
    )
    .bind(query.start)
    .bind(query.end)
    .bind(query.previous_start())
    .bind(query.previous_end())
    .bind(&demo)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ProvenanceRow {
            provenance: row.get("provenance"),
            orders_current: count(row.get("orders_current")),
            orders_previous: count(row.get("orders_previous")),
            bids_current: count(row.get("bids_current")),
            asks_current: count(row.get("asks_current")),
            orgs_current: count(row.get("orgs_current")),
            orgs_previous: count(row.get("orgs_previous")),
            bid_orgs_current: count(row.get("bid_orgs_current")),
            ask_orgs_current: count(row.get("ask_orgs_current")),
            executed_current: count(row.get("executed_current")),
            executed_previous: count(row.get("executed_previous")),
            still_open_current: count(row.get("still_open_current")),
        })
        .collect())
}

pub async fn load_trades_facts(
    conn: &mut PgConnection,
    query: &AnalyticsQuery,
) -> Result<Vec<TradeFacts>, sqlx::Error> {
    let demo = demo_ids();
    let rows = sqlx::query(
        "SELECT CASE WHEN t.buyer_provenance = 'DEMO' OR t.buyer_id = ANY($5::uuid[])
                          OR t.seller_provenance = 'DEMO' OR t.seller_id = ANY($5::uuid[]) THEN 'demo'
                     WHEN t.buyer_provenance = 'REAL' AND t.seller_provenance = 'REAL' THEN 'live'
                     ELSE 'unknown' END AS provenance,
                COUNT(*) FILTER (WHERE t.confirmed_at >= $1 AND t.confirmed_at < $2)::bigint
                    AS confirmed_current_strict,
                COUNT(*) FILTER (WHERE t.confirmed_at >= $3 AND t.confirmed_at < $4)::bigint
                    AS confirmed_previous_strict,
                COUNT(*) FILTER (
                    WHERE t.confirmed_at IS NULL AND t.created_at >= $1 AND t.created_at < $2
                )::bigint AS legacy_current,
                COUNT(*) FILTER (
                    WHERE t.confirmed_at IS NULL AND t.created_at >= $3 AND t.created_at < $4
                )::bigint AS legacy_previous,
                COALESCE(SUM(CASE WHEN t.confirmed_at >= $1 AND t.confirmed_at < $2
                    THEN COALESCE(t.final_quantity_mt, t.quantity_mt) ELSE 0 END), 0) AS volume_current_strict,
                COALESCE(SUM(CASE WHEN t.confirmed_at IS NULL
                    AND t.created_at >= $1 AND t.created_at < $2
                    THEN COALESCE(t.final_quantity_mt, t.quantity_mt) ELSE 0 END), 0) AS volume_current_legacy,
                COALESCE(SUM(CASE WHEN (t.confirmed_at >= $3 AND t.confirmed_at < $4)
                    OR (t.confirmed_at IS NULL AND t.created_at >= $3 AND t.created_at < $4)
                    THEN COALESCE(t.final_quantity_mt, t.quantity_mt) ELSE 0 END), 0) AS volume_previous,
                COALESCE(SUM(CASE WHEN t.status = 'PAID' AND t.paid_at >= $1 AND t.paid_at < $2
                    THEN t.final_total_usd ELSE 0 END), 0) AS gmv_current,
                COALESCE(SUM(CASE WHEN t.status = 'PAID' AND t.paid_at >= $3 AND t.paid_at < $4
                    THEN t.final_total_usd ELSE 0 END), 0) AS gmv_previous,
                COUNT(*) FILTER (WHERE t.status = 'PAID' AND t.paid_at IS NULL
                    AND COALESCE(t.confirmed_at, t.created_at) >= $1
                    AND COALESCE(t.confirmed_at, t.created_at) < $2)::bigint AS missing_paid_at_current
         FROM trades t
         WHERE t.status IN ('CONFIRMED','DELIVERED','PAID')
           AND ((COALESCE(t.confirmed_at, t.created_at) >= COALESCE($3::timestamptz, $1::timestamptz)
                 AND COALESCE(t.confirmed_at, t.created_at) < $2)
                OR (t.status = 'PAID'
                    AND t.paid_at >= COALESCE($3::timestamptz, $1::timestamptz)
                    AND t.paid_at < $2))
         GROUP BY 1",
    )
    .bind(query.start)
    .bind(query.end)
    .bind(query.previous_start())
    .bind(query.previous_end())
    .bind(&demo)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| TradeFacts {
            provenance: row.get("provenance"),
            confirmed_current_strict: count(row.get("confirmed_current_strict")),
            confirmed_previous_strict: count(row.get("confirmed_previous_strict")),
            legacy_current: count(row.get("legacy_current")),
            legacy_previous: count(row.get("legacy_previous")),
            volume_current_strict: row.get("volume_current_strict"),
            volume_current_legacy: row.get("volume_current_legacy"),
            volume_previous: row.get("volume_previous"),
            gmv_current: row.get("gmv_current"),
            gmv_previous: row.get("gmv_previous"),
            missing_paid_at_current: count(row.get("missing_paid_at_current")),
        })
        .collect())
}

fn daily_map(rows: &[sqlx::postgres::PgRow], value: &str) -> BTreeMap<NaiveDate, u64> {
    rows.iter()
        .map(|row| {
            (
                row.get::<NaiveDate, _>("day"),
                count(row.get::<i64, _>(value)),
            )
        })
        .collect()
}

pub async fn load_daily_orders(
    conn: &mut PgConnection,
    query: &AnalyticsQuery,
) -> Result<BTreeMap<NaiveDate, u64>, sqlx::Error> {
    let demo = demo_ids();
    let rows = sqlx::query(
        "SELECT (o.created_at AT TIME ZONE 'UTC')::date AS day, COUNT(*)::bigint AS orders
         FROM orderbook_orders o
         WHERE o.created_at >= $1 AND o.created_at < $2
           AND o.organization_id IN (
               SELECT DISTINCT u.organization_id FROM users u
               WHERE u.organization_id IS NOT NULL AND u.role IN ('BUYER','SUPPLIER')
                 AND u.status = 'APPROVED' AND u.organization_id <> ALL($3::uuid[]))
         GROUP BY 1",
    )
    .bind(query.start)
    .bind(query.end)
    .bind(&demo)
    .fetch_all(&mut *conn)
    .await?;
    Ok(daily_map(&rows, "orders"))
}

pub async fn load_daily_trades(
    conn: &mut PgConnection,
    query: &AnalyticsQuery,
) -> Result<BTreeMap<NaiveDate, u64>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT (COALESCE(t.confirmed_at, t.created_at) AT TIME ZONE 'UTC')::date AS day,
                COUNT(*)::bigint AS trades
         FROM trades t
         WHERE t.status IN ('CONFIRMED','DELIVERED','PAID')
           AND t.buyer_provenance = 'REAL' AND t.seller_provenance = 'REAL'
           AND t.buyer_id <> ALL($3::uuid[]) AND t.seller_id <> ALL($3::uuid[])
           AND COALESCE(t.confirmed_at, t.created_at) >= $1
           AND COALESCE(t.confirmed_at, t.created_at) < $2
         GROUP BY 1",
    )
    .bind(query.start)
    .bind(query.end)
    .bind(&demo_ids())
    .fetch_all(&mut *conn)
    .await?;
    Ok(daily_map(&rows, "trades"))
}

/// Live organization activity instants: order creation and strict economic
/// trade confirmation. Provenance is the stored party provenance, so a trade
/// is live only when both parties are REAL; the demo identity set is excluded
/// from the order arm through the recognized-organization filter.
const ACTIVITY_SQL: &str = "
WITH activity AS (
    SELECT o.organization_id AS org_id, o.created_at AS ts, 'order' AS kind
    FROM orderbook_orders o
    WHERE o.organization_id IN (
        SELECT DISTINCT u.organization_id FROM users u
        WHERE u.organization_id IS NOT NULL AND u.role IN ('BUYER','SUPPLIER')
          AND u.status = 'APPROVED' AND u.organization_id <> ALL($5::uuid[]))
    UNION ALL
    SELECT t.buyer_id, t.confirmed_at, 'trade' FROM trades t
    WHERE t.status IN ('CONFIRMED','DELIVERED','PAID') AND t.confirmed_at IS NOT NULL
      AND t.buyer_provenance = 'REAL' AND t.seller_provenance = 'REAL'
      AND t.buyer_id <> ALL($5::uuid[]) AND t.seller_id <> ALL($5::uuid[])
    UNION ALL
    SELECT t.seller_id, t.confirmed_at, 'trade' FROM trades t
    WHERE t.status IN ('CONFIRMED','DELIVERED','PAID') AND t.confirmed_at IS NOT NULL
      AND t.buyer_provenance = 'REAL' AND t.seller_provenance = 'REAL'
      AND t.buyer_id <> ALL($5::uuid[]) AND t.seller_id <> ALL($5::uuid[])
),
per_org AS (
    SELECT org_id,
           max(CASE WHEN ts >= $1 AND ts < $2 THEN 1 ELSE 0 END) AS cur,
           max(CASE WHEN $3::timestamptz IS NOT NULL AND ts >= $3 AND ts < $4
               THEN 1 ELSE 0 END) AS prev,
           max(CASE WHEN $6::timestamptz IS NOT NULL AND ts >= $6 AND ts < $3
               THEN 1 ELSE 0 END) AS prev_prev,
           max(CASE WHEN ts < $7 THEN 1 ELSE 0 END) AS earlier,
           max(CASE WHEN kind = 'trade' AND ts >= $1 AND ts < $2
               THEN 1 ELSE 0 END) AS trading_cur,
           max(CASE WHEN kind = 'trade' AND $3::timestamptz IS NOT NULL
               AND ts >= $3 AND ts < $4 THEN 1 ELSE 0 END) AS trading_prev,
           count(DISTINCT CASE WHEN ts >= $1 AND ts < $2
               THEN (ts AT TIME ZONE 'UTC')::date END) AS active_days
    FROM activity GROUP BY org_id
)
SELECT COUNT(*) FILTER (WHERE cur = 1 AND prev = 1)::bigint AS retained_current,
       COUNT(*) FILTER (WHERE prev = 1 AND prev_prev = 1)::bigint AS retained_previous,
       COUNT(*) FILTER (WHERE cur = 1 AND prev = 0
           AND (prev_prev = 1 OR earlier = 1))::bigint AS reactivated_current,
       COUNT(*) FILTER (WHERE prev = 1 AND prev_prev = 0
           AND earlier = 1)::bigint AS reactivated_previous,
       COUNT(*) FILTER (WHERE trading_cur = 1)::bigint AS trading_current,
       COUNT(*) FILTER (WHERE trading_prev = 1)::bigint AS trading_previous,
       COUNT(*) FILTER (WHERE active_days = 1)::bigint AS days_1,
       COUNT(*) FILTER (WHERE active_days = 2)::bigint AS days_2,
       COUNT(*) FILTER (WHERE active_days >= 3)::bigint AS days_3_plus
FROM per_org";

/// The same activity instants projected to distinct UTC dates, for the
/// organization cohort input. Dates may predate login-fact coverage.
const ORGANIZATION_DAYS_SQL: &str = "
WITH activity AS (
    SELECT o.organization_id AS org_id, o.created_at AS ts
    FROM orderbook_orders o
    WHERE o.organization_id IN (
        SELECT DISTINCT u.organization_id FROM users u
        WHERE u.organization_id IS NOT NULL AND u.role IN ('BUYER','SUPPLIER')
          AND u.status = 'APPROVED' AND u.organization_id <> ALL($2::uuid[]))
    UNION ALL
    SELECT t.buyer_id, t.confirmed_at FROM trades t
    WHERE t.status IN ('CONFIRMED','DELIVERED','PAID') AND t.confirmed_at IS NOT NULL
      AND t.buyer_provenance = 'REAL' AND t.seller_provenance = 'REAL'
      AND t.buyer_id <> ALL($2::uuid[]) AND t.seller_id <> ALL($2::uuid[])
    UNION ALL
    SELECT t.seller_id, t.confirmed_at FROM trades t
    WHERE t.status IN ('CONFIRMED','DELIVERED','PAID') AND t.confirmed_at IS NOT NULL
      AND t.buyer_provenance = 'REAL' AND t.seller_provenance = 'REAL'
      AND t.buyer_id <> ALL($2::uuid[]) AND t.seller_id <> ALL($2::uuid[])
)
SELECT DISTINCT org_id, (ts AT TIME ZONE 'UTC')::date AS activity_date
FROM activity
WHERE ts IS NOT NULL AND ts < $1";

pub async fn load_activity_facts(
    conn: &mut PgConnection,
    query: &AnalyticsQuery,
) -> Result<ActivityFacts, sqlx::Error> {
    let demo = demo_ids();
    let previous_start = query.previous_start();
    let previous_end = query.previous_end();
    let previous_previous_start = match (previous_start, previous_end) {
        (Some(start), Some(end)) => Some(start - (end - start)),
        _ => None,
    };
    let earlier_bound = previous_previous_start
        .or(previous_start)
        .unwrap_or(query.start);
    let row = sqlx::query(ACTIVITY_SQL)
        .bind(query.start)
        .bind(query.end)
        .bind(previous_start)
        .bind(previous_end)
        .bind(&demo)
        .bind(previous_previous_start)
        .bind(earlier_bound)
        .fetch_one(&mut *conn)
        .await?;
    Ok(ActivityFacts {
        retained_current: count(row.get("retained_current")),
        retained_previous: count(row.get("retained_previous")),
        reactivated_current: count(row.get("reactivated_current")),
        reactivated_previous: count(row.get("reactivated_previous")),
        trading_current: count(row.get("trading_current")),
        trading_previous: count(row.get("trading_previous")),
        active_days_1: count(row.get("days_1")),
        active_days_2: count(row.get("days_2")),
        active_days_3_plus: count(row.get("days_3_plus")),
    })
}

pub async fn load_login_facts(
    conn: &mut PgConnection,
    query: &AnalyticsQuery,
) -> Result<LoginFacts, sqlx::Error> {
    let demo = demo_ids();
    let (current_lower, current_upper, previous_lower, previous_upper) = window_bounds(query);
    let rows = sqlx::query(
        "WITH member_days AS (
             SELECT activity_date, user_id
             FROM user_login_days
             WHERE role IN ('BUYER','SUPPLIER')
               AND (organization_id IS NULL OR organization_id <> ALL($5::uuid[]))
         )
         SELECT 'daily' AS kind, activity_date AS day, NULL::date AS coverage_start,
                COUNT(DISTINCT user_id)::bigint AS active_current,
                0::bigint AS active_previous, 0::bigint AS dau, 0::bigint AS wau, 0::bigint AS mau
         FROM member_days
         WHERE activity_date >= $1::date AND activity_date < $2::date
         GROUP BY activity_date
         UNION ALL
         SELECT 'summary', NULL::date, min(activity_date),
                COUNT(DISTINCT CASE WHEN activity_date >= $1::date AND activity_date < $2::date
                    THEN user_id END)::bigint,
                COUNT(DISTINCT CASE WHEN $3::date IS NOT NULL
                    AND activity_date >= $3::date AND activity_date < $4::date
                    THEN user_id END)::bigint,
                COUNT(DISTINCT CASE WHEN activity_date >= $2::date - 1 THEN user_id END)::bigint,
                COUNT(DISTINCT CASE WHEN activity_date >= $2::date - 7 THEN user_id END)::bigint,
                COUNT(DISTINCT CASE WHEN activity_date >= $2::date - 30 THEN user_id END)::bigint
         FROM member_days
         WHERE activity_date < $2::date",
    )
    .bind(current_lower)
    .bind(current_upper)
    .bind(previous_lower)
    .bind(previous_upper)
    .bind(&demo)
    .fetch_all(&mut *conn)
    .await?;
    let mut facts = LoginFacts::default();
    for row in rows {
        let kind: String = row.get("kind");
        if kind == "summary" {
            facts.coverage_start = row.get("coverage_start");
            facts.active_current = count(row.get("active_current"));
            facts.active_previous = count(row.get("active_previous"));
            facts.dau = count(row.get("dau"));
            facts.wau = count(row.get("wau"));
            facts.mau = count(row.get("mau"));
        } else {
            facts.daily.insert(
                row.get::<NaiveDate, _>("day"),
                count(row.get("active_current")),
            );
        }
    }
    Ok(facts)
}

pub async fn load_status_facts(
    conn: &mut PgConnection,
    query: &AnalyticsQuery,
) -> Result<StatusFacts, sqlx::Error> {
    let demo = demo_ids();
    let row = sqlx::query(
        "SELECT
            (SELECT COUNT(*) FROM user_status_transitions t
             WHERE t.to_status = 'APPROVED' AND t.role IN ('BUYER','SUPPLIER')
               AND (t.organization_id IS NULL OR t.organization_id <> ALL($5::uuid[]))
               AND t.effective_at >= $1 AND t.effective_at < $2)::bigint AS approved_current,
            (SELECT COUNT(*) FROM user_status_transitions t
             WHERE t.to_status = 'APPROVED' AND t.role IN ('BUYER','SUPPLIER')
               AND (t.organization_id IS NULL OR t.organization_id <> ALL($5::uuid[]))
               AND $3::timestamptz IS NOT NULL
               AND t.effective_at >= $3 AND t.effective_at < $4)::bigint AS approved_previous,
            (SELECT COUNT(DISTINCT t.organization_id)
             FROM user_status_transitions t
             JOIN (SELECT user_id, max(effective_at) AS latest_at
                   FROM user_status_transitions WHERE effective_at <= $2
                   GROUP BY user_id) latest
               ON latest.user_id = t.user_id AND latest.latest_at = t.effective_at
             WHERE t.to_status = 'APPROVED' AND t.organization_id IS NOT NULL
               AND t.role IN ('BUYER','SUPPLIER') AND t.organization_id <> ALL($5::uuid[]))::bigint
                AS qualified_end,
            CASE WHEN $4::timestamptz IS NULL THEN NULL ELSE
                (SELECT COUNT(DISTINCT t.organization_id)
                 FROM user_status_transitions t
                 JOIN (SELECT user_id, max(effective_at) AS latest_at
                       FROM user_status_transitions WHERE effective_at <= $4
                       GROUP BY user_id) latest
                   ON latest.user_id = t.user_id AND latest.latest_at = t.effective_at
                 WHERE t.to_status = 'APPROVED'
                   AND t.organization_id IS NOT NULL
                   AND t.role IN ('BUYER','SUPPLIER') AND t.organization_id <> ALL($5::uuid[]))::bigint
            END AS qualified_previous_end,
            (SELECT min(effective_at) FROM user_status_transitions) AS coverage_start",
    )
    .bind(query.start)
    .bind(query.end)
    .bind(query.previous_start())
    .bind(query.previous_end())
    .bind(&demo)
    .fetch_one(&mut *conn)
    .await?;
    Ok(StatusFacts {
        approved_current: count(row.get("approved_current")),
        approved_previous: count(row.get("approved_previous")),
        qualified_end: row.get::<Option<i64>, _>("qualified_end").map(count),
        qualified_previous_end: row
            .get::<Option<i64>, _>("qualified_previous_end")
            .map(count),
        coverage_start: row.get("coverage_start"),
    })
}

pub async fn load_first_live_orders(
    conn: &mut PgConnection,
) -> Result<Vec<(DateTime<Utc>, DateTime<Utc>)>, sqlx::Error> {
    let demo = demo_ids();
    let rows = sqlx::query(
        "SELECT min(o.created_at) AS first_order_at, org.created_at AS org_created_at
         FROM orderbook_orders o
         JOIN organizations org ON org.id = o.organization_id
         WHERE o.organization_id IN (
             SELECT DISTINCT u.organization_id FROM users u
             WHERE u.organization_id IS NOT NULL AND u.role IN ('BUYER','SUPPLIER')
               AND u.status = 'APPROVED' AND u.organization_id <> ALL($1::uuid[]))
         GROUP BY o.organization_id, org.created_at",
    )
    .bind(&demo)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| (row.get("first_order_at"), row.get("org_created_at")))
        .collect())
}

pub async fn load_login_rollup(
    conn: &mut PgConnection,
    query: &AnalyticsQuery,
) -> Result<Vec<LoginRollupRow>, sqlx::Error> {
    let demo = demo_ids();
    let (current_lower, current_upper, previous_lower, previous_upper) = window_bounds(query);
    let rows = sqlx::query(
        "WITH per_user AS (
             SELECT user_id,
                    min(first_login_at) AS first_login_at,
                    min(activity_date) AS first_login_date,
                    max(CASE WHEN activity_date >= $1::date AND activity_date < $2::date
                        THEN 1 ELSE 0 END) AS in_current,
                    max(CASE WHEN $3::date IS NOT NULL
                        AND activity_date >= $3::date AND activity_date < $4::date
                        THEN 1 ELSE 0 END) AS in_previous
             FROM user_login_days
             WHERE role IN ('BUYER','SUPPLIER')
               AND (organization_id IS NULL OR organization_id <> ALL($5::uuid[]))
             GROUP BY user_id
         )
         SELECT p.first_login_at, p.first_login_date, p.in_current, p.in_previous,
                u.created_at AS registered_at
         FROM per_user p JOIN users u ON u.id = p.user_id",
    )
    .bind(current_lower)
    .bind(current_upper)
    .bind(previous_lower)
    .bind(previous_upper)
    .bind(&demo)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| LoginRollupRow {
            first_login_at: row.get("first_login_at"),
            first_login_date: row.get("first_login_date"),
            in_current: row.get::<i64, _>("in_current") == 1,
            in_previous: row.get::<i64, _>("in_previous") == 1,
            registered_at: row.get("registered_at"),
        })
        .collect())
}

/// Distinct member login dates before the as-of instant.
///
/// The load is bounded by the fact contract itself: login facts are pruned to
/// 800 UTC dates, so the lower horizon is the retention floor, never an
/// arbitrary truncation of a member's history.
pub async fn load_member_days(
    conn: &mut PgConnection,
    end: DateTime<Utc>,
) -> Result<Vec<(Uuid, NaiveDate)>, sqlx::Error> {
    let demo = demo_ids();
    let upper = date_window(
        Some(midnight(NaiveDate::from_ymd_opt(1970, 1, 1).unwrap())),
        Some(end),
    )
    .map(|(_, upper)| upper)
    .unwrap_or(end.date_naive());
    let floor = upper - TimeDelta::days(799);
    let rows = sqlx::query(
        "SELECT DISTINCT user_id, activity_date
         FROM user_login_days
         WHERE role IN ('BUYER','SUPPLIER')
           AND (organization_id IS NULL OR organization_id <> ALL($2::uuid[]))
           AND activity_date < $1
           AND activity_date >= $3",
    )
    .bind(upper)
    .bind(&demo)
    .bind(floor)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| (row.get("user_id"), row.get("activity_date")))
        .collect())
}

pub async fn load_organization_days(
    conn: &mut PgConnection,
    end: DateTime<Utc>,
) -> Result<Vec<(Uuid, NaiveDate)>, sqlx::Error> {
    let demo = demo_ids();
    let rows = sqlx::query(ORGANIZATION_DAYS_SQL)
        .bind(end)
        .bind(&demo)
        .fetch_all(&mut *conn)
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| (row.get("org_id"), row.get("activity_date")))
        .collect())
}

pub async fn load_overview_facts(
    conn: &mut PgConnection,
    query: &AnalyticsQuery,
) -> Result<OverviewFacts, sqlx::Error> {
    let (registered, drop_off) = load_users_facts(&mut *conn, query).await?;
    let by_provenance = load_orders_facts(&mut *conn, query).await?;
    let daily_orders = load_daily_orders(&mut *conn, query).await?;
    let trades = load_trades_facts(&mut *conn, query).await?;
    let daily_trades = load_daily_trades(&mut *conn, query).await?;
    let activity = load_activity_facts(&mut *conn, query).await?;
    let login = load_login_facts(&mut *conn, query).await?;
    let status = load_status_facts(&mut *conn, query).await?;
    Ok(OverviewFacts {
        registered,
        drop_off,
        by_provenance,
        trades,
        activity,
        login,
        status,
        daily_orders,
        daily_trades,
    })
}

pub async fn load_activation_facts(
    conn: &mut PgConnection,
    query: &AnalyticsQuery,
) -> Result<ActivationFacts, sqlx::Error> {
    let (registered, drop_off) = load_users_facts(&mut *conn, query).await?;
    let first_live_orders = load_first_live_orders(&mut *conn).await?;
    let status = load_status_facts(&mut *conn, query).await?;
    let logins = load_login_rollup(&mut *conn, query).await?;
    Ok(ActivationFacts {
        registered,
        drop_off,
        first_live_orders,
        status,
        logins,
    })
}

pub async fn load_engagement_facts(
    conn: &mut PgConnection,
    query: &AnalyticsQuery,
) -> Result<EngagementFacts, sqlx::Error> {
    Ok(EngagementFacts {
        login: load_login_facts(conn, query).await?,
    })
}

pub async fn load_retention_facts(
    conn: &mut PgConnection,
    query: &AnalyticsQuery,
) -> Result<RetentionFacts, sqlx::Error> {
    let activity = load_activity_facts(&mut *conn, query).await?;
    let drop_off = load_drop_off(&mut *conn).await?;
    let rollup = load_login_rollup(&mut *conn, query).await?;
    let member_days = load_member_days(&mut *conn, query.end).await?;
    let organization_days = load_organization_days(&mut *conn, query.end).await?;
    Ok(RetentionFacts {
        activity,
        drop_off,
        rollup,
        member_days,
        organization_days,
    })
}

/// Monday-anchored ISO week of a calendar date.
pub fn week_start(date: NaiveDate) -> NaiveDate {
    date - TimeDelta::days(i64::from(date.weekday().num_days_from_monday()))
}

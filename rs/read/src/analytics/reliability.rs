//! Reliability tab (P11): provider health, bounded error panels, navigation
//! latency and recent audit activity.
//!
//! The provider panels are behavioral. The audit strip is authoritative, and
//! its sample is **restricted to the requested half-open UTC window** — a fixed
//! "latest twenty rows overall" sample would silently answer a different
//! question than the caller asked. Ordering is newest first with an id
//! tie-break so a repeated request cannot reorder equal timestamps.
//!
//! The audit strip carries no actor identity, no IP address and no free text:
//! action, resource type and the actor's role only.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgConnection, Row};

use super::acquisition::{Reading, event_metric};
use super::marketplace::{
    AggregateCell, AllowedPropertySet, AnalyticsQuery, BehavioralWindow, ComponentStatus,
    Diagnostic, Event, MAX_AUDIT_ROWS, MAX_RANKED_ROWS, Meta, MetricValue, Property,
    PropertyValueCount, Quality, SeriesPoint, SourceStatus, cell, daily_series, database, metric,
};

/// Behavioral events this tab requests, with the breakdowns it reads.
pub fn requested_properties() -> AllowedPropertySet {
    AllowedPropertySet::new([
        (Event::LoginFailed, Property::ReasonCategory),
        (Event::FrontendError, Property::RouteFamily),
        (Event::FrontendError, Property::Category),
        (Event::BackendUnavailable, Property::RouteFamily),
        (Event::NavigationPerformance, Property::LatencyBucket),
    ])
    .expect("the reliability property set is static and valid")
}

/// Closed latency buckets, in display order.
pub const LATENCY_BUCKETS: [&str; 5] = ["lt250", "250_500", "500_1000", "1000_2500", "gte2500"];

#[derive(Clone, Debug, Serialize)]
pub struct CollectorState {
    pub status: SourceStatus,
    pub diagnostic: Option<Diagnostic>,
    pub last_observation_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LoginFailurePanel {
    pub total: MetricValue,
    pub categories: Vec<AggregateCell>,
    pub trend: Vec<SeriesPoint>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FrontendErrorPanel {
    pub total: MetricValue,
    pub by_route_family: Vec<AggregateCell>,
    pub by_category: Vec<AggregateCell>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BackendUnavailablePanel {
    pub total: MetricValue,
    pub by_route_family: Vec<AggregateCell>,
}

#[derive(Clone, Debug, Serialize)]
pub struct NavigationLatencyRow {
    pub destination: String,
    pub buckets: Vec<AggregateCell>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AuditActivityRow {
    pub occurred_at: DateTime<Utc>,
    pub action: String,
    pub resource_type: Option<String>,
    pub actor_role: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReliabilityResponse {
    pub meta: Meta,
    pub collector: CollectorState,
    pub login_failures: LoginFailurePanel,
    pub frontend_errors: FrontendErrorPanel,
    pub backend_unavailable: BackendUnavailablePanel,
    pub navigation_latency: Vec<NavigationLatencyRow>,
    pub audit_activity: Vec<AuditActivityRow>,
}

/// The audit sample: at most twenty rows inside `[start, end)`, newest first,
/// ties broken by id so the order is total.
///
/// `audit_logs` is append-only, so an `id` tie-break never reorders past rows;
/// it only makes the equal-timestamp case deterministic.
pub async fn audit_activity(
    conn: &mut PgConnection,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<Vec<AuditActivityRow>, crate::ApiError> {
    let rows = sqlx::query(
        "SELECT a.timestamp AS occurred_at, a.action, a.resource_type, u.role::text AS actor_role\nFROM audit_logs a\nLEFT JOIN users u ON u.id = a.user_id\nWHERE a.timestamp >= $1 AND a.timestamp < $2\nORDER BY a.timestamp DESC, a.id DESC\nLIMIT $3",
    )
    .bind(start)
    .bind(end)
    .bind(MAX_AUDIT_ROWS as i64)
    .fetch_all(&mut *conn)
    .await
    .map_err(database)?;
    Ok(rows
        .into_iter()
        .map(|row| AuditActivityRow {
            occurred_at: row.get("occurred_at"),
            action: row.get("action"),
            resource_type: row.get("resource_type"),
            actor_role: row.get("actor_role"),
        })
        .collect())
}

fn property_rows(
    window: &BehavioralWindow,
    event: Event,
    property: Property,
) -> Option<&Vec<PropertyValueCount>> {
    let result = window.property(event, property)?;
    if !matches!(
        result.result.status(),
        ComponentStatus::Available | ComponentStatus::Partial
    ) {
        return None;
    }
    result.result.data()
}

fn cells(
    window: &BehavioralWindow,
    event: Event,
    property: Property,
    limit: usize,
    quality: &mut Quality,
) -> Vec<AggregateCell> {
    let Some(rows) = property_rows(window, event, property) else {
        return Vec::new();
    };
    rows.iter()
        .take(limit)
        .map(|row| {
            cell(
                row.value.clone(),
                Some(row.total as i64),
                Some(row.total as i64),
                quality,
            )
        })
        .collect()
}

fn event_trend(
    window: &BehavioralWindow,
    event: Event,
    query: &AnalyticsQuery,
    quality: &mut Quality,
) -> Vec<SeriesPoint> {
    match window.event_day(event) {
        Some(values) => daily_series(query.start, query.end, &values, quality),
        None => Vec::new(),
    }
}

fn navigation_latency(
    window: &BehavioralWindow,
    quality: &mut Quality,
) -> Vec<NavigationLatencyRow> {
    let Some(rows) = property_rows(
        window,
        Event::NavigationPerformance,
        Property::LatencyBucket,
    ) else {
        return Vec::new();
    };
    if rows.is_empty() {
        return Vec::new();
    }
    // The verified property API has no per-destination cross-tab, so the
    // cross-destination distribution is labelled `all` rather than invented
    // per destination.
    let buckets = LATENCY_BUCKETS
        .iter()
        .map(|bucket| {
            let total: i64 = rows
                .iter()
                .filter(|row| row.value == *bucket)
                .map(|row| row.total as i64)
                .sum();
            cell(*bucket, Some(total), Some(total), quality)
        })
        .collect();
    vec![NavigationLatencyRow {
        destination: "all".to_owned(),
        buckets,
    }]
}

/// Compose the reliability tab.
pub fn compose(
    query: &AnalyticsQuery,
    meta: Meta,
    current: Option<&Reading>,
    previous: Option<&Reading>,
    audit: Vec<AuditActivityRow>,
    quality: &mut Quality,
) -> ReliabilityResponse {
    let window = current
        .filter(|reading| reading.window.status() != SourceStatus::Unavailable)
        .map(|reading| &reading.window);

    let collector = CollectorState {
        status: current
            .map(|reading| reading.status())
            .unwrap_or(SourceStatus::Unavailable),
        diagnostic: current.and_then(|reading| reading.diagnostic()),
        last_observation_at: current.map(|reading| reading.window.observed_at),
    };

    let login_failures = match window {
        Some(window) => LoginFailurePanel {
            total: event_metric(current, previous, Event::LoginFailed, quality),
            categories: cells(
                window,
                Event::LoginFailed,
                Property::ReasonCategory,
                8,
                quality,
            ),
            trend: event_trend(window, Event::LoginFailed, query, quality),
        },
        None => LoginFailurePanel {
            // An unavailable provider reports null, never a zero count.
            total: metric(None, None, true, quality),
            categories: Vec::new(),
            trend: Vec::new(),
        },
    };

    let frontend_errors = match window {
        Some(window) => FrontendErrorPanel {
            total: event_metric(current, previous, Event::FrontendError, quality),
            by_route_family: cells(
                window,
                Event::FrontendError,
                Property::RouteFamily,
                4,
                quality,
            ),
            by_category: cells(window, Event::FrontendError, Property::Category, 4, quality),
        },
        None => FrontendErrorPanel {
            total: metric(None, None, true, quality),
            by_route_family: Vec::new(),
            by_category: Vec::new(),
        },
    };

    let backend_unavailable = match window {
        Some(window) => BackendUnavailablePanel {
            total: event_metric(current, previous, Event::BackendUnavailable, quality),
            by_route_family: cells(
                window,
                Event::BackendUnavailable,
                Property::RouteFamily,
                4,
                quality,
            ),
        },
        None => BackendUnavailablePanel {
            total: metric(None, None, true, quality),
            by_route_family: Vec::new(),
        },
    };

    let navigation_latency = window
        .map(|window| navigation_latency(window, quality))
        .unwrap_or_default()
        .into_iter()
        .take(MAX_RANKED_ROWS)
        .collect();

    let audit_activity = audit.into_iter().take(MAX_AUDIT_ROWS).collect();

    ReliabilityResponse {
        collector,
        login_failures,
        frontend_errors,
        backend_unavailable,
        navigation_latency,
        audit_activity,
        meta,
    }
}

/// Derived: whether the caller asked for a period that has not finished. Used
/// only to keep `cohort_complete` honest, mirroring the marketplace rule.
pub fn cohort_complete(query: &AnalyticsQuery, now: DateTime<Utc>) -> bool {
    query.end <= now
}

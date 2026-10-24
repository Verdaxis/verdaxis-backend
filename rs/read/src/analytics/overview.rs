//! Product-analytics overview (P05).
//!
//! Composes the authoritative facts (`super::aggregates`) with the typed
//! behavioural window from the CF4_03 provider contract. Facts come from the
//! history tables only; nothing here reconstructs history from the current
//! `users` snapshot. Covered zeros stay `0`, uncovered or unavailable values
//! stay `null`, and 1/2-entity cells are suppressed.

use chrono::{DateTime, Utc};

use super::aggregates::{
    LoginFacts, OverviewFacts, QualityTracker, behavioral_available, behavioral_coverage,
    behavioral_daily_series, behavioral_metric, date_window, dense_series, fact_coverage,
};
use super::types::{
    ActivityTrend, AnalyticsCoverage, AnalyticsMeta, AnalyticsQuery, AnalyticsSourceCoverage,
    AnalyticsSourceStatus, BehavioralWindow, LifecycleStage, LifecycleStageKey, MarketplaceBalance,
    MetricValue, NeedsAttentionItem, NeedsAttentionRule, OverviewKpis, OverviewResponse,
    SeriesPoint,
};

const LIFECYCLE_LIMIT: usize = 6;
const ATTENTION_LIMIT: usize = 10;
/// Five or more failed logins in the period is an operational signal.
const LOGIN_FAILURE_ALERT: u64 = 5;

/// Active members exist only when the login-day facts cover the window.
/// Coverage is compared on the half-open date bounds, so a period whose lower
/// bound predates the first fact is unavailable even if it contains facts.
pub fn login_metric(login: &LoginFacts, query: &AnalyticsQuery) -> MetricValue {
    let current_bounds = date_window(Some(query.start), Some(query.end));
    let previous_bounds = date_window(query.previous_start(), query.previous_end());
    let covered = |bounds: Option<(chrono::NaiveDate, chrono::NaiveDate)>| matches!((login.coverage_start, bounds), (Some(coverage), Some((lower, _))) if lower >= coverage);
    MetricValue {
        value: covered(current_bounds).then_some(login.active_current),
        previous: covered(previous_bounds).then_some(login.active_previous),
        suppressed: false,
    }
}

/// Dense member trend: a genuine zero on covered dates and `null` before the
/// first login fact. Never inferred from `users.last_login`.
pub fn member_series(login: &LoginFacts, query: &AnalyticsQuery) -> Vec<SeriesPoint> {
    let Some(coverage) = login.coverage_start else {
        return Vec::new();
    };
    dense_series(query.start, query.end, &login.daily)
        .into_iter()
        .map(|point| SeriesPoint {
            date: point.date,
            value: (point.date >= coverage).then_some(point.value.unwrap_or(0)),
        })
        .collect()
}

pub fn overview(
    facts: &OverviewFacts,
    behavioral: Option<&BehavioralWindow>,
    behavioral_previous: Option<&BehavioralWindow>,
    query: &AnalyticsQuery,
    observed_at: DateTime<Utc>,
) -> OverviewResponse {
    let mut quality = QualityTracker::default();
    let live_orders = facts.orders("live");
    let live_trades = facts.trades("live");

    if let Some(trades) = live_trades {
        quality.legacy_timestamp_fallback_count = trades.legacy_current + trades.legacy_previous;
        quality.missing_paid_at_count = trades.missing_paid_at_current;
    }
    let confirmed_current = live_trades
        .map(|trades| trades.confirmed_current_strict + trades.legacy_current)
        .unwrap_or(0);
    let confirmed_previous = live_trades
        .map(|trades| trades.confirmed_previous_strict + trades.legacy_previous)
        .unwrap_or(0);

    let comparing = query.previous_end().is_some();
    let orders_current = live_orders.map_or(0, |row| row.orders_current);
    let orders_previous = live_orders.map_or(0, |row| row.orders_previous);
    let participating_current = live_orders.map_or(0, |row| row.orgs_current);
    let participating_previous = live_orders.map_or(0, |row| row.orgs_previous);
    let bids = live_orders.map_or(0, |row| row.bids_current);
    let asks = live_orders.map_or(0, |row| row.asks_current);

    let active_members = login_metric(&facts.login, query);
    let status_coverage = facts.status.coverage_start;
    // As-of qualification from the append-only transition history: available
    // only when the history already covers the as-of instant.
    let qualified = MetricValue {
        value: facts
            .status
            .qualified_end
            .filter(|_| status_coverage.is_some_and(|start| query.end >= start)),
        previous: facts.status.qualified_previous_end.filter(|_| {
            query
                .previous_end()
                .is_some_and(|end| status_coverage.is_some_and(|start| end >= start))
        }),
        suppressed: false,
    };

    let kpis = OverviewKpis {
        qualified_organizations: qualified,
        active_members,
        participating_organizations: MetricValue {
            value: Some(participating_current),
            previous: comparing.then_some(participating_previous),
            suppressed: false,
        },
        live_orders: MetricValue {
            value: Some(orders_current),
            previous: comparing.then_some(orders_previous),
            suppressed: false,
        },
        confirmed_trades: MetricValue {
            value: Some(confirmed_current),
            previous: comparing.then_some(confirmed_previous),
            suppressed: false,
        },
    };

    let visitors = behavioral_metric(behavioral, behavioral_previous, |core| core.visitors);
    let behavioral_status = if behavioral_available(behavioral) {
        AnalyticsSourceStatus::Available
    } else {
        AnalyticsSourceStatus::Unavailable
    };
    let login_coverage = fact_coverage(
        facts.login.coverage_start.map(super::aggregates::midnight),
        query,
        observed_at,
    );

    let lifecycle: Vec<LifecycleStage> = vec![
        LifecycleStage {
            key: LifecycleStageKey::Visitors,
            count: visitors.value,
            previous: visitors.previous,
            coverage: behavioral_status,
            detail_tab: "acquisition".into(),
        },
        LifecycleStage {
            key: LifecycleStageKey::Registered,
            count: Some(facts.registered.current()),
            previous: comparing.then_some(facts.registered.previous()),
            coverage: AnalyticsSourceStatus::Available,
            detail_tab: "activation".into(),
        },
        LifecycleStage {
            key: LifecycleStageKey::Active,
            count: active_members.value,
            previous: active_members.previous,
            coverage: login_coverage.status,
            detail_tab: "engagement".into(),
        },
        LifecycleStage {
            key: LifecycleStageKey::Participating,
            count: Some(participating_current),
            previous: comparing.then_some(participating_previous),
            coverage: AnalyticsSourceStatus::Available,
            detail_tab: "marketplace".into(),
        },
        LifecycleStage {
            key: LifecycleStageKey::Trading,
            count: Some(facts.activity.trading_current),
            previous: comparing.then_some(facts.activity.trading_previous),
            coverage: AnalyticsSourceStatus::Available,
            detail_tab: "marketplace".into(),
        },
        LifecycleStage {
            key: LifecycleStageKey::Retained,
            count: Some(facts.activity.retained_current),
            previous: comparing.then_some(facts.activity.retained_previous),
            coverage: AnalyticsSourceStatus::Available,
            detail_tab: "retention".into(),
        },
    ]
    .into_iter()
    .take(LIFECYCLE_LIMIT)
    .collect();

    let mut needs_attention = Vec::new();
    if facts.drop_off.never_logged_in > 0 {
        needs_attention.push(NeedsAttentionItem {
            rule: NeedsAttentionRule::ApprovedMembersNeverLoggedIn,
            count: Some(facts.drop_off.never_logged_in),
        });
    }
    let signup_started = raw_event_total(behavioral, "signup_started");
    let signup_submitted = raw_event_total(behavioral, "signup_submitted");
    // A drop is only asserted from answered provider data; an unavailable
    // provider is reported separately, never as a zero-submission drop.
    if signup_started.is_some_and(|started| started > 0) && signup_submitted == Some(0) {
        // A one-event drop is a real operational signal, but its count is a
        // derived single-event value, so it carries no number.
        needs_attention.push(NeedsAttentionItem {
            rule: NeedsAttentionRule::SignupSubmissionDrop,
            count: (signup_started.unwrap_or(0) >= 3).then_some(signup_started.unwrap_or(0)),
        });
    }
    // Both counts are order counts, so a one-sided market is visible without
    // exposing a small entity population.
    if (bids == 0) != (asks == 0) {
        needs_attention.push(NeedsAttentionItem {
            rule: NeedsAttentionRule::OneSidedLiveMarket,
            count: None,
        });
    }
    let login_failures = raw_event_total(behavioral, "login_failed");
    if login_failures.is_some_and(|failures| failures >= LOGIN_FAILURE_ALERT) {
        needs_attention.push(NeedsAttentionItem {
            rule: NeedsAttentionRule::ElevatedLoginFailures,
            count: login_failures,
        });
    }
    if behavioral_status != AnalyticsSourceStatus::Available {
        needs_attention.push(NeedsAttentionItem {
            rule: NeedsAttentionRule::DegradedAnalyticsCollection,
            count: None,
        });
    }
    needs_attention.truncate(ATTENTION_LIMIT);

    // Organization counts are entity cells, so they take the small-cell rule;
    // order counts stay visible.
    let buyer_organizations = quality.segmented_metric(live_orders.map(|row| row.bid_orgs_current));
    let supplier_organizations =
        quality.segmented_metric(live_orders.map(|row| row.ask_orgs_current));

    OverviewResponse {
        meta: AnalyticsMeta {
            start: query.start,
            end: query.end,
            previous_start: query.previous_start(),
            previous_end: query.previous_end(),
            observed_at,
            coverage: AnalyticsCoverage {
                authoritative: AnalyticsSourceCoverage {
                    coverage_start: status_coverage,
                    coverage_end: None,
                    observed_at: Some(observed_at),
                    status: AnalyticsSourceStatus::Available,
                    diagnostic: None,
                },
                behavioral: behavioral_coverage(behavioral),
                login_history: login_coverage,
                status_history: fact_coverage(status_coverage, query, observed_at),
                reference: AnalyticsSourceCoverage::not_applicable(),
            },
            data_quality: quality.to_schema(),
        },
        kpis,
        lifecycle,
        activity_trend: ActivityTrend {
            visitors: behavioral_daily_series(behavioral, query),
            active_members: member_series(&facts.login, query),
            orders: dense_series(query.start, query.end, &facts.daily_orders),
            confirmed_trades: dense_series(query.start, query.end, &facts.daily_trades),
        },
        marketplace_balance: MarketplaceBalance {
            buyer_organizations,
            supplier_organizations,
            bid_orders: MetricValue {
                value: Some(bids),
                previous: None,
                suppressed: false,
            },
            ask_orders: MetricValue {
                value: Some(asks),
                previous: None,
                suppressed: false,
            },
        },
        needs_attention,
    }
}

/// Raw event total for a business rule. `None` means the provider did not
/// answer; callers never substitute zero for it.
pub fn raw_event_total(window: Option<&BehavioralWindow>, event: &str) -> Option<u64> {
    let rows = window?.event_totals.data()?;
    Some(
        rows.iter()
            .filter(|row| row.event == event)
            .map(|row| row.count.count.unwrap_or(0))
            .sum(),
    )
}

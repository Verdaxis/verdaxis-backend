//! Product-analytics activation tab (P07).
//!
//! The behavioural signup journey comes from the typed provider window; the
//! registered/approved/first-login/first-order stages come from history facts.
//! Ratios never mix a behavioural numerator with an authoritative denominator
//! (`ConversionRatioKind::AggregateEvent` only), and a small cohort suppresses
//! its distribution rather than publishing a fabricated median.

use chrono::{DateTime, Utc};

use super::aggregates::{
    ActivationFacts, QualityTracker, behavioral_coverage, behavioral_event_cell, date_window,
    fact_coverage, median_hours,
};
use super::query::AnalyticsQuery;
use super::types::{
    ActivationResponse, AggregateCell, AnalyticsCoverage, AnalyticsMeta, AnalyticsSourceCoverage,
    AnalyticsSourceStatus, BehavioralWindow, ConversionRatioKind, DurationDistribution,
    JourneySource, JourneyStage, LabeledRatio, RatioValue,
};

/// Signup journey stages in the order the funnel presents them.
pub const SIGNUP_JOURNEY: [&str; 5] = [
    "signup_started",
    "signup_role_selected",
    "signup_submitted",
    "signup_organization_required",
    "signup_organization_submitted",
];

pub fn activation(
    facts: &ActivationFacts,
    behavioral: Option<&BehavioralWindow>,
    query: &AnalyticsQuery,
    observed_at: DateTime<Utc>,
) -> ActivationResponse {
    let mut quality = QualityTracker::default();

    let mut journey: Vec<JourneyStage> = SIGNUP_JOURNEY
        .iter()
        .map(|event| JourneyStage {
            key: (*event).to_owned(),
            source: JourneySource::Behavioral,
            total: behavioral_event_cell(behavioral, event),
            buyer: None,
            supplier: None,
        })
        .collect();

    // Role split is a segmented entity cell: two buyers suppress, a genuine
    // zero supplier count stays visible.
    let buyer = quality.cell("BUYER", Some(facts.registered.buyer_current), None);
    let supplier = quality.cell("SUPPLIER", Some(facts.registered.supplier_current), None);
    journey.push(JourneyStage {
        key: "registered".into(),
        source: JourneySource::Authoritative,
        total: AggregateCell {
            key: "registered".into(),
            count: Some(facts.registered.current()),
            suppressed: false,
        },
        buyer: Some(buyer),
        supplier: Some(supplier),
    });

    // The status fact is only usable once its history covers the period end.
    let status_available = facts
        .status
        .coverage_start
        .is_some_and(|start| start <= query.end);
    journey.push(JourneyStage {
        key: "approved".into(),
        source: JourneySource::Authoritative,
        total: quality.cell(
            "approved",
            status_available.then_some(facts.status.approved_current),
            None,
        ),
        buyer: None,
        supplier: None,
    });

    // First recorded logins inside the period, and the registration-to-login
    // duration of those members.
    let bounds = date_window(Some(query.start), Some(query.end));
    let mut first_login_durations = Vec::new();
    let mut login_coverage = None;
    for row in &facts.logins {
        login_coverage = Some(match login_coverage {
            Some(current) => std::cmp::min(current, row.first_login_date),
            None => row.first_login_date,
        });
        if bounds.is_some_and(|(lower, upper)| {
            row.first_login_date >= lower && row.first_login_date < upper
        }) {
            first_login_durations.push(row.first_login_at - row.registered_at);
        }
    }
    let first_login_count = first_login_durations.len() as u64;
    let first_login_cell = super::policy::suppress(Some(first_login_count), None);
    let first_login_suppressed = first_login_cell.suppressed;
    if first_login_suppressed {
        quality.suppressed_cell_count += 1;
    }
    journey.push(JourneyStage {
        key: "first_login".into(),
        source: JourneySource::Authoritative,
        total: AggregateCell {
            key: "first_login".into(),
            count: (!first_login_suppressed).then_some(first_login_count),
            suppressed: first_login_suppressed,
        },
        buyer: None,
        supplier: None,
    });

    // Organizations whose first-ever live order falls inside the period.
    let mut live_order_durations = Vec::new();
    for (first_order_at, organization_created_at) in &facts.first_live_orders {
        if *first_order_at >= query.start && *first_order_at < query.end {
            live_order_durations.push(*first_order_at - *organization_created_at);
        }
    }
    let live_order_count = live_order_durations.len() as u64;
    let live_order_cell = super::policy::suppress(Some(live_order_count), None);
    let live_order_suppressed = live_order_cell.suppressed;
    if live_order_suppressed {
        quality.suppressed_cell_count += 1;
    }
    journey.push(JourneyStage {
        key: "first_live_order".into(),
        source: JourneySource::Authoritative,
        total: AggregateCell {
            key: "first_live_order".into(),
            count: (!live_order_suppressed).then_some(live_order_count),
            suppressed: live_order_suppressed,
        },
        buyer: None,
        supplier: None,
    });

    let drop_off = vec![
        quality.cell("rejected", Some(facts.drop_off.rejected), None),
        quality.cell("unverified", Some(facts.drop_off.unverified), None),
        quality.cell(
            "pending_approval",
            Some(facts.drop_off.pending_approval),
            None,
        ),
        quality.cell(
            "organization_incomplete",
            Some(facts.drop_off.organization_incomplete),
            None,
        ),
        quality.cell(
            "never_logged_in",
            Some(facts.drop_off.never_logged_in),
            None,
        ),
    ];

    let status_coverage = facts.status.coverage_start;
    ActivationResponse {
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
                login_history: fact_coverage(
                    login_coverage.map(super::aggregates::midnight),
                    query,
                    observed_at,
                ),
                status_history: fact_coverage(status_coverage, query, observed_at),
                reference: AnalyticsSourceCoverage::not_applicable(),
            },
            data_quality: quality.to_schema(),
        },
        journey,
        time_to_first_login: DurationDistribution {
            buckets: Vec::new(),
            median_hours: (!first_login_suppressed)
                .then(|| median_hours(&first_login_durations))
                .flatten(),
            sample_size: (!first_login_suppressed)
                .then_some(first_login_count)
                .filter(|count| *count > 0),
            suppressed: first_login_suppressed,
        },
        time_to_first_live_order: DurationDistribution {
            buckets: Vec::new(),
            median_hours: (!live_order_suppressed)
                .then(|| median_hours(&live_order_durations))
                .flatten(),
            sample_size: (!live_order_suppressed)
                .then_some(live_order_count)
                .filter(|count| *count > 0),
            suppressed: live_order_suppressed,
        },
        drop_off,
        ratios: vec![aggregate_ratio(
            "signup_started_to_submitted",
            super::overview::raw_event_total(behavioral, "signup_submitted"),
            super::overview::raw_event_total(behavioral, "signup_started"),
        )],
    }
}

/// Aggregate-event ratio: both sides share the behavioural event basis.
///
/// A 1 or 2 on either side is a small cell, and a rate derived from it would
/// publish that suppressed value. The whole ratio is then suppressed with no
/// component emitted. A non-positive or absent denominator leaves `rate_pct`
/// null without suppressing a visible zero.
pub fn aggregate_ratio(
    key: &str,
    numerator: Option<u64>,
    denominator: Option<u64>,
) -> LabeledRatio {
    let small = |value| super::policy::suppress(value, None).suppressed;
    if small(numerator) || small(denominator) {
        return LabeledRatio {
            key: key.to_owned(),
            kind: ConversionRatioKind::AggregateEvent,
            ratio: RatioValue {
                numerator: None,
                denominator: None,
                rate_pct: None,
                suppressed: true,
                cohort_complete: true,
            },
        };
    }
    LabeledRatio {
        key: key.to_owned(),
        kind: ConversionRatioKind::AggregateEvent,
        ratio: super::policy::ratio_from_counts(numerator, denominator, true),
    }
}

//! Product-analytics engagement tab (P08).
//!
//! DAU/WAU/MAU are rolling as-of counts from the login-day facts and are
//! coverage-gated per rolling length; the trend is dense and never precedes
//! the first fact. Feature adoption and workflow ratios come only from the
//! typed behavioural window, with small event totals suppressed.

use chrono::{DateTime, TimeDelta, Utc};

use super::aggregates::{
    EngagementFacts, QualityTracker, behavioral_coverage, behavioral_event_cell,
    behavioral_property_rows, date_window, fact_coverage, rate_pct,
};
use super::types::{
    AggregateCell, AnalyticsCoverage, AnalyticsMeta, AnalyticsQuery, AnalyticsSourceCoverage,
    AnalyticsSourceStatus, BehavioralWindow, EngagementKpis, EngagementResponse,
    FeatureAdoptionRow, LabeledRatio, MetricValue, NavigationDestination, NavigationDestinationRow,
    TutorialStepRow,
};

/// Registered feature families, in the order the dashboard shows them until
/// the event totals sort them descending.
pub const FEATURE_FAMILIES: [&str; 9] = [
    "platform_navigation",
    "market_slice_selected",
    "listing_opened",
    "order_form_opened",
    "order_form_submitted",
    "tutorial_started",
    "tutorial_completed",
    "estimator_opened",
    "estimator_completed",
];

/// Navigation destinations the response schema accepts.
pub const DESTINATIONS: [(&str, NavigationDestination); 12] = [
    ("home", NavigationDestination::Home),
    ("map", NavigationDestination::Map),
    ("marketplace", NavigationDestination::Marketplace),
    ("curve", NavigationDestination::Curve),
    ("watchlist", NavigationDestination::Watchlist),
    ("analytics", NavigationDestination::Analytics),
    ("trades", NavigationDestination::Trades),
    ("quotes", NavigationDestination::Quotes),
    ("compliance", NavigationDestination::Compliance),
    ("training", NavigationDestination::Training),
    ("settings", NavigationDestination::Settings),
    ("admin", NavigationDestination::Admin),
];

const DESTINATION_LIMIT: usize = 12;
const TUTORIAL_STEP_LIMIT: usize = 20;

pub fn engagement(
    facts: &EngagementFacts,
    behavioral: Option<&BehavioralWindow>,
    query: &AnalyticsQuery,
    observed_at: DateTime<Utc>,
) -> EngagementResponse {
    let mut quality = QualityTracker::default();
    let login = &facts.login;
    let bounds = date_window(Some(query.start), Some(query.end));
    let upper = bounds.map(|(_, upper)| upper);

    // Each rolling window needs login facts far enough back to cover it.
    let rolling = |value: u64, days: i64| -> MetricValue {
        let covered = matches!(
            (login.coverage_start, upper),
            (Some(coverage), Some(upper)) if upper - TimeDelta::days(days) >= coverage
        );
        MetricValue {
            value: covered.then_some(value),
            previous: None,
            suppressed: false,
        }
    };
    let dau = rolling(login.dau, 1);
    let wau = rolling(login.wau, 7);
    let mau = rolling(login.mau, 30);
    let stickiness = match (dau.value, mau.value) {
        (Some(dau), Some(mau)) if mau > 0 => rate_pct(dau, mau),
        _ => None,
    };

    let mut adoption: Vec<FeatureAdoptionRow> = FEATURE_FAMILIES
        .iter()
        .map(|family| {
            let cell = behavioral_event_cell(behavioral, family);
            if cell.suppressed {
                quality.suppressed_cell_count += 1;
            }
            FeatureAdoptionRow {
                family: (*family).to_owned(),
                events: cell.count,
                suppressed: cell.suppressed,
            }
        })
        .collect();
    // Descending by value; a suppressed or unavailable row sorts last but is
    // never rewritten to zero.
    adoption.sort_by_key(|row| std::cmp::Reverse(row.events.unwrap_or(0)));

    let ratios = vec![
        workflow_ratio(
            "listing_to_order_form",
            behavioral,
            "order_form_opened",
            "listing_opened",
        ),
        workflow_ratio(
            "order_form_to_submit",
            behavioral,
            "order_form_submitted",
            "order_form_opened",
        ),
        workflow_ratio(
            "tutorial_start_to_complete",
            behavioral,
            "tutorial_completed",
            "tutorial_started",
        ),
        workflow_ratio(
            "estimator_open_to_complete",
            behavioral,
            "estimator_completed",
            "estimator_opened",
        ),
    ];

    // A destination row exists only for a value the closed domain accepts;
    // `buyer`/`supplier` splits need a cross-tab the property API does not
    // provide, so they stay `null`.
    let destinations: Vec<NavigationDestinationRow> =
        behavioral_property_rows(behavioral, "platform_navigation", "destination")
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| {
                        let (_, destination) = DESTINATIONS
                            .iter()
                            .find(|(value, _)| *value == row.value.as_str())?;
                        let cell = super::types::suppress(row.count.count, None);
                        Some(NavigationDestinationRow {
                            destination: *destination,
                            total: AggregateCell {
                                key: row.value.clone(),
                                ..cell
                            },
                            buyer: None,
                            supplier: None,
                        })
                    })
                    .take(DESTINATION_LIMIT)
                    .collect()
            })
            .unwrap_or_default();

    let mut steps: Vec<(String, Option<u64>, Option<u64>)> = Vec::new();
    for (event, index) in [("tutorial_step_completed", 1), ("tutorial_step_skipped", 2)] {
        for row in behavioral_property_rows(behavioral, event, "step").unwrap_or_default() {
            match steps.iter_mut().find(|(step, _, _)| *step == row.value) {
                Some(entry) => {
                    if index == 1 {
                        entry.1 = Some(row.count.count.unwrap_or(0));
                    } else {
                        entry.2 = Some(row.count.count.unwrap_or(0));
                    }
                }
                None => {
                    let completed = (index == 1).then_some(row.count.count.unwrap_or(0));
                    let skipped = (index == 2).then_some(row.count.count.unwrap_or(0));
                    steps.push((row.value.clone(), completed, skipped));
                }
            }
        }
    }
    steps.sort_by(|left, right| left.0.cmp(&right.0));
    let tutorial_steps: Vec<TutorialStepRow> = steps
        .into_iter()
        .take(TUTORIAL_STEP_LIMIT)
        .map(|(step, completed, skipped)| TutorialStepRow {
            completed: quality.cell("completed", completed, None),
            skipped: quality.cell("skipped", skipped, None),
            step,
        })
        .collect();

    EngagementResponse {
        meta: AnalyticsMeta {
            start: query.start,
            end: query.end,
            previous_start: query.previous_start(),
            previous_end: query.previous_end(),
            observed_at,
            coverage: AnalyticsCoverage {
                authoritative: AnalyticsSourceCoverage {
                    coverage_start: None,
                    coverage_end: None,
                    observed_at: Some(observed_at),
                    status: AnalyticsSourceStatus::Available,
                    diagnostic: None,
                },
                behavioral: behavioral_coverage(behavioral),
                login_history: fact_coverage(
                    login.coverage_start.map(super::aggregates::midnight),
                    query,
                    observed_at,
                ),
                status_history: AnalyticsSourceCoverage::not_applicable(),
                reference: AnalyticsSourceCoverage::not_applicable(),
            },
            data_quality: quality.to_schema(),
        },
        kpis: EngagementKpis {
            dau,
            wau,
            mau,
            stickiness_pct: stickiness,
        },
        active_members_trend: super::overview::member_series(login, query),
        feature_adoption: adoption,
        workflow_ratios: ratios,
        navigation_destinations: destinations,
        tutorial_steps,
    }
}

fn workflow_ratio(
    key: &str,
    behavioral: Option<&BehavioralWindow>,
    numerator_event: &str,
    denominator_event: &str,
) -> LabeledRatio {
    super::activation::aggregate_ratio(
        key,
        super::overview::raw_event_total(behavioral, numerator_event),
        super::overview::raw_event_total(behavioral, denominator_event),
    )
}

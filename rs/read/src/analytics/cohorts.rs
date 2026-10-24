//! Product-analytics retention tab (P10).
//!
//! Weekly cohorts are Monday-anchored (ISO weeks) and start at the first day
//! the member login facts have a row for that member — history never predates
//! coverage. Organization cohorts use live order/trade activity dates, which
//! may legitimately predate login coverage. Every cohort size and cell takes
//! the small-cell rule, and a percentage renders only when both the cell and
//! its cohort size are unsuppressed.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, NaiveDate, Utc};

use super::aggregates::{
    MAX_COHORTS, QualityTracker, RetentionFacts, behavioral_coverage, date_window, fact_coverage,
    rate_pct, week_start,
};
use super::types::{
    AnalyticsCoverage, AnalyticsMeta, AnalyticsQuery, AnalyticsSourceCoverage,
    AnalyticsSourceStatus, CohortCell, CohortRow, MetricValue, RetentionKpis, RetentionResponse,
};

/// Cohort cells keep the first 54 weekly offsets (53 ISO weeks plus one).
const COHORT_CELL_LIMIT: usize = MAX_COHORTS;

pub fn retention(
    facts: &RetentionFacts,
    _behavioral: Option<&super::types::BehavioralWindow>,
    query: &AnalyticsQuery,
    observed_at: DateTime<Utc>,
) -> RetentionResponse {
    let mut quality = QualityTracker::default();

    // Returning members need login coverage over the comparison period too;
    // without it the count is unavailable, not zero.
    let mut login_coverage: Option<NaiveDate> = None;
    let mut returning = 0_u64;
    for row in &facts.rollup {
        login_coverage = Some(match login_coverage {
            Some(current) => current.min(row.first_login_date),
            None => row.first_login_date,
        });
        if row.in_current && row.in_previous {
            returning += 1;
        }
    }
    let previous_bounds = date_window(query.previous_start(), query.previous_end());
    let returning_covered = matches!(
        (login_coverage, previous_bounds),
        (Some(coverage), Some((lower, _))) if lower >= coverage
    );

    let repeat_participation = vec![
        quality.cell("1", Some(facts.activity.active_days_1), None),
        quality.cell("2", Some(facts.activity.active_days_2), None),
        quality.cell("3_plus", Some(facts.activity.active_days_3_plus), None),
    ];

    RetentionResponse {
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
                behavioral: behavioral_coverage(_behavioral),
                login_history: fact_coverage(
                    login_coverage.map(super::aggregates::midnight),
                    query,
                    observed_at,
                ),
                status_history: AnalyticsSourceCoverage::not_applicable(),
                reference: AnalyticsSourceCoverage::not_applicable(),
            },
            data_quality: quality.to_schema(),
        },
        kpis: RetentionKpis {
            returning_members: MetricValue {
                value: returning_covered.then_some(returning),
                previous: None,
                suppressed: false,
            },
            retained_organizations: MetricValue {
                value: Some(facts.activity.retained_current),
                previous: query
                    .previous_end()
                    .map(|_| facts.activity.retained_previous),
                suppressed: false,
            },
            reactivated_organizations: MetricValue {
                value: Some(facts.activity.reactivated_current),
                previous: query
                    .previous_end()
                    .map(|_| facts.activity.reactivated_previous),
                suppressed: false,
            },
            dormant_approved_members: MetricValue {
                value: Some(facts.drop_off.never_logged_in),
                previous: None,
                suppressed: false,
            },
        },
        member_cohorts: cohorts(&facts.member_days, &mut quality),
        organization_cohorts: cohorts(&facts.organization_days, &mut quality),
        repeat_participation,
    }
}

/// Weekly cohorts from (entity, activity date) pairs.
///
/// A cohort is the Monday-anchored week of an entity's first activity date;
/// each cell counts cohort members active in the week at that offset. Rows are
/// ordered by cohort start and capped, as Python caps the response.
pub fn cohorts(days: &[(uuid::Uuid, NaiveDate)], quality: &mut QualityTracker) -> Vec<CohortRow> {
    let mut weeks_per_entity: BTreeMap<uuid::Uuid, BTreeSet<NaiveDate>> = BTreeMap::new();
    for (entity, date) in days {
        weeks_per_entity
            .entry(*entity)
            .or_default()
            .insert(week_start(*date));
    }
    let mut sizes: BTreeMap<NaiveDate, u64> = BTreeMap::new();
    let mut buckets: BTreeMap<NaiveDate, BTreeMap<i64, u64>> = BTreeMap::new();
    for weeks in weeks_per_entity.values() {
        let Some(first) = weeks.iter().next().copied() else {
            continue;
        };
        *sizes.entry(first).or_insert(0) += 1;
        let cohort = buckets.entry(first).or_default();
        for week in weeks {
            let offset = (week.signed_duration_since(first).num_days()) / 7;
            *cohort.entry(offset).or_insert(0) += 1;
        }
    }

    buckets
        .into_iter()
        .take(MAX_COHORTS)
        .map(|(cohort_start, offsets)| {
            let size_cell = quality.cell(
                &cohort_start.to_string(),
                sizes.get(&cohort_start).copied(),
                None,
            );
            let size_value = size_cell.count;
            let cells = offsets
                .into_iter()
                .take(COHORT_CELL_LIMIT)
                .map(|(offset, value)| {
                    let cell = quality.cell(&offset.to_string(), Some(value), None);
                    // Percentages render only from an unsuppressed numerator
                    // and an unsuppressed cohort size.
                    let pct = match (cell.count, size_value) {
                        (Some(value), Some(size)) if size > 0 => rate_pct(value, size),
                        _ => None,
                    };
                    CohortCell { offset, cell, pct }
                })
                .collect();
            CohortRow {
                cohort_start,
                size: size_cell,
                cells,
            }
        })
        .collect()
}

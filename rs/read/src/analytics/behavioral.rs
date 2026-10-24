//! Typed behavioral window aggregates (CF4_03 `BehavioralWindow`).
//!
//! An optional provider component is either available with typed data, or
//! unavailable/not requested with `data = None`. It is never a fabricated zero
//! or an empty list. `Partial` keeps only the components that resolved.
//!
//! Every collection is bounded: at most 366 daily points, 20 ranking rows and
//! 500 property rows per requested event.

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::Serialize;

use crate::analytics::{
    events::{AllowedProperty, AllowedPropertyValue, EntryPage, ReferrerCategory, RegisteredEvent},
    types::{
        AggregateCell, AnalyticsDiagnostic, AnalyticsSourceCoverage, AnalyticsSourceStatus,
        ComponentStatus, ProviderComponent,
    },
};

/// The window-level status the routes map onto `AnalyticsSourceStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BehavioralAvailability {
    Available,
    Partial,
    Unavailable,
}

impl BehavioralAvailability {
    pub fn source_status(self) -> AnalyticsSourceStatus {
        match self {
            Self::Available => AnalyticsSourceStatus::Available,
            Self::Partial => AnalyticsSourceStatus::Partial,
            Self::Unavailable => AnalyticsSourceStatus::Unavailable,
        }
    }
}

/// Core visit metrics. `average_session_duration_seconds` is present only
/// when visits are positive and both inputs are available.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CoreMetrics {
    pub visitors: Option<u64>,
    pub visits: Option<u64>,
    pub pageviews: Option<u64>,
    pub total_time_seconds: Option<u64>,
    pub average_session_duration_seconds: Option<Decimal>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AllowedEventAggregate {
    pub event: RegisteredEvent,
    pub count: AggregateCell,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DailyVisitorRow {
    pub date: NaiveDate,
    pub visitors: AggregateCell,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DailyEventRow {
    pub date: NaiveDate,
    pub event: RegisteredEvent,
    pub count: AggregateCell,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EntryRankingRow {
    pub entry: EntryPage,
    pub count: AggregateCell,
    pub rank: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReferrerRankingRow {
    pub referrer: ReferrerCategory,
    pub count: AggregateCell,
    pub rank: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AllowedPropertyAggregate {
    pub event: RegisteredEvent,
    pub property: AllowedProperty,
    pub value: AllowedPropertyValue,
    pub count: AggregateCell,
}

/// One requested (event, property) pair and its own result. Every requested
/// pair has exactly one status; a failed event endpoint marks every requested
/// property of that event unavailable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PropertyAvailability {
    pub event: RegisteredEvent,
    pub property: AllowedProperty,
    pub result: ProviderComponent<Vec<AllowedPropertyAggregate>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BehavioralWindow {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub observed_at: DateTime<Utc>,
    pub availability: BehavioralAvailability,
    pub diagnostic: Option<AnalyticsDiagnostic>,
    pub coverage: AnalyticsSourceCoverage,
    pub core: ProviderComponent<CoreMetrics>,
    pub event_totals: ProviderComponent<Vec<AllowedEventAggregate>>,
    pub daily_visitors: ProviderComponent<Vec<DailyVisitorRow>>,
    pub event_series: ProviderComponent<Vec<DailyEventRow>>,
    pub top_entries: ProviderComponent<Vec<EntryRankingRow>>,
    pub top_referrers: ProviderComponent<Vec<ReferrerRankingRow>>,
    pub property_results: Vec<PropertyAvailability>,
}

impl BehavioralWindow {
    /// A window where nothing resolved. Every component is unavailable with
    /// null data; the diagnostic is carried once at window level and on each
    /// component so a consumer never has to infer it.
    pub fn unavailable(
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        observed_at: DateTime<Utc>,
        diagnostic: AnalyticsDiagnostic,
        property_results: Vec<PropertyAvailability>,
    ) -> Self {
        Self {
            start,
            end,
            observed_at,
            availability: BehavioralAvailability::Unavailable,
            diagnostic: Some(diagnostic),
            coverage: coverage(
                start,
                end,
                observed_at,
                BehavioralAvailability::Unavailable,
                Some(diagnostic),
            ),
            core: unavailable_component(diagnostic),
            event_totals: unavailable_component(diagnostic),
            daily_visitors: unavailable_component(diagnostic),
            event_series: unavailable_component(diagnostic),
            top_entries: unavailable_component(diagnostic),
            top_referrers: unavailable_component(diagnostic),
            property_results,
        }
    }
}

fn coverage(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    observed_at: DateTime<Utc>,
    availability: BehavioralAvailability,
    diagnostic: Option<AnalyticsDiagnostic>,
) -> AnalyticsSourceCoverage {
    AnalyticsSourceCoverage {
        coverage_start: Some(start),
        coverage_end: Some(end),
        observed_at: Some(observed_at),
        status: availability.source_status(),
        diagnostic,
    }
}

fn unavailable_component<T>(diagnostic: AnalyticsDiagnostic) -> ProviderComponent<T> {
    ProviderComponent {
        status: ComponentStatus::Unavailable,
        diagnostic: Some(diagnostic),
        data: None,
    }
}

fn available_component<T>(data: T) -> ProviderComponent<T> {
    ProviderComponent {
        status: ComponentStatus::Available,
        diagnostic: None,
        data: Some(data),
    }
}

/// The core aggregator. It receives already-decoded rows, never a raw body,
/// and only applies ordering, window filtering and suppression.
pub(crate) struct WindowRows {
    pub stats: CoreMetrics,
    pub sessions: Vec<(NaiveDate, u64)>,
    pub events: Vec<(NaiveDate, RegisteredEvent, u64)>,
    pub entries: Vec<(EntryPage, u64)>,
    pub referrers: Vec<(ReferrerCategory, u64)>,
}

/// Maximum daily points in one series.
pub const MAX_SERIES_POINTS: usize = 366;
/// Maximum ranking rows per ranking.
pub const MAX_RANK_ROWS: usize = 20;
/// Maximum property rows per requested event.
pub const MAX_PROPERTY_ROWS: usize = 500;

pub(crate) fn assemble(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    observed_at: DateTime<Utc>,
    rows: WindowRows,
    property_results: Vec<PropertyAvailability>,
    property_failures: usize,
) -> BehavioralWindow {
    let (availability, diagnostic) = if property_failures > 0 {
        (
            BehavioralAvailability::Partial,
            property_results.iter().find_map(|entry| {
                if entry.result.status == ComponentStatus::Unavailable {
                    entry.result.diagnostic
                } else {
                    None
                }
            }),
        )
    } else {
        (BehavioralAvailability::Available, None)
    };
    let window_start = start.date_naive();
    let window_end = end.date_naive();

    let mut daily: Vec<(NaiveDate, u64)> = rows
        .sessions
        .into_iter()
        .filter(|(date, _)| *date >= window_start && *date < window_end)
        .collect();
    daily.sort_by_key(|(date, _)| *date);
    let mut merged_daily: Vec<(NaiveDate, u64)> = Vec::new();
    for (date, count) in daily {
        match merged_daily.last_mut() {
            Some((last, total)) if *last == date => *total += count,
            _ => merged_daily.push((date, count)),
        }
    }
    merged_daily.truncate(MAX_SERIES_POINTS);
    let daily_visitors = merged_daily
        .into_iter()
        .map(|(date, count)| DailyVisitorRow {
            date,
            visitors: cell(date.to_string(), count),
        })
        .collect::<Vec<_>>();

    let mut event_totals: Vec<(RegisteredEvent, u64)> = Vec::new();
    let mut event_points: Vec<(NaiveDate, RegisteredEvent, u64)> = Vec::new();
    for (date, event, count) in rows.events {
        match event_totals
            .iter_mut()
            .find(|(candidate, _)| *candidate == event)
        {
            Some((_, total)) => *total += count,
            None => event_totals.push((event, count)),
        }
        if date >= window_start && date < window_end {
            match event_points
                .iter_mut()
                .find(|(candidate_date, candidate, _)| {
                    *candidate_date == date && *candidate == event
                }) {
                Some((_, _, total)) => *total += count,
                None => event_points.push((date, event, count)),
            }
        }
    }
    event_points.sort_by_key(|row| (row.0, row.1));
    event_points.truncate(MAX_SERIES_POINTS);
    let event_series = event_points
        .into_iter()
        .map(|(date, event, count)| DailyEventRow {
            date,
            event,
            count: cell(event.name().to_owned(), count),
        })
        .collect::<Vec<_>>();

    let event_totals = event_totals
        .into_iter()
        .map(|(event, count)| AllowedEventAggregate {
            event,
            count: cell(event.name().to_owned(), count),
        })
        .collect::<Vec<_>>();

    let top_entries = ranked(rows.entries, |entry| entry.as_str())
        .into_iter()
        .enumerate()
        .map(|(index, (entry, count))| EntryRankingRow {
            entry,
            count: cell(entry.as_str().to_owned(), count),
            rank: u8::try_from(index + 1).unwrap_or(u8::MAX),
        })
        .collect::<Vec<_>>();
    let top_referrers = ranked(rows.referrers, |referrer| referrer.as_str())
        .into_iter()
        .enumerate()
        .map(|(index, (referrer, count))| ReferrerRankingRow {
            referrer,
            count: cell(referrer.as_str().to_owned(), count),
            rank: u8::try_from(index + 1).unwrap_or(u8::MAX),
        })
        .collect::<Vec<_>>();

    BehavioralWindow {
        start,
        end,
        observed_at,
        availability,
        diagnostic,
        coverage: coverage(start, end, observed_at, availability, diagnostic),
        core: available_component(rows.stats),
        event_totals: available_component(event_totals),
        daily_visitors: available_component(daily_visitors),
        event_series: available_component(event_series),
        top_entries: available_component(top_entries),
        top_referrers: available_component(top_referrers),
        property_results,
    }
}

/// Aggregate duplicate labels, then order by count descending and label
/// ascending, and keep at most twenty rows.
fn ranked<T: Copy + Ord>(rows: Vec<(T, u64)>, label: impl Fn(T) -> &'static str) -> Vec<(T, u64)> {
    let mut merged: Vec<(T, u64)> = Vec::new();
    for (key, count) in rows {
        match merged.iter_mut().find(|(candidate, _)| *candidate == key) {
            Some((_, total)) => *total += count,
            None => merged.push((key, count)),
        }
    }
    merged.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| label(left.0).cmp(label(right.0)))
    });
    merged.truncate(MAX_RANK_ROWS);
    merged
}

fn cell(key: String, count: u64) -> AggregateCell {
    let mut cell = crate::analytics::policy::suppress(Some(count), None);
    cell.key = key;
    cell
}

//! Typed behavioral window aggregates (CF4_03 `BehavioralWindow`).
//!
//! An optional provider component is either available with typed data, or
//! unavailable/not requested with `data = None`. It is never a fabricated zero
//! or an empty list. `Partial` keeps only the components that resolved.
//!
//! Every collection is bounded: at most 366 daily points, 20 ranking rows and
//! 500 property rows per requested event.

use chrono::{DateTime, NaiveDate, Utc};

pub use crate::analytics::types::{
    AllowedEventAggregate, AllowedPropertyAggregate, BehavioralAvailability, BehavioralWindow,
    CoreMetrics, DailyEventRow, DailyVisitorRow, EntryPage, EntryRankingRow, PropertyAvailability,
    ReferrerCategory, ReferrerRankingRow,
};
use crate::analytics::{
    events::RegisteredEvent,
    types::{
        AggregateCell, AnalyticsDiagnostic, AnalyticsSourceCoverage, AnalyticsSourceStatus,
        ComponentStatus, ProviderComponent,
    },
};

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
        status: match availability {
            BehavioralAvailability::Available => AnalyticsSourceStatus::Available,
            BehavioralAvailability::Partial => AnalyticsSourceStatus::Partial,
            BehavioralAvailability::Unavailable => AnalyticsSourceStatus::Unavailable,
        },
        diagnostic,
    }
}

/// The core aggregator. It receives already-decoded rows, never a raw body,
/// and only applies ordering, window filtering and suppression.
pub(crate) struct WindowRows {
    pub stats: CoreMetrics,
    pub sessions: Vec<(DateTime<Utc>, u64)>,
    pub events: Vec<(DateTime<Utc>, RegisteredEvent, u64)>,
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
                if entry.result.status() == ComponentStatus::Unavailable {
                    entry.result.diagnostic()
                } else {
                    None
                }
            }),
        )
    } else {
        (BehavioralAvailability::Available, None)
    };

    let mut daily: Vec<(NaiveDate, u64)> = rows
        .sessions
        .into_iter()
        .filter(|(instant, _)| *instant >= start && *instant < end)
        .map(|(instant, count)| (instant.date_naive(), count))
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
    for (instant, event, count) in rows.events {
        let date = instant.date_naive();
        match event_totals
            .iter_mut()
            .find(|(candidate, _)| *candidate == event)
        {
            Some((_, total)) => *total += count,
            None => event_totals.push((event, count)),
        }
        if instant >= start && instant < end {
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
    let mut points_per_event = std::collections::BTreeMap::new();
    event_points.retain(|(_, event, _)| {
        let retained = points_per_event.entry(*event).or_insert(0);
        *retained += 1;
        *retained <= MAX_SERIES_POINTS
    });
    let event_series = event_points
        .into_iter()
        .map(|(date, event, count)| DailyEventRow {
            date,
            event: event.name().to_owned(),
            count: cell(event.name().to_owned(), count),
        })
        .collect::<Vec<_>>();

    let event_totals = event_totals
        .into_iter()
        .map(|(event, count)| AllowedEventAggregate {
            event: event.name().to_owned(),
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
        core: ProviderComponent::available(rows.stats),
        event_totals: if property_failures > 0 {
            ProviderComponent::partial(Some(event_totals), diagnostic)
        } else {
            ProviderComponent::available(event_totals)
        },
        daily_visitors: ProviderComponent::available(daily_visitors),
        event_series: ProviderComponent::available(event_series),
        top_entries: ProviderComponent::available(top_entries),
        top_referrers: ProviderComponent::available(top_referrers),
        property_results,
    }
}

/// Aggregate duplicate labels, then order by count descending and label
/// ascending, and keep at most twenty rows.
fn ranked<T: Copy + Eq>(rows: Vec<(T, u64)>, label: impl Fn(T) -> &'static str) -> Vec<(T, u64)> {
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

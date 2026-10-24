//! Acquisition tab (P06): behavioral traffic, referrers, entry pages, CTA
//! breakdowns and the calculator funnel.
//!
//! Every value here comes from the behavioral provider window; this tab owns no
//! database aggregate. The composition rules are the CF4_03 ones:
//!
//! - A failed or disabled provider leaves every behavioral value `null` and the
//!   tab still answers 200. It never fabricates a zero.
//! - Aggregate duplicate allowed labels before suppression; suppress cells
//!   backed by one or two entities/events, including totals, series and ranks.
//! - Rankings are bounded to twenty rows; CTA and language rows to ten and
//!   five. Only closed value domains reach the response, never raw text.
//! - The property breakdown the provider cannot supply (a CTA × placement
//!   cross-tab) is not invented: each dimension reports its own rows.

use std::collections::BTreeMap;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Serialize;

use super::marketplace::{
    AggregateCell, AllowedPropertySet, AnalyticsQuery, BehavioralWindow, ComponentStatus,
    DecimalMetricValue, Diagnostic, Event, MAX_RANKED_ROWS, Meta, MetricValue, Property, Quality,
    RankedRow, SeriesPoint, SourceStatus, cell, daily_series, metric, one_decimal, share_pct,
    suppress_cell, wire,
};

/// Behavioral events this tab requests property breakdowns for.
pub fn requested_properties() -> AllowedPropertySet {
    AllowedPropertySet::new([
        (Event::LandingCtaClicked, Property::Cta),
        (Event::LandingCtaClicked, Property::Placement),
        (Event::LandingCtaClicked, Property::Language),
    ])
    .expect("the acquisition property set is static and valid")
}

#[derive(Clone, Debug, Serialize)]
pub struct AcquisitionKpis {
    pub visitors: MetricValue,
    pub visits: MetricValue,
    pub pageviews: MetricValue,
    pub average_session_duration_seconds: DecimalMetricValue,
    pub cta_clicks: MetricValue,
}

#[derive(Clone, Debug, Serialize)]
pub struct CalculatorFunnel {
    pub starts: MetricValue,
    pub completions: MetricValue,
}

#[derive(Clone, Debug, Serialize)]
pub struct CtaMatrixRow {
    pub cta: String,
    pub placement: String,
    pub clicks: Option<i64>,
    #[serde(serialize_with = "wire::decimal_option")]
    pub share_pct: Option<Decimal>,
    pub suppressed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct AcquisitionResponse {
    pub meta: Meta,
    pub kpis: AcquisitionKpis,
    pub visitors_trend: Vec<SeriesPoint>,
    pub previous_visitors_trend: Vec<SeriesPoint>,
    pub visits_trend: Vec<SeriesPoint>,
    pub referrers: Vec<RankedRow>,
    pub entry_pages: Vec<RankedRow>,
    pub cta_matrix: Vec<CtaMatrixRow>,
    pub languages: Vec<RankedRow>,
    pub calculator: CalculatorFunnel,
}

/// A behavioral reading is either a typed window or an unavailable component
/// carrying the diagnostic that ended it.
#[derive(Clone, Debug)]
pub struct Reading {
    pub window: BehavioralWindow,
}

impl Reading {
    pub fn unavailable(diagnostic: Diagnostic, observed_at: chrono::DateTime<chrono::Utc>) -> Self {
        Self {
            window: BehavioralWindow::unavailable(diagnostic, observed_at),
        }
    }

    pub fn status(&self) -> SourceStatus {
        self.window.status()
    }

    fn available(&self) -> bool {
        self.window.status() != SourceStatus::Unavailable
    }

    /// The diagnostic that a degraded window reported, never a default.
    pub fn diagnostic(&self) -> Option<Diagnostic> {
        self.window.diagnostic()
    }
}

fn read_field(
    reading: Option<&Reading>,
    field: impl Fn(&BehavioralWindow) -> Option<i64>,
) -> Option<i64> {
    let reading = reading?;
    if !reading.available() {
        return None;
    }
    field(&reading.window)
}

/// A headline behavioral metric. These are event/visit totals, so the
/// event-count suppression rule applies to the current and previous values.
pub(super) fn int_metric(
    current: Option<&Reading>,
    previous: Option<&Reading>,
    field: impl Fn(&BehavioralWindow) -> Option<i64>,
    quality: &mut Quality,
) -> MetricValue {
    metric(
        read_field(current, &field),
        read_field(previous, &field),
        true,
        quality,
    )
}

pub(super) fn event_metric(
    current: Option<&Reading>,
    previous: Option<&Reading>,
    event: Event,
    quality: &mut Quality,
) -> MetricValue {
    int_metric(
        current,
        previous,
        move |window| window.event_total(event),
        quality,
    )
}

fn share_total(count: Option<i64>, total: Option<i64>) -> Option<Decimal> {
    match (count, total) {
        (Some(count), Some(total)) if total > 0 => share_pct(count, total),
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum ComponentKind {
    Referrers,
    Entries,
}

impl ComponentKind {
    fn rows(self, window: &BehavioralWindow) -> Option<Vec<(String, i64)>> {
        match self {
            Self::Referrers => window.top_referrers.data().map(|rows| {
                rows.iter()
                    .map(|row| (row.referrer.as_str().to_owned(), row.count as i64))
                    .collect()
            }),
            Self::Entries => window.top_entries.data().map(|rows| {
                rows.iter()
                    .map(|row| (row.entry.as_str().to_owned(), row.count as i64))
                    .collect()
            }),
        }
    }
}

fn ranked(
    window: &BehavioralWindow,
    kind: ComponentKind,
    quality: &mut Quality,
    limit: usize,
) -> Vec<RankedRow> {
    let Some(rows) = kind.rows(window) else {
        return Vec::new();
    };
    let total: Option<i64> = Some(rows.iter().map(|(_, count)| *count).sum());
    rows.into_iter()
        .take(limit)
        .map(|(key, count)| {
            let (count, suppressed) = suppress_cell(Some(count), Some(count), quality);
            RankedRow {
                share_pct: if suppressed {
                    None
                } else {
                    share_total(count, total)
                },
                label: key.clone(),
                key,
                count,
                suppressed,
            }
        })
        .collect()
}

/// One requested property breakdown, or nothing when the provider could not
/// answer it. A missing breakdown is an absent collection, never zeros.
fn property_cells(
    window: &BehavioralWindow,
    event: Event,
    property: Property,
    limit: usize,
    quality: &mut Quality,
) -> Vec<(String, AggregateCell)> {
    let Some(result) = window.property(event, property) else {
        return Vec::new();
    };
    if !matches!(
        result.result.status(),
        ComponentStatus::Available | ComponentStatus::Partial
    ) {
        return Vec::new();
    }
    let Some(rows) = result.result.data() else {
        return Vec::new();
    };
    rows.iter()
        .take(limit)
        .map(|row| {
            (
                row.value.clone(),
                cell(
                    row.value.clone(),
                    Some(row.total as i64),
                    Some(row.total as i64),
                    quality,
                ),
            )
        })
        .collect()
}

/// Compose the acquisition tab from the current and comparison windows.
pub fn compose(
    query: &AnalyticsQuery,
    meta: Meta,
    current: Option<&Reading>,
    previous: Option<&Reading>,
    quality: &mut Quality,
) -> AcquisitionResponse {
    let duration = |reading: Option<&Reading>| -> Option<Decimal> {
        let window = &reading?.window;
        if window.core.status() == ComponentStatus::Unavailable {
            return None;
        }
        let core = window.core.data()?;
        let visits = core.visits?;
        if visits == 0 {
            return None;
        }
        let total = core.total_time_seconds?;
        Some(one_decimal(Decimal::from(total) / Decimal::from(visits)))
    };

    let visitors_trend: Vec<SeriesPoint> = match current.filter(|reading| reading.available()) {
        Some(reading) => {
            let mut values: BTreeMap<NaiveDate, i64> = BTreeMap::new();
            if let Some(rows) = reading.window.daily_visitors.data() {
                for row in rows {
                    *values.entry(row.date).or_insert(0) += row.visitors as i64;
                }
            }
            daily_series(query.start, query.end, &values, quality)
        }
        None => Vec::new(),
    };

    let (referrers, entry_pages, cta_matrix, languages) =
        match current.filter(|reading| reading.available()) {
            Some(reading) => {
                let window = &reading.window;
                let referrers = ranked(window, ComponentKind::Referrers, quality, MAX_RANKED_ROWS);
                let entry_pages = ranked(window, ComponentKind::Entries, quality, MAX_RANKED_ROWS);
                let cta_total = window.event_total(Event::LandingCtaClicked);

                let mut cta_matrix: Vec<CtaMatrixRow> = Vec::new();
                for (value, cta_cell) in
                    property_cells(window, Event::LandingCtaClicked, Property::Cta, 10, quality)
                {
                    cta_matrix.push(CtaMatrixRow {
                        cta: value,
                        placement: "all".to_owned(),
                        clicks: cta_cell.count,
                        share_pct: if cta_cell.suppressed {
                            None
                        } else {
                            share_total(cta_cell.count, cta_total)
                        },
                        suppressed: cta_cell.suppressed,
                    });
                }
                for (value, placement_cell) in property_cells(
                    window,
                    Event::LandingCtaClicked,
                    Property::Placement,
                    10,
                    quality,
                ) {
                    cta_matrix.push(CtaMatrixRow {
                        cta: "all".to_owned(),
                        placement: value,
                        clicks: placement_cell.count,
                        // The verified property API has no per-placement
                        // denominator, so no share is invented.
                        share_pct: None,
                        suppressed: placement_cell.suppressed,
                    });
                }
                cta_matrix.truncate(MAX_RANKED_ROWS);

                let languages: Vec<RankedRow> = property_cells(
                    window,
                    Event::LandingCtaClicked,
                    Property::Language,
                    5,
                    quality,
                )
                .into_iter()
                .map(|(value, language_cell)| RankedRow {
                    label: value.clone(),
                    key: value,
                    count: language_cell.count,
                    share_pct: None,
                    suppressed: language_cell.suppressed,
                })
                .collect();
                (referrers, entry_pages, cta_matrix, languages)
            }
            None => (Vec::new(), Vec::new(), Vec::new(), Vec::new()),
        };

    AcquisitionResponse {
        kpis: AcquisitionKpis {
            visitors: int_metric(
                current,
                previous,
                |window| {
                    window
                        .core
                        .data()
                        .and_then(|core| core.visitors)
                        .map(|value| value as i64)
                },
                quality,
            ),
            visits: int_metric(
                current,
                previous,
                |window| {
                    window
                        .core
                        .data()
                        .and_then(|core| core.visits)
                        .map(|value| value as i64)
                },
                quality,
            ),
            pageviews: int_metric(
                current,
                previous,
                |window| {
                    window
                        .core
                        .data()
                        .and_then(|core| core.pageviews)
                        .map(|value| value as i64)
                },
                quality,
            ),
            average_session_duration_seconds: DecimalMetricValue {
                value: duration(current),
                previous: duration(previous),
                suppressed: false,
            },
            cta_clicks: event_metric(current, previous, Event::LandingCtaClicked, quality),
        },
        visitors_trend,
        // This view requests neither of these series; they stay empty
        // collections rather than a dense zero series.
        previous_visitors_trend: Vec::new(),
        visits_trend: Vec::new(),
        referrers,
        entry_pages,
        cta_matrix,
        languages,
        calculator: CalculatorFunnel {
            starts: event_metric(current, previous, Event::EnergyCalculatorStarted, quality),
            completions: event_metric(current, previous, Event::EnergyCalculatorCompleted, quality),
        },
        meta,
    }
}

// Keep the requested-set helper honest if the registry changes.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_properties_are_registered_for_the_cta_event() {
        let set = requested_properties();
        assert_eq!(set.pairs().len(), 3);
        assert!(set.contains(Event::LandingCtaClicked, Property::Cta));
        assert!(!set.contains(Event::LoginFailed, Property::Cta));
    }
}

//! Canonical curve arithmetic and ordered public projections.
#[path = "curve_cells.rs"]
pub mod cells;
use crate::Json;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::json;

pub const DISCLAIMER: &str =
    "Indicative estimate only. Not legal, regulatory, tax, or compliance filing advice.";

pub fn decimal(value: &str) -> Decimal {
    if let Ok(decimal) = Decimal::from_str_exact(value) {
        return decimal;
    }
    let trimmed = if value.contains('.') {
        value.trim_end_matches('0').trim_end_matches('.')
    } else {
        value
    };
    Decimal::from_str_exact(trimmed).expect("bounded database decimal")
}
pub fn money(value: Decimal) -> String {
    let mut rounded = value.round_dp_with_strategy(2, RoundingStrategy::MidpointNearestEven);
    rounded.rescale(2);
    if value.is_sign_negative() && rounded.is_zero() {
        "-0.00".to_owned()
    } else {
        rounded.to_string()
    }
}
pub fn timestamp(value: DateTime<Utc>) -> Json {
    Json::String(crate::routes::ports::utc_rfc3339(value, "Z"))
}
pub fn default_windows(today: NaiveDate, board: bool) -> Vec<String> {
    let year = today.year();
    let month = today.month();
    let quarter = (month - 1) / 3;
    let mut windows = vec!["SPOT".to_owned()];
    if board {
        for offset in 1..=6 {
            let absolute = month - 1 + offset;
            windows.push(format!(
                "{}-{:02}",
                year + (absolute / 12) as i32,
                absolute % 12 + 1
            ));
        }
    } else {
        for month in month..=(quarter + 1) * 3 {
            windows.push(format!("{year}-{month:02}"));
        }
    }
    for offset in 1..=if board { 4 } else { 20 } {
        let absolute = year * 4 + quarter as i32 + offset;
        windows.push(format!("{}-Q{}", absolute / 4, absolute % 4 + 1));
    }
    if board {
        windows.extend([format!("{}-CAL", year + 1), format!("{}-CAL", year + 2)]);
    }
    windows
}

#[derive(Default, Clone)]
pub struct Book {
    pub real_bid: Option<Decimal>,
    pub real_ask: Option<Decimal>,
    pub demo_bid: Option<Decimal>,
    pub demo_ask: Option<Decimal>,
    pub real_volume: Decimal,
    pub demo_volume: Decimal,
    pub real_count: i64,
    pub demo_count: i64,
    pub unknown_count: i64,
    pub managed_count: i64,
    pub real_time: Option<DateTime<Utc>>,
    pub demo_time: Option<DateTime<Utc>>,
}
pub struct BookGroup {
    pub count: i64,
    pub volume: Decimal,
    pub best_price: Decimal,
    pub observed_at: Option<DateTime<Utc>>,
    pub managed_count: i64,
}

impl Book {
    pub fn merge(&mut self, side: &str, real: bool, group: BookGroup) {
        let best = match (real, side) {
            (true, "BID") => &mut self.real_bid,
            (true, _) => &mut self.real_ask,
            (false, "BID") => &mut self.demo_bid,
            _ => &mut self.demo_ask,
        };
        if best.is_none_or(|old| {
            if side == "BID" {
                group.best_price > old
            } else {
                group.best_price < old
            }
        }) {
            *best = Some(group.best_price);
        }
        if real {
            self.real_count += group.count;
            self.real_volume += group.volume;
            self.real_time = self.real_time.max(group.observed_at);
        } else {
            self.demo_count += group.count;
            self.demo_volume += group.volume;
            self.demo_time = self.demo_time.max(group.observed_at);
            self.managed_count += group.managed_count;
        }
    }

    pub fn add(
        &mut self,
        side: &str,
        real: bool,
        price: Decimal,
        quantity: Decimal,
        observed: Option<DateTime<Utc>>,
        managed: bool,
    ) {
        self.merge(
            side,
            real,
            BookGroup {
                count: 1,
                volume: quantity,
                best_price: price,
                observed_at: observed,
                managed_count: i64::from(managed),
            },
        );
    }
    pub fn status(&self) -> &'static str {
        if self.real_count > 0 {
            "REAL_ONLY"
        } else if self.demo_count > 0 {
            "DEMO_ONLY"
        } else if self.unknown_count > 0 {
            "UNKNOWN"
        } else {
            "NOT_APPLICABLE"
        }
    }
    pub fn source(&self) -> &'static str {
        match self.status() {
            "REAL_ONLY" => "LIVE_ORDER",
            "DEMO_ONLY" => "DEMO_SEED",
            "UNKNOWN" => "UNKNOWN",
            _ => "NO_DATA",
        }
    }
    pub fn selected(
        &self,
    ) -> (
        Option<Decimal>,
        Option<Decimal>,
        Decimal,
        i64,
        Option<DateTime<Utc>>,
    ) {
        if self.real_count > 0 {
            (
                self.real_bid,
                self.real_ask,
                self.real_volume,
                self.real_count,
                self.real_time,
            )
        } else if self.demo_count > 0 {
            (
                self.demo_bid,
                self.demo_ask,
                self.demo_volume,
                self.demo_count,
                self.demo_time,
            )
        } else {
            (None, None, Decimal::ZERO, 0, None)
        }
    }
}

pub fn no_band(now: DateTime<Utc>) -> Json {
    super::signals::provenance(&[], "FAIR_PRICE_BAND", now)
}
pub fn label_policy(
    label: &str,
    tooltip: &str,
    allowed: &[&str],
    disclaimer: Option<&str>,
) -> Json {
    json!({"public_label":label,"tooltip":tooltip,"allowed_terms":allowed,
        "forbidden_terms":["live","top-of-book","firm","market price","last","clearing"],"disclaimer":disclaimer})
}

pub fn midpoint(bid: Option<Decimal>, ask: Option<Decimal>) -> Option<Decimal> {
    bid.zip(ask).map(|(bid, ask)| {
        ((bid + ask) / Decimal::from(2))
            .round_dp_with_strategy(2, RoundingStrategy::MidpointNearestEven)
    })
}

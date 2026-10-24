//! Shared Product Analytics query parsing and validation.
//!
//! One pure parser owns the rules every analytics route applies before any
//! database or provider access:
//!
//! - `start` and `end` are required timestamps; a naive date-time is UTC and
//!   an explicit offset is converted to UTC;
//! - the period is half-open and `end` must be strictly after `start`;
//! - the elapsed range is at most 365 days (31,536,000 seconds);
//! - with `compare = true` the previous period is the checked, immediately
//!   preceding interval of the same length, and underflow is a 422 refusal;
//! - `availability_window` is at most 16 characters;
//! - unknown parameters are ignored and never reach SQL or a provider.
//!
//! Refusals use `{"detail": {"code": "ANALYTICS_QUERY_INVALID", ...}}` with
//! HTTP 422. Catalog and canonical-window checks are route-owned database
//! validation after this parser; they are not part of this pure function.

use axum::http::StatusCode;
use chrono::{DateTime, NaiveDateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::types::{AnalyticsActivity, AnalyticsAudience};
use crate::business::error::refusal;
use crate::ApiError;

/// Longest accepted half-open period: 365 days.
pub const MAX_RANGE_SECONDS: i64 = 365 * 24 * 60 * 60;

/// Longest accepted availability-window string.
pub const MAX_AVAILABILITY_WINDOW_CHARS: usize = 16;

/// Raw query parameters, before validation.
///
/// Unknown keys are ignored. Every field is optional so a missing parameter
/// can be refused or defaulted explicitly, rather than at deserialization
/// time.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct QueryInput {
    pub start: Option<String>,
    pub end: Option<String>,
    pub compare: Option<String>,
    pub audience: Option<String>,
    pub activity: Option<String>,
    pub product_id: Option<String>,
    pub delivery_point_id: Option<String>,
    pub availability_window: Option<String>,
}

impl QueryInput {
    /// Collect a query-string map, ignoring unknown parameters and keeping
    /// the last value for a repeated known parameter.
    pub fn from_pairs<'a, I>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        let mut input = Self::default();
        for (key, value) in pairs {
            let value = Some(value.to_owned());
            match key {
                "start" => input.start = value,
                "end" => input.end = value,
                "compare" => input.compare = value,
                "audience" => input.audience = value,
                "activity" => input.activity = value,
                "product_id" => input.product_id = value,
                "delivery_point_id" => input.delivery_point_id = value,
                "availability_window" => input.availability_window = value,
                _ => {}
            }
        }
        input
    }
}

/// Validated analytics query shared by every product analytics tab.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnalyticsQuery {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    /// Checked immediately preceding interval, present only with `compare`.
    pub previous: Option<(DateTime<Utc>, DateTime<Utc>)>,
    pub compare: bool,
    pub audience: AnalyticsAudience,
    pub activity: AnalyticsActivity,
    pub product_id: Option<Uuid>,
    pub delivery_point_id: Option<Uuid>,
    pub availability_window: Option<String>,
}

impl AnalyticsQuery {
    pub fn previous_start(&self) -> Option<DateTime<Utc>> {
        self.previous.map(|(start, _)| start)
    }

    pub fn previous_end(&self) -> Option<DateTime<Utc>> {
        self.previous.map(|(_, end)| end)
    }
}

/// Parse and validate one query. No database or provider access occurs here.
pub fn validate_query(input: QueryInput) -> Result<AnalyticsQuery, ApiError> {
    let start = input
        .start
        .as_deref()
        .ok_or_else(|| invalid("start is required"))?;
    let start = parse_timestamp(start).ok_or_else(|| invalid("start must be a timestamp"))?;
    let end = input
        .end
        .as_deref()
        .ok_or_else(|| invalid("end is required"))?;
    let end = parse_timestamp(end).ok_or_else(|| invalid("end must be a timestamp"))?;

    if end <= start {
        return Err(invalid("end must be after start"));
    }
    let elapsed = end.signed_duration_since(start);
    if elapsed > TimeDelta::seconds(MAX_RANGE_SECONDS) {
        return Err(invalid("range must not exceed 365 days"));
    }

    let compare = match input.compare.as_deref() {
        None => true,
        Some("true") => true,
        Some("false") => false,
        Some(_) => return Err(invalid("compare must be true or false")),
    };
    let audience = match input.audience.as_deref() {
        None | Some("ALL") => AnalyticsAudience::All,
        Some("BUYER") => AnalyticsAudience::Buyer,
        Some("SUPPLIER") => AnalyticsAudience::Supplier,
        Some(_) => return Err(invalid("audience must be ALL, BUYER or SUPPLIER")),
    };
    let activity = match input.activity.as_deref() {
        None | Some("LIVE") => AnalyticsActivity::Live,
        Some("DEMO") => AnalyticsActivity::Demo,
        Some("REFERENCE") => AnalyticsActivity::Reference,
        Some("ALL") => AnalyticsActivity::All,
        Some(_) => {
            return Err(invalid("activity must be LIVE, DEMO, REFERENCE or ALL"));
        }
    };

    let product_id = parse_uuid(input.product_id.as_deref(), "product_id must be a UUID")?;
    let delivery_point_id = parse_uuid(
        input.delivery_point_id.as_deref(),
        "delivery_point_id must be a UUID",
    )?;

    if input
        .availability_window
        .as_deref()
        .is_some_and(|window| window.chars().count() > MAX_AVAILABILITY_WINDOW_CHARS)
    {
        return Err(invalid("availability_window must be at most 16 characters"));
    }

    let previous = if compare {
        // The previous period has the same length and ends exactly at `start`.
        let previous_start = start
            .checked_sub_signed(elapsed)
            .ok_or_else(|| invalid("the comparison period is outside the supported range"))?;
        Some((previous_start, start))
    } else {
        None
    };

    Ok(AnalyticsQuery {
        start,
        end,
        previous,
        compare,
        audience,
        activity,
        product_id,
        delivery_point_id,
        availability_window: input.availability_window,
    })
}

fn invalid(message: &'static str) -> ApiError {
    refusal(
        StatusCode::UNPROCESSABLE_ENTITY,
        "ANALYTICS_QUERY_INVALID",
        message,
    )
}

fn parse_uuid(value: Option<&str>, message: &'static str) -> Result<Option<Uuid>, ApiError> {
    value
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| invalid(message))
}

/// Parse one ISO 8601 timestamp.
///
/// The relaxed RFC 3339 form accepts a space or `T` separator and an offset
/// with or without a colon. Expanded ISO 8601 years (outside RFC 3339's four
/// digits) are also accepted so the checked previous-window guard stays
/// reachable; ordinary client values never use them.
fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    if value.is_empty() {
        return None;
    }
    if let Ok(parsed) = value.parse::<DateTime<Utc>>() {
        return Some(parsed);
    }
    let offset_candidate = match value.strip_suffix(['Z', 'z']) {
        Some(base) => format!("{base}+00:00"),
        None => value.to_owned(),
    };
    for format in ["%Y-%m-%dT%H:%M:%S%.f%:z", "%Y-%m-%dT%H:%M:%S%.f%z"] {
        if let Ok(parsed) = DateTime::parse_from_str(&offset_candidate, format) {
            return Some(parsed.with_timezone(&Utc));
        }
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(value, format) {
            return Some(DateTime::from_naive_utc_and_offset(naive, Utc));
        }
    }
    None
}

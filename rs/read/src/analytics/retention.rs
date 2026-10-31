//! Retention policy for Product Analytics facts.
//!
//! `user_login_days` keeps exactly 800 UTC calendar dates: today plus the
//! previous 799. Rows with an activity date before `login_retention_cutoff`
//! are prunable. 800 dates cover a maximum 366-point daily series and its
//! 366-point comparison series, so a full-range query and its comparison are
//! always inside the window.
//!
//! `user_status_transitions` is durable business history and is never pruned.
//!
//! This module is the pure policy only. Phase 4 installs no prune command and
//! no timer; the existing environment-specific Python timer keeps ownership
//! until Phase 10, and consumes the same rule.

use chrono::{NaiveDate, TimeDelta};

use super::policy::MAX_SERIES_POINTS;

/// Number of retained UTC calendar dates, including today.
pub const RETAINED_DATES: i64 = 800;

/// Longest set of daily points one response can need: the period series plus
/// its comparison series.
pub const MAX_SERIES_DATES: i64 = 2 * MAX_SERIES_POINTS as i64;

/// The Product Analytics fact families and whether a retention prune applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnalyticsFactFamily {
    /// Daily login fact, pruned at the 800-date boundary.
    LoginDay,
    /// Account-status history, retained indefinitely.
    StatusTransition,
}

impl AnalyticsFactFamily {
    /// Whether the 800-date login-day prune deletes rows of this family.
    pub fn is_pruned(self) -> bool {
        matches!(self, AnalyticsFactFamily::LoginDay)
    }
}

/// First retained UTC calendar date: `today - 799`.
///
/// Anything older is prunable. The subtraction saturates at the earliest
/// representable date, so the policy never panics for an extreme input and
/// nothing is prunable in that degenerate case.
pub fn login_retention_cutoff(today: NaiveDate) -> NaiveDate {
    today
        .checked_sub_signed(TimeDelta::days(RETAINED_DATES - 1))
        .unwrap_or(NaiveDate::MIN)
}

/// Whether one login-day date is inside the retained window.
pub fn login_day_is_retained(activity_date: NaiveDate, today: NaiveDate) -> bool {
    activity_date >= login_retention_cutoff(today)
}

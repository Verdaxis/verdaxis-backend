//! Time boundaries and identity for the trading cache.
//!
//! Trading data has no safe time-to-live. An entry is usable only while every
//! time-dependent predicate that produced it keeps the same answer. The route
//! that computes a payload therefore reports the earliest instant at which its
//! own result can change without a write: an order expiry, a KYC-grace cutoff,
//! a certificate date, a rolling-window departure, a benchmark expiry or a
//! calendar-dependent rule.
//!
//! Every instant is derived from the one [`TradingClock`] the route captured
//! before admission and bound into its SQL. A change reported at or before
//! that clock cannot come from it, so the boundary fails closed.
//!
//! A route that cannot derive one of the dependencies it reads reports
//! [`Deadline::Unknown`]. The cache then bypasses instead of guessing a TTL.
//! [`Deadline::None`] is a positive claim: time alone cannot change the result,
//! and any later change is a write that advances the trading generation. The
//! two are deliberately different types so a missing derivation cannot look
//! like a proved one.
use chrono::{Datelike, Duration, NaiveDate, TimeZone, Utc};
use sha2::{Digest, Sha256};
use std::time::{Instant, SystemTime};

/// Every certificate date, grace cutoff and calendar rule in the read
/// contract is stated in the business day used by the Python oracle.
pub const BUSINESS_TIMEZONE: chrono_tz::Tz = chrono_tz::Asia::Singapore;

/// The next instant one dependency can change the query result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Deadline {
    /// Time alone cannot change the result.
    None,
    /// The result can change at this wall-clock instant.
    At(SystemTime),
    /// The route did not derive the dependency, so it must not cache.
    #[default]
    Unknown,
}

/// A rolling lookback such as `observed_at >= $2 - interval '7 days'`.
///
/// Each entry is the earliest instant at which an included row leaves its
/// window's trailing edge. `Derived(vec![])` is a proved "no included row can
/// leave"; `Unknown` means the route reads a window but could not derive the
/// edge.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RollingWindows {
    /// The query reads no rolling window. A family that lists
    /// [`TemporalFact::RollingWindows`] always reads one, so for that family
    /// this is not a derivation and the entry is not stored.
    None,
    /// One departure instant per window the query reads.
    Derived(Vec<SystemTime>),
    /// The query reads a window and the route cannot derive its departure.
    #[default]
    Unknown,
}

/// Which facts a reviewed route family must derive before it may cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemporalFact {
    OrderExpiry,
    KycGrace,
    DeclarationDate,
    RollingWindows,
    BenchmarkExpiry,
    CalendarWindow,
}

/// Facts a route family reads, or `None` when the family is not reviewed for
/// trading caching. An unreviewed family is never cached, whatever it reports.
pub fn required_facts(family: &str) -> Option<&'static [TemporalFact]> {
    match family {
        // Resting orders: expiry, owner admission through KYC grace, and the
        // FAME certificate date compared with the Singapore business day.
        "orderbook" => Some(&[
            TemporalFact::OrderExpiry,
            TemporalFact::KycGrace,
            TemporalFact::DeclarationDate,
        ]),
        // Live benchmarks are resting orders projected through the same rule.
        "benchmarks" => Some(&[
            TemporalFact::OrderExpiry,
            TemporalFact::KycGrace,
            TemporalFact::DeclarationDate,
        ]),
        // Curves read resting orders through the same rule, 7/30/90-day
        // evidence windows, 30 days of trades, benchmark rows and the
        // calendar-dependent default window.
        "curves" => Some(&[
            TemporalFact::OrderExpiry,
            TemporalFact::KycGrace,
            TemporalFact::DeclarationDate,
            TemporalFact::RollingWindows,
            TemporalFact::BenchmarkExpiry,
            TemporalFact::CalendarWindow,
        ]),
        // Price summaries use a moving lookback, even when trade rows are
        // immutable. An included trade can leave without a write.
        "prices" => Some(&[TemporalFact::RollingWindows]),
        // Explicit fixed ranges do not move with the request clock.
        "prices_fixed" => Some(&[]),
        // The tape and its `total` read `confirmed_at >= now - 7 days`.
        "trade_tape" => Some(&[TemporalFact::RollingWindows]),
        _ => None,
    }
}

/// Captured once per read so every derived instant shares one clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TradingClock {
    pub monotonic: Instant,
    pub wall: SystemTime,
}

impl TradingClock {
    pub fn now() -> Self {
        Self {
            monotonic: Instant::now(),
            wall: SystemTime::now(),
        }
    }

    pub fn new(monotonic: Instant, wall: SystemTime) -> Self {
        Self { monotonic, wall }
    }

    /// The monotonic instant at which a wall-clock deadline is reached. A
    /// deadline already in the past maps to `self.monotonic`.
    pub fn monotonic_at(&self, wall: SystemTime) -> Instant {
        match wall.duration_since(self.wall) {
            Ok(ahead) => self.monotonic + ahead,
            Err(_) => self.monotonic,
        }
    }

    pub fn singapore_date(&self) -> NaiveDate {
        let utc: chrono::DateTime<Utc> = self.wall.into();
        utc.with_timezone(&BUSINESS_TIMEZONE).date_naive()
    }

    /// Midnight that starts the next Singapore business day.
    pub fn next_singapore_midnight(&self) -> Option<SystemTime> {
        let tomorrow = self.singapore_date().succ_opt()?;
        let local = BUSINESS_TIMEZONE.with_ymd_and_hms(
            tomorrow.year(),
            tomorrow.month(),
            tomorrow.day(),
            0,
            0,
            0,
        );
        match local {
            chrono::LocalResult::Single(value) => Some(value.to_utc().into()),
            _ => None,
        }
    }
}

/// Every time dependency one route reads. `Default` is all-unknown, so a route
/// that forgets a field bypasses instead of caching with a guessed boundary.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TemporalFacts {
    pub order_expiry: Deadline,
    pub kyc_grace: Deadline,
    pub declaration_date: Deadline,
    pub rolling_windows: RollingWindows,
    pub benchmark_expiry: Deadline,
    pub calendar_window: Deadline,
}

impl TemporalFacts {
    /// A query over written rows only: counts, immutable trade snapshots and
    /// fixed request ranges. Only a write can change the answer.
    ///
    /// This asserts that *no* time predicate shaped the query. It is sound only
    /// for the reviewed family whose query has no time predicate at all
    /// (`prices_fixed`). A route that reads order expiry, KYC grace, a
    /// certificate date, a rolling window or a calendar rule must build its
    /// facts from the boundary helpers below, which report what the actual rows
    /// proved. A moving-window family refuses this constructor's "no window".
    pub fn proven_written_only() -> Self {
        Self {
            order_expiry: Deadline::None,
            kyc_grace: Deadline::None,
            declaration_date: Deadline::None,
            rolling_windows: RollingWindows::None,
            benchmark_expiry: Deadline::None,
            calendar_window: Deadline::None,
        }
    }

    /// The instants at which one fact can change the result: empty when time
    /// cannot change it, `None` when the route did not derive it.
    fn changes(&self, fact: TemporalFact) -> Option<&[SystemTime]> {
        let deadline = match fact {
            TemporalFact::OrderExpiry => &self.order_expiry,
            TemporalFact::KycGrace => &self.kyc_grace,
            TemporalFact::DeclarationDate => &self.declaration_date,
            TemporalFact::BenchmarkExpiry => &self.benchmark_expiry,
            TemporalFact::CalendarWindow => &self.calendar_window,
            // A family that lists this fact always reads a moving window, so
            // only derived departures prove anything.
            TemporalFact::RollingWindows => {
                return match &self.rolling_windows {
                    RollingWindows::Derived(departures) => Some(departures.as_slice()),
                    RollingWindows::None | RollingWindows::Unknown => None,
                };
            }
        };
        match deadline {
            Deadline::None => Some(&[]),
            Deadline::At(wall) => Some(std::slice::from_ref(wall)),
            Deadline::Unknown => None,
        }
    }

    /// The earliest change among the facts this family actually reads. A fact
    /// the family reads but the route did not derive makes it `Unknown`.
    pub fn boundary_for(&self, family: &str, clock: &TradingClock) -> Boundary {
        let Some(required) = required_facts(family) else {
            return Boundary::Unknown;
        };
        let mut earliest: Option<Instant> = None;
        for fact in required {
            let Some(changes) = self.changes(*fact) else {
                return Boundary::Unknown;
            };
            for wall in changes {
                // Every helper maps an instant that has passed to
                // `Deadline::None`, and a row that a window still includes
                // departs after the clock. A change reported at or before
                // `clock` therefore means the loader bound another, earlier
                // clock: its rows may already be stale, so fail closed.
                let ahead = match wall.duration_since(clock.wall) {
                    Ok(ahead) if !ahead.is_zero() => ahead,
                    _ => return Boundary::Unknown,
                };
                // No process outlives an instant its monotonic clock cannot
                // represent, so such a change never ends the entry.
                if let Some(at) = clock.monotonic.checked_add(ahead) {
                    earliest = Some(earliest.map_or(at, |current| current.min(at)));
                }
            }
        }
        earliest.map_or(Boundary::Never, Boundary::At)
    }

    /// Whether every fact this family reads is derived. `required_facts` is
    /// the authority: a family that is not reviewed is refused outright, and a
    /// fact the family does not read is ignored, so a route is never asked to
    /// prove a dependency its query does not have.
    pub fn complete_for(&self, family: &str) -> bool {
        required_facts(family)
            .is_some_and(|required| required.iter().all(|fact| self.changes(*fact).is_some()))
    }
}

/// Whether a fill may be stored: the family is reviewed and every boundary it
/// reads is known. `Boundary::Never` is cacheable because time alone cannot
/// change it.
pub fn cacheable(family: &str, facts: &TemporalFacts, clock: &TradingClock) -> bool {
    facts.complete_for(family) && facts.boundary_for(family, clock) != Boundary::Unknown
}

/// The monotonic deadline stored beside a payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boundary {
    Never,
    At(Instant),
    Unknown,
}

impl Boundary {
    /// `Unknown` is never live, so an accidental store cannot serve a hit.
    pub fn is_live(self, now: Instant) -> bool {
        match self {
            Self::Never => true,
            Self::At(at) => now < at,
            Self::Unknown => false,
        }
    }
}

/// Order rows disappear at `expires_at`; `None` means no included row expires.
pub fn order_expiry_boundary(earliest: Option<SystemTime>, clock: &TradingClock) -> Deadline {
    future_deadline(earliest, clock)
}

/// `kyc_execution_grace.expires_at > $2` changes at the same instant.
pub fn kyc_grace_boundary(earliest: Option<SystemTime>, clock: &TradingClock) -> Deadline {
    future_deadline(earliest, clock)
}

/// `benchmarks.valid_until` and live benchmark expiry.
pub fn benchmark_expiry_boundary(earliest: Option<SystemTime>, clock: &TradingClock) -> Deadline {
    future_deadline(earliest, clock)
}

/// `certificate_valid_until >= $3` where `$3` is the Singapore date. The
/// predicate can change only at a Singapore midnight, and only when that
/// midnight passes the earliest included certificate date.
pub fn certificate_boundary(
    earliest_valid_until: Option<NaiveDate>,
    clock: &TradingClock,
) -> Deadline {
    let Some(date) = earliest_valid_until else {
        return Deadline::None;
    };
    if date < clock.singapore_date() {
        // An ineligible certificate never becomes eligible again.
        return Deadline::None;
    }
    let first_changed_day = date.succ_opt();
    match first_changed_day.and_then(|day| {
        BUSINESS_TIMEZONE
            .with_ymd_and_hms(day.year(), day.month(), day.day(), 0, 0, 0)
            .single()
    }) {
        Some(value) => Deadline::At(value.to_utc().into()),
        None => Deadline::Unknown,
    }
}

/// A rule that reads the Singapore business day changes at the next midnight.
pub fn calendar_boundary(clock: &TradingClock) -> Deadline {
    clock
        .next_singapore_midnight()
        .map_or(Deadline::Unknown, Deadline::At)
}

/// The earliest departure instant of a rolling lookback
/// (`observed_at >= now - days`). `earliest_observed` is the oldest included
/// row's `observed_at`; `None` means no included row can leave the window.
///
/// The predicate keeps a row on the window's edge, which leaves just after the
/// clock. Such a row, or one the window does not include at `clock`, leaves
/// no instant to store against, so the departure is `Unknown`.
pub fn rolling_window_departure(
    earliest_observed: Option<SystemTime>,
    days: i64,
    clock: &TradingClock,
) -> Deadline {
    let Some(observed) = earliest_observed else {
        return Deadline::None;
    };
    let departure = observed
        .checked_add(Duration::days(days).to_std().unwrap_or_default())
        .unwrap_or(observed);
    if departure <= clock.wall {
        Deadline::Unknown
    } else {
        Deadline::At(departure)
    }
}

fn future_deadline(value: Option<SystemTime>, clock: &TradingClock) -> Deadline {
    match value {
        None => Deadline::None,
        Some(wall) if wall <= clock.wall => Deadline::None,
        Some(wall) => Deadline::At(wall),
    }
}

/// Stable identity of the process's environment, release and market policy.
/// Two entries with different identities are never interchangeable. Secrets
/// and the database URL are deliberately absent.
pub fn policy_identity(config: &crate::Config, policy: &crate::MarketPolicy) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"verdaxis-trading-cache-policy-v1\0");
    let text = |hasher: &mut Sha256, value: &str| {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    };
    text(&mut hasher, &config.environment);
    text(&mut hasher, &config.release_sha);
    text(&mut hasher, &config.migration_revision);
    text(&mut hasher, &config.db_session_time_zone);
    hasher.update([u8::from(config.kyc_execution_gate_enabled)]);
    text(&mut hasher, &policy.source_sha);
    hasher.update((policy.products.len() as u64).to_be_bytes());
    for product in &policy.products {
        text(&mut hasher, &product.id.to_string());
        text(&mut hasher, &product.code);
        text(&mut hasher, &product.name);
        text(&mut hasher, &product.fuel_type);
        text(&mut hasher, &product.fuel_grade);
        text(&mut hasher, &product.execution_mode);
        hasher.update((product.lanes.len() as u64).to_be_bytes());
        for lane in &product.lanes {
            text(&mut hasher, &lane.to_string());
        }
        text(&mut hasher, &product.qualification_profile);
        text(
            &mut hasher,
            product.specification_standard.as_deref().unwrap_or(""),
        );
        hasher.update(product.display_order.to_be_bytes());
    }
    hasher.update((policy.points.len() as u64).to_be_bytes());
    for point in &policy.points {
        text(&mut hasher, &point.id.to_string());
        text(&mut hasher, &point.name);
        text(&mut hasher, &point.region);
        text(&mut hasher, &point.timezone);
        hasher.update(point.display_order.to_be_bytes());
    }
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            use std::fmt::Write;
            let _ = write!(text, "{byte:02x}");
            text
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wall(text: &str) -> SystemTime {
        chrono::DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
            .into()
    }

    fn clock(text: &str) -> TradingClock {
        TradingClock::new(Instant::now(), wall(text))
    }

    fn at(text: &str) -> Deadline {
        Deadline::At(wall(text))
    }

    #[test]
    fn expiry_boundary_is_the_earliest_future_deadline() {
        let clock = clock("2026-10-20T08:00:00Z");
        let past = wall("2026-10-20T07:00:00Z");
        let soon = wall("2026-10-20T09:00:00Z");
        let later = wall("2026-10-20T11:00:00Z");
        assert_eq!(order_expiry_boundary(Some(past), &clock), Deadline::None);
        assert_eq!(order_expiry_boundary(None, &clock), Deadline::None);
        assert_eq!(
            order_expiry_boundary(Some(later), &clock),
            Deadline::At(later)
        );
        let facts = TemporalFacts {
            order_expiry: order_expiry_boundary(Some(soon), &clock),
            kyc_grace: kyc_grace_boundary(Some(later), &clock),
            ..TemporalFacts::proven_written_only()
        };
        let Boundary::At(boundary) = facts.boundary_for("orderbook", &clock) else {
            panic!("a future dependency must produce a boundary");
        };
        assert_eq!(boundary, clock.monotonic_at(soon));
        assert!(Boundary::At(boundary).is_live(clock.monotonic));
        assert!(!Boundary::At(boundary).is_live(boundary));
    }

    #[test]
    fn unknown_dependency_bypasses_instead_of_guessing() {
        let clock = clock("2026-10-20T08:00:00Z");
        let facts = TemporalFacts::default();
        assert_eq!(facts.boundary_for("orderbook", &clock), Boundary::Unknown);
        assert!(!cacheable("orderbook", &facts, &clock));
        // The shortcut is sound only for a family whose query has no time
        // predicate. A moving window or an orderbook rule must be derived.
        assert!(cacheable(
            "prices_fixed",
            &TemporalFacts::proven_written_only(),
            &clock
        ));
        for moving in ["prices", "trade_tape"] {
            assert!(
                !cacheable(moving, &TemporalFacts::proven_written_only(), &clock),
                "{moving}"
            );
        }
        assert!(!cacheable(
            "not-a-reviewed-route",
            &TemporalFacts::proven_written_only(),
            &clock
        ));
        // An empty orderbook genuinely proves every fact from its own rows.
        let empty_orderbook = TemporalFacts {
            order_expiry: order_expiry_boundary(None, &clock),
            kyc_grace: kyc_grace_boundary(None, &clock),
            declaration_date: certificate_boundary(None, &clock),
            ..TemporalFacts::default()
        };
        assert!(cacheable("orderbook", &empty_orderbook, &clock));
        // A route that could not derive a dependency its family reads is
        // refused; curves read rolling windows, orderbook does not.
        let missing_window = TemporalFacts {
            rolling_windows: RollingWindows::Unknown,
            ..TemporalFacts::proven_written_only()
        };
        assert_eq!(
            missing_window.boundary_for("curves", &clock),
            Boundary::Unknown
        );
        assert!(!missing_window.complete_for("curves"));
        assert!(missing_window.complete_for("orderbook"));
        assert!(cacheable("orderbook", &missing_window, &clock));
        let window = TemporalFacts {
            rolling_windows: RollingWindows::Derived(vec![]),
            ..TemporalFacts::proven_written_only()
        };
        assert_eq!(window.boundary_for("prices", &clock), Boundary::Never);
    }

    #[test]
    fn every_family_requires_the_time_predicates_its_query_binds() {
        // The shared order rule binds `expires_at > $2`, the KYC grace cutoff
        // and the certificate date. Curves read resting orders through it too.
        let order_rule = [
            TemporalFact::OrderExpiry,
            TemporalFact::KycGrace,
            TemporalFact::DeclarationDate,
        ];
        for family in ["orderbook", "benchmarks", "curves"] {
            let required = required_facts(family).unwrap();
            for fact in order_rule {
                assert!(required.contains(&fact), "{family}: {fact:?}");
            }
        }
        // Moving trade and signal lookbacks: the tape's seven days, the price
        // summary's hours and the curves' 7/30/90-day evidence windows.
        for family in ["curves", "prices", "trade_tape"] {
            assert!(
                required_facts(family)
                    .unwrap()
                    .contains(&TemporalFact::RollingWindows),
                "{family}"
            );
        }
        // Explicit request dates only.
        assert_eq!(required_facts("prices_fixed"), Some(&[][..]));
    }

    #[test]
    fn moving_windows_need_a_derived_departure() {
        let clock = clock("2026-10-20T08:00:00Z");
        // Confirmed six days and 23 hours ago: it leaves a seven-day tape in
        // one hour, with no write.
        let confirmed = wall("2026-10-13T09:00:00Z");
        let leaves = wall("2026-10-20T09:00:00Z");
        assert_eq!(
            rolling_window_departure(Some(confirmed), 7, &clock),
            Deadline::At(leaves)
        );
        for family in ["prices", "trade_tape"] {
            // "No window" and "not derived" both leave the departure unproved.
            for unproved in [RollingWindows::None, RollingWindows::Unknown] {
                let facts = TemporalFacts {
                    rolling_windows: unproved.clone(),
                    ..TemporalFacts::proven_written_only()
                };
                assert_eq!(
                    facts.boundary_for(family, &clock),
                    Boundary::Unknown,
                    "{family}: {unproved:?}"
                );
                assert!(!cacheable(family, &facts, &clock), "{family}: {unproved:?}");
            }
            let leaving = TemporalFacts {
                rolling_windows: RollingWindows::Derived(vec![leaves]),
                ..TemporalFacts::proven_written_only()
            };
            assert_eq!(
                leaving.boundary_for(family, &clock),
                Boundary::At(clock.monotonic_at(leaves)),
                "{family}"
            );
            // A derived empty list proves that no included row can leave.
            let staying = TemporalFacts {
                rolling_windows: RollingWindows::Derived(vec![]),
                ..TemporalFacts::proven_written_only()
            };
            assert_eq!(staying.boundary_for(family, &clock), Boundary::Never);
        }
    }

    #[test]
    fn a_reported_change_at_or_before_the_clock_fails_closed() {
        // Every helper maps an instant that has passed to `Deadline::None`. A
        // reported change at or before the clock therefore means the loader
        // bound an earlier clock, and its rows may already be stale.
        let clock = clock("2026-10-20T08:00:00Z");
        for reported in ["2026-10-20T07:59:59Z", "2026-10-20T08:00:00Z"] {
            let expiry = TemporalFacts {
                order_expiry: at(reported),
                ..TemporalFacts::proven_written_only()
            };
            assert_eq!(
                expiry.boundary_for("orderbook", &clock),
                Boundary::Unknown,
                "{reported}"
            );
            assert!(!cacheable("orderbook", &expiry, &clock), "{reported}");
            let departure = TemporalFacts {
                rolling_windows: RollingWindows::Derived(vec![wall(reported)]),
                ..TemporalFacts::proven_written_only()
            };
            assert_eq!(
                departure.boundary_for("prices", &clock),
                Boundary::Unknown,
                "{reported}"
            );
        }
        // `confirmed_at >= now - 7 days` still includes a row on the edge, and
        // that row leaves at once: there is no instant to store against.
        assert_eq!(
            rolling_window_departure(Some(wall("2026-10-13T08:00:00Z")), 7, &clock),
            Deadline::Unknown
        );
    }

    #[test]
    fn kyc_grace_cutoff_is_a_boundary() {
        let clock = clock("2026-10-20T08:00:00Z");
        let cutoff = wall("2026-10-20T08:30:00Z");
        let facts = TemporalFacts {
            kyc_grace: kyc_grace_boundary(Some(cutoff), &clock),
            ..TemporalFacts::proven_written_only()
        };
        assert_eq!(
            facts.boundary_for("orderbook", &clock),
            Boundary::At(clock.monotonic_at(cutoff))
        );
    }

    #[test]
    fn rolling_window_departures_are_the_earliest_edge() {
        let clock = clock("2026-10-20T08:00:00Z");
        let oldest = wall("2026-10-14T08:00:00Z");
        let week = rolling_window_departure(Some(oldest), 7, &clock);
        assert_eq!(week, at("2026-10-21T08:00:00Z"));
        assert_eq!(rolling_window_departure(None, 30, &clock), Deadline::None);
        let facts = TemporalFacts {
            rolling_windows: RollingWindows::Derived(vec![
                wall("2026-10-27T08:00:00Z"),
                wall("2026-10-21T08:00:00Z"),
                wall("2026-12-01T08:00:00Z"),
            ]),
            ..TemporalFacts::proven_written_only()
        };
        assert_eq!(
            facts.boundary_for("prices", &clock),
            Boundary::At(clock.monotonic_at(wall("2026-10-21T08:00:00Z")))
        );
    }

    #[test]
    fn singapore_day_boundary_uses_business_midnight() {
        // 23:30 in Singapore on 20 October is 15:30 UTC.
        let clock = clock("2026-10-20T15:30:00Z");
        assert_eq!(
            clock.singapore_date(),
            NaiveDate::from_ymd_opt(2026, 10, 20).unwrap()
        );
        assert_eq!(
            clock.next_singapore_midnight(),
            Some(wall("2026-10-20T16:00:00Z"))
        );
        assert_eq!(calendar_boundary(&clock), at("2026-10-20T16:00:00Z"));
        // The stored boundary is the wall-clock instant, not the monotonic one.
        assert_eq!(
            calendar_boundary(&clock),
            Deadline::At(wall("2026-10-20T16:00:00Z"))
        );
        // A UTC day boundary would be eight hours late; the boundary must not
        // silently treat the UTC date as the business date.
        assert_ne!(calendar_boundary(&clock), at("2026-10-21T00:00:00Z"));
    }

    #[test]
    fn certificate_boundary_moves_at_the_next_business_midnight() {
        let clock = clock("2026-10-20T15:30:00Z");
        let today = NaiveDate::from_ymd_opt(2026, 10, 20).unwrap();
        let yesterday = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        let tomorrow = NaiveDate::from_ymd_opt(2026, 10, 21).unwrap();
        assert_eq!(certificate_boundary(None, &clock), Deadline::None);
        assert_eq!(
            certificate_boundary(Some(yesterday), &clock),
            Deadline::None
        );
        assert_eq!(
            certificate_boundary(Some(today), &clock),
            at("2026-10-20T16:00:00Z")
        );
        assert_eq!(
            certificate_boundary(Some(tomorrow), &clock),
            at("2026-10-21T16:00:00Z")
        );
    }

    #[test]
    fn policy_identity_changes_with_the_policy_or_the_gate() {
        let config = crate::Config {
            kyc_execution_gate_enabled: true,
            environment: "test".into(),
            release_sha: crate::compiled_sha().into(),
            migration_revision: "prc_20261018_cache".into(),
            database_url: "postgres://app:secret@127.0.0.1/verdaxis_test".into(),
            db_session_time_zone: "UTC".into(),
            prepared_budget: None,
            bind: "127.0.0.1:0".parse().unwrap(),
            read_admission_slots: 16,
            pool_max: 17,
            python_budget: 52,
            configured_max_connections: 100,
            compliance_eur_usd_rate: None,
            fleet_artifact_path: "data/fleet_demand.json".into(),
        };
        let policy = crate::MarketPolicy {
            source_sha: crate::compiled_sha().into(),
            products: vec![],
            points: vec![],
        };
        let first = policy_identity(&config, &policy);
        assert_eq!(first, policy_identity(&config, &policy));
        let mut changed_gate = config.clone();
        changed_gate.kyc_execution_gate_enabled = false;
        assert_ne!(first, policy_identity(&changed_gate, &policy));
        let mut changed_policy = policy.clone();
        changed_policy.source_sha = "0000".into();
        assert_ne!(first, policy_identity(&config, &changed_policy));
    }
}

//! The one admission rule for stored watchlist events.
//!
//! The feed, the acknowledgement and the container counts all filter rows with
//! the SQL relation below, inside their own statement and before `LIMIT`, the
//! read-state `UPDATE` and aggregation. No surface keeps a second copy of the
//! rule, so a row is never shown by one surface and refused by another.
//!
//! The relation reads only stored columns, the payload's source order and the
//! public order scope the caller captured on its transaction:
//!
//! 1. A `SLICE_BEST_PRICE_MOVED` row carries aggregate prices that no current
//!    order can vouch for. It is admitted only when the trusted producer wrote
//!    it (`producer = 'rust-market-v1'` with a source identity) and its scope
//!    was attested at or after the activation instant this release recorded.
//!    Without a recorded activation no best-price row is admitted. Rows of any
//!    other producer stay hidden for this class, whatever their timestamps.
//! 2. A PIN target must name an order in the public scope.
//! 3. A row whose payload names a source order is admitted only while that
//!    order is in the public scope. This applies to every class, trusted rows
//!    included. A best-price row carries no source order, so for that class the
//!    attestation and activation of rule 1 decide alone.
//!
//! Other classes keep rules 2 and 3 only, for every producer. The relation
//! never reads the producer switch: a paused producer leaves its stored rows
//! readable under the same rule.
use chrono::{DateTime, Utc};

/// The payload's source order: the `order_id` text as PostgreSQL reads a
/// UUID, else none. `pg_input_is_valid` gates the cast so a bad value never
/// fails a read. Every surface reads the source order through this one
/// expression, so a spelling that one parser reads and another does not cannot
/// make them disagree.
macro_rules! source_order_expr {
    () => {
        "(CASE WHEN pg_input_is_valid(e.event_payload->>'order_id','uuid') \
         THEN (e.event_payload->>'order_id')::uuid END)"
    };
}
pub(super) use source_order_expr;

/// The source order as a selected column, for responses that name it.
macro_rules! source_order_sql {
    () => {
        concat!(
            $crate::crud::watchlists::admission::source_order_expr!(),
            " AS source_order"
        )
    };
}
pub(super) use source_order_sql;

/// The admission relation as one boolean SQL expression over `e`
/// (`watchlist_events`) and `t` (its `watchlist_targets` row, which may be
/// absent after a target was deleted). `$visible` is the bind placeholder of
/// the public order scope (`uuid[]`); `$activation` is the bind placeholder of
/// [`EventAdmission::activation`] (`timestamptz`, NULL when none is recorded).
macro_rules! event_visible_sql {
    ($visible:literal, $activation:literal) => {
        concat!(
            "((e.event_type::text<>'SLICE_BEST_PRICE_MOVED' ",
            "OR (e.producer='rust-market-v1' ",
            "AND e.source_event_id IS NOT NULL ",
            "AND e.scope_attested_at IS NOT NULL ",
            "AND ",
            $activation,
            "::timestamptz IS NOT NULL ",
            "AND e.scope_attested_at>=",
            $activation,
            "::timestamptz)) ",
            "AND (t.target_type::text IS DISTINCT FROM 'PIN' ",
            "OR t.order_id=ANY(",
            $visible,
            "::uuid[])) ",
            "AND (",
            $crate::crud::watchlists::admission::source_order_expr!(),
            " IS NULL OR ",
            $crate::crud::watchlists::admission::source_order_expr!(),
            "=ANY(",
            $visible,
            "::uuid[])))"
        )
    };
}
pub(super) use event_visible_sql;

/// The release-recorded input of the admission relation: the instant the
/// trusted producer was activated in this environment, or none.
///
/// It is built only from the watchlist settings, which take the value from
/// release metadata. `None` binds SQL NULL, so trusted best-price rows stay
/// hidden; no clock reading or fixed instant stands in for a missing record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventAdmission {
    activation: Option<DateTime<Utc>>,
}

impl EventAdmission {
    pub fn new(activation: Option<DateTime<Utc>>) -> Self {
        Self { activation }
    }

    /// The value bound to the relation's activation placeholder.
    pub fn activation(&self) -> Option<DateTime<Utc>> {
        self.activation
    }
}

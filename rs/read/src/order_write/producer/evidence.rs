//! Typed decoder for the writer's version 1 evidence envelope.
//!
//! The envelope is internal database data written by `effects.rs` in the same
//! transaction as the economic change. These types read it; they have no
//! `Serialize` implementation and no conversion into a transport payload.

use std::collections::BTreeSet;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

/// The only envelope version this consumer reads.
pub const EVIDENCE_VERSION: u64 = 1;
/// The largest stored envelope the consumer fetches, in bytes of JSON text.
pub const MAX_EVIDENCE_BYTES: i64 = 4 * 1024 * 1024;
/// One mutation changes at most 100 crossing orders plus 100 expired orders;
/// 256 leaves room without accepting an unbounded list.
pub const MAX_ORDERS: usize = 256;
/// Two sides per affected slice, and a slice comes from an order or a declared insert.
pub const MAX_AGGREGATES: usize = 2 * MAX_ORDERS;
/// A key part longer than this cannot come from the market policy.
const MAX_IDENTITY_TEXT: usize = 64;

/// Why one trusted envelope cannot be read. The text is a fixed reason, never data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceError {
    TooLarge,
    Malformed,
    UnsupportedVersion,
    OutOfBounds,
    Inconsistent,
}

impl EvidenceError {
    pub fn reason(self) -> &'static str {
        match self {
            Self::TooLarge => "evidence_too_large",
            Self::Malformed => "evidence_malformed",
            Self::UnsupportedVersion => "evidence_version_unsupported",
            Self::OutOfBounds => "evidence_out_of_bounds",
            Self::Inconsistent => "evidence_inconsistent",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Side {
    Bid,
    Ask,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bid => "BID",
            Self::Ask => "ASK",
        }
    }
    fn parse(text: &str) -> Result<Self, EvidenceError> {
        match text {
            "BID" => Ok(Self::Bid),
            "ASK" => Ok(Self::Ask),
            _ => Err(EvidenceError::Malformed),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderStatus {
    Open,
    PartiallyFilled,
    Filled,
    Cancelled,
    Expired,
}

impl OrderStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "OPEN",
            Self::PartiallyFilled => "PARTIALLY_FILLED",
            Self::Filled => "FILLED",
            Self::Cancelled => "CANCELLED",
            Self::Expired => "EXPIRED",
        }
    }
    /// Live statuses rest on the book; the others end the order.
    pub fn is_live(self) -> bool {
        matches!(self, Self::Open | Self::PartiallyFilled)
    }
    pub fn is_terminal(self) -> bool {
        !self.is_live()
    }
    fn parse(text: &str) -> Result<Self, EvidenceError> {
        Ok(match text {
            "OPEN" => Self::Open,
            "PARTIALLY_FILLED" => Self::PartiallyFilled,
            "FILLED" => Self::Filled,
            "CANCELLED" => Self::Cancelled,
            "EXPIRED" => Self::Expired,
            _ => return Err(EvidenceError::Malformed),
        })
    }
}

/// The market slice of an order, aggregate or SLICE target.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SliceIdentity {
    pub market_product: String,
    pub delivery_point_id: Uuid,
    pub availability_window: String,
}

impl SliceIdentity {
    /// The canonical text of the slice inside a derived-event key.
    pub fn key_text(&self) -> String {
        format!(
            "{}:{}:{}",
            self.market_product, self.delivery_point_id, self.availability_window
        )
    }
}

/// One order state as the writer captured it under its locks.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderEvidence {
    pub status: OrderStatus,
    pub side: Side,
    pub market_product: Option<String>,
    pub delivery_point_id: Option<Uuid>,
    pub availability_window: String,
    pub price: Decimal,
    pub remaining: Decimal,
    pub provenance: String,
    pub public_admitted: bool,
    pub benchmark_qualified: bool,
    /// The public label of the delivery point, present only in an admitted projection.
    pub delivery_point_name: Option<String>,
    pub observed_at: Option<DateTime<Utc>>,
}

impl OrderEvidence {
    pub fn slice(&self) -> Option<SliceIdentity> {
        Some(SliceIdentity {
            market_product: self.market_product.clone()?,
            delivery_point_id: self.delivery_point_id?,
            availability_window: self.availability_window.clone(),
        })
    }
}

/// One order's change inside the mutation.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderTransition {
    pub order_id: Uuid,
    pub transition_index: u32,
    /// Absent for an order the mutation inserted.
    pub before: Option<OrderEvidence>,
    pub after: OrderEvidence,
}

/// The qualified quotes of one provenance on one side of a slice.
#[derive(Clone, Debug, PartialEq)]
pub struct ProvenanceQuotes {
    pub order_count: u64,
    pub best_price: Option<Decimal>,
    pub observed_at: Option<DateTime<Utc>>,
}

/// One side of one slice, before or after the mutation.
#[derive(Clone, Debug, PartialEq)]
pub struct SideAggregate {
    pub slice: SliceIdentity,
    pub side: Side,
    pub real: ProvenanceQuotes,
    pub demo: ProvenanceQuotes,
}

/// The evidence class a public aggregate selects: REAL first, then DEMO.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    Real,
    Demo,
    NoData,
}

impl SideAggregate {
    pub fn selection(&self) -> Selection {
        if self.real.order_count > 0 {
            Selection::Real
        } else if self.demo.order_count > 0 {
            Selection::Demo
        } else {
            Selection::NoData
        }
    }
    /// The best price of the selected evidence class; never a blend of both.
    pub fn selected_best(&self) -> Option<Decimal> {
        match self.selection() {
            Selection::Real => self.real.best_price,
            Selection::Demo => self.demo.best_price,
            Selection::NoData => None,
        }
    }
    pub fn selected_observed_at(&self) -> Option<DateTime<Utc>> {
        match self.selection() {
            Selection::Real => self.real.observed_at,
            Selection::Demo => self.demo.observed_at,
            Selection::NoData => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetKind {
    Pin,
    Slice,
}

/// A watchlist target that existed when the writer captured the mutation.
#[derive(Clone, Debug, PartialEq)]
pub struct CapturedTarget {
    pub target_id: Uuid,
    pub watchlist_id: Uuid,
    pub kind: TargetKind,
    pub order_id: Option<Uuid>,
    pub slice: SliceIdentity,
}

/// The decoded, validated envelope of one source mutation.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceEvidence {
    pub mutation_id: Uuid,
    pub captured_at: DateTime<Utc>,
    pub orders: Vec<OrderTransition>,
    pub aggregates_before: Vec<SideAggregate>,
    pub aggregates_after: Vec<SideAggregate>,
    pub targets: Vec<CapturedTarget>,
}

#[derive(Deserialize)]
struct VersionProbe {
    version: Option<u64>,
}

#[derive(Deserialize)]
struct RawEnvelope {
    mutation_id: Uuid,
    captured_at: DateTime<Utc>,
    orders: Vec<RawTransition>,
    aggregates_before: Vec<RawAggregate>,
    aggregates_after: Vec<RawAggregate>,
    targets: Vec<RawTarget>,
}

#[derive(Deserialize)]
struct RawTransition {
    order_id: Uuid,
    transition_index: u32,
    before: Option<RawOrderState>,
    after: RawOrderState,
}

#[derive(Deserialize)]
struct RawOrderState {
    status: String,
    side: String,
    market_product: Option<String>,
    delivery_point_id: Option<Uuid>,
    availability_window: String,
    price_per_mt_usd: String,
    remaining_quantity_mt: String,
    provenance: String,
    public_admitted: bool,
    benchmark_qualified: bool,
    #[serde(default)]
    public_projection: Option<Value>,
    #[serde(default)]
    observed_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct RawAggregate {
    market_product: String,
    delivery_point_id: Uuid,
    availability_window: String,
    side: String,
    real: RawQuotes,
    demo: RawQuotes,
}

#[derive(Deserialize)]
struct RawQuotes {
    order_count: u64,
    best_price_per_mt_usd: Option<String>,
    #[serde(default)]
    observed_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct RawTarget {
    target_id: Uuid,
    watchlist_id: Uuid,
    target_type: String,
    order_id: Option<Uuid>,
    market_product: String,
    delivery_point_id: Uuid,
    availability_window: String,
}

/// Decode one envelope whose stored size is `stored_bytes`. The caller fetches
/// the value only when that size is within the limit; `None` means it did not.
pub fn decode(stored_bytes: i64, value: Option<&Value>) -> Result<SourceEvidence, EvidenceError> {
    if stored_bytes > MAX_EVIDENCE_BYTES {
        return Err(EvidenceError::TooLarge);
    }
    let value = value.ok_or(EvidenceError::TooLarge)?;
    let probe = VersionProbe::deserialize(value).map_err(|_| EvidenceError::Malformed)?;
    if probe.version != Some(EVIDENCE_VERSION) {
        return Err(EvidenceError::UnsupportedVersion);
    }
    let raw = RawEnvelope::deserialize(value).map_err(|_| EvidenceError::Malformed)?;
    if raw.orders.len() > MAX_ORDERS
        || raw.aggregates_before.len() > MAX_AGGREGATES
        || raw.aggregates_after.len() > MAX_AGGREGATES
    {
        return Err(EvidenceError::OutOfBounds);
    }
    let orders = raw
        .orders
        .into_iter()
        .map(transition)
        .collect::<Result<Vec<_>, _>>()?;
    let indexes: BTreeSet<_> = orders.iter().map(|o| o.transition_index).collect();
    let order_ids: BTreeSet<_> = orders.iter().map(|o| o.order_id).collect();
    if indexes.len() != orders.len() || order_ids.len() != orders.len() {
        return Err(EvidenceError::Inconsistent);
    }
    let aggregates_before = aggregates(raw.aggregates_before)?;
    let aggregates_after = aggregates(raw.aggregates_after)?;
    // The writer computes both snapshots over one slice list, so position i is
    // the same side of the same slice before and after.
    if aggregates_before.len() != aggregates_after.len()
        || aggregates_before
            .iter()
            .zip(&aggregates_after)
            .any(|(before, after)| before.slice != after.slice || before.side != after.side)
    {
        return Err(EvidenceError::Inconsistent);
    }
    let targets = raw
        .targets
        .into_iter()
        .map(target)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SourceEvidence {
        mutation_id: raw.mutation_id,
        captured_at: raw.captured_at,
        orders,
        aggregates_before,
        aggregates_after,
        targets,
    })
}

fn decimal(text: &str) -> Result<Decimal, EvidenceError> {
    Decimal::from_str(text).map_err(|_| EvidenceError::Malformed)
}

fn identity_text(text: String) -> Result<String, EvidenceError> {
    if text.is_empty() || text.len() > MAX_IDENTITY_TEXT || text.contains(['|', ':']) {
        return Err(EvidenceError::Malformed);
    }
    Ok(text)
}

fn slice(
    market_product: String,
    delivery_point_id: Uuid,
    availability_window: String,
) -> Result<SliceIdentity, EvidenceError> {
    Ok(SliceIdentity {
        market_product: identity_text(market_product)?,
        delivery_point_id,
        availability_window: identity_text(availability_window)?,
    })
}

fn order_state(raw: RawOrderState) -> Result<OrderEvidence, EvidenceError> {
    let delivery_point_name = raw
        .public_projection
        .as_ref()
        .and_then(|projection| projection.get("delivery_point_name"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(OrderEvidence {
        status: OrderStatus::parse(&raw.status)?,
        side: Side::parse(&raw.side)?,
        market_product: raw.market_product.map(identity_text).transpose()?,
        delivery_point_id: raw.delivery_point_id,
        availability_window: identity_text(raw.availability_window)?,
        price: decimal(&raw.price_per_mt_usd)?,
        remaining: decimal(&raw.remaining_quantity_mt)?,
        provenance: raw.provenance,
        public_admitted: raw.public_admitted,
        benchmark_qualified: raw.benchmark_qualified,
        delivery_point_name,
        observed_at: raw.observed_at,
    })
}

fn transition(raw: RawTransition) -> Result<OrderTransition, EvidenceError> {
    let before = raw.before.map(order_state).transpose()?;
    let after = order_state(raw.after)?;
    // An order never changes side or market identity after insert.
    if before.as_ref().is_some_and(|before| {
        before.side != after.side
            || before.market_product != after.market_product
            || before.delivery_point_id != after.delivery_point_id
            || before.availability_window != after.availability_window
    }) {
        return Err(EvidenceError::Inconsistent);
    }
    Ok(OrderTransition {
        order_id: raw.order_id,
        transition_index: raw.transition_index,
        before,
        after,
    })
}

fn quotes(raw: RawQuotes) -> Result<ProvenanceQuotes, EvidenceError> {
    Ok(ProvenanceQuotes {
        order_count: raw.order_count,
        best_price: raw
            .best_price_per_mt_usd
            .as_deref()
            .map(decimal)
            .transpose()?,
        observed_at: raw.observed_at,
    })
}

fn aggregates(raw: Vec<RawAggregate>) -> Result<Vec<SideAggregate>, EvidenceError> {
    raw.into_iter()
        .map(|raw| {
            Ok(SideAggregate {
                slice: slice(
                    raw.market_product,
                    raw.delivery_point_id,
                    raw.availability_window,
                )?,
                side: Side::parse(&raw.side)?,
                real: quotes(raw.real)?,
                demo: quotes(raw.demo)?,
            })
        })
        .collect()
}

fn target(raw: RawTarget) -> Result<CapturedTarget, EvidenceError> {
    let kind = match raw.target_type.as_str() {
        "PIN" => TargetKind::Pin,
        "SLICE" => TargetKind::Slice,
        _ => return Err(EvidenceError::Malformed),
    };
    if (kind == TargetKind::Pin) != raw.order_id.is_some() {
        return Err(EvidenceError::Inconsistent);
    }
    Ok(CapturedTarget {
        target_id: raw.target_id,
        watchlist_id: raw.watchlist_id,
        kind,
        order_id: raw.order_id,
        slice: slice(
            raw.market_product,
            raw.delivery_point_id,
            raw.availability_window,
        )?,
    })
}

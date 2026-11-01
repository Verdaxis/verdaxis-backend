//! Pure derivation of watchlist events from one source envelope.
//!
//! Every class and payload value comes from the immutable before/after
//! evidence. Nothing here reads a database row, a current target snapshot or
//! the clock, so a delayed or repeated consumption derives the same set.
//!
//! The business rules follow the existing watchlist emitter: the PIN price
//! change threshold, the PIN quantity and terminal classes, the slice new-order
//! class, best-price movement by inequality and the quiet class. The admission
//! rule is the full public scope captured by the writer.

use std::collections::BTreeSet;
use std::str::FromStr;

use chrono::{DateTime, SecondsFormat, Utc};
use rust_decimal::Decimal;
use serde_json::{Map, Number, Value, json};
use uuid::Uuid;

use super::evidence::{
    CapturedTarget, OrderEvidence, OrderStatus, OrderTransition, Selection, SideAggregate,
    SliceIdentity, SourceEvidence, TargetKind,
};

/// A PIN price move must reach this percentage of the old price.
const PIN_PRICE_THRESHOLD_PERCENT: Decimal = Decimal::ONE;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EventClass {
    SliceNewOrder,
    SliceBestPriceMoved,
    SliceWentQuiet,
    PinPriceChanged,
    PinQuantityChanged,
    PinPartiallyFilled,
    PinFilled,
    PinWithdrawn,
    PinExpired,
}

impl EventClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SliceNewOrder => "SLICE_NEW_ORDER",
            Self::SliceBestPriceMoved => "SLICE_BEST_PRICE_MOVED",
            Self::SliceWentQuiet => "SLICE_WENT_QUIET",
            Self::PinPriceChanged => "PIN_PRICE_CHANGED",
            Self::PinQuantityChanged => "PIN_QUANTITY_CHANGED",
            Self::PinPartiallyFilled => "PIN_PARTIALLY_FILLED",
            Self::PinFilled => "PIN_FILLED",
            Self::PinWithdrawn => "PIN_WITHDRAWN",
            Self::PinExpired => "PIN_EXPIRED",
        }
    }
}

/// One watchlist event for one captured target.
#[derive(Clone, Debug, PartialEq)]
pub struct DerivedEvent {
    pub target: CapturedTarget,
    pub class: EventClass,
    /// `class|order-or-slice|side|index`, stable across replays.
    pub key: String,
    pub payload: Map<String, Value>,
}

/// Derive every distinct event of one source, in a stable order.
pub fn derive(source: &SourceEvidence) -> Vec<DerivedEvent> {
    let mut events = Vec::new();
    for transition in &source.orders {
        order_events(source, transition, &mut events);
    }
    for (index, (before, after)) in source
        .aggregates_before
        .iter()
        .zip(&source.aggregates_after)
        .enumerate()
    {
        slice_events(source, index, before, after, &mut events);
    }
    // Identical entries collapse; distinct classes, orders, sides and indexes stay.
    let mut seen = BTreeSet::new();
    events.retain(|event| seen.insert((event.target.target_id, event.key.clone())));
    events
}

/// An order-bearing class needs the captured public scope: admitted after the
/// change, or admitted before a change that ended the order.
fn admitted(transition: &OrderTransition) -> bool {
    transition.after.public_admitted
        || (transition.after.status.is_terminal()
            && transition
                .before
                .as_ref()
                .is_some_and(|before| before.public_admitted))
}

fn order_key(class: EventClass, transition: &OrderTransition) -> String {
    format!(
        "{}|{}|{}|{}",
        class.as_str(),
        transition.order_id,
        transition.after.side.as_str(),
        transition.transition_index
    )
}

fn slice_key(class: EventClass, aggregate: &SideAggregate, index: usize) -> String {
    format!(
        "{}|{}|{}|{}",
        class.as_str(),
        aggregate.slice.key_text(),
        aggregate.side.as_str(),
        index
    )
}

fn slice_targets<'a>(
    source: &'a SourceEvidence,
    slice: &'a SliceIdentity,
) -> impl Iterator<Item = &'a CapturedTarget> {
    source
        .targets
        .iter()
        .filter(move |target| target.kind == TargetKind::Slice && &target.slice == slice)
}

fn order_events(
    source: &SourceEvidence,
    transition: &OrderTransition,
    out: &mut Vec<DerivedEvent>,
) {
    if !admitted(transition) {
        return;
    }
    let after = &transition.after;
    let pins = source.targets.iter().filter(|target| {
        target.kind == TargetKind::Pin && target.order_id == Some(transition.order_id)
    });
    for target in pins {
        for (class, payload) in pin_classes(transition) {
            out.push(DerivedEvent {
                target: target.clone(),
                class,
                key: order_key(class, transition),
                payload,
            });
        }
    }
    let inserted_on_book = transition.before.is_none()
        && after.status.is_live()
        && after.public_admitted
        && after.benchmark_qualified;
    if let (true, Some(slice)) = (inserted_on_book, after.slice()) {
        let mut payload = order_payload(transition.order_id, after, None);
        payload.insert("side".into(), json!(after.side.as_str()));
        payload.insert("price_per_mt_usd".into(), number(after.price));
        payload.insert("remaining_quantity_mt".into(), number(after.remaining));
        for target in slice_targets(source, &slice) {
            out.push(DerivedEvent {
                target: target.clone(),
                class: EventClass::SliceNewOrder,
                key: order_key(EventClass::SliceNewOrder, transition),
                payload: payload.clone(),
            });
        }
    }
}

/// The price branch and the quantity/status branch are independent; both may emit.
fn pin_classes(transition: &OrderTransition) -> Vec<(EventClass, Map<String, Value>)> {
    let after = &transition.after;
    let before = transition.before.as_ref();
    let base = order_payload(transition.order_id, after, before);
    let mut classes = Vec::new();
    if let Some(before) = before
        && price_move_is_material(before.price, after.price)
    {
        let mut payload = base.clone();
        payload.insert("old_price_per_mt_usd".into(), number(before.price));
        payload.insert("new_price_per_mt_usd".into(), number(after.price));
        classes.push((EventClass::PinPriceChanged, payload));
    }
    match before {
        Some(before) if before.remaining != after.remaining => {
            let class = match after.status {
                OrderStatus::PartiallyFilled => EventClass::PinPartiallyFilled,
                OrderStatus::Filled => EventClass::PinFilled,
                OrderStatus::Cancelled => EventClass::PinWithdrawn,
                OrderStatus::Expired => EventClass::PinExpired,
                OrderStatus::Open => EventClass::PinQuantityChanged,
            };
            let mut payload = base;
            payload.insert("old_remaining_quantity_mt".into(), number(before.remaining));
            payload.insert("new_remaining_quantity_mt".into(), number(after.remaining));
            payload.insert("status".into(), json!(after.status.as_str()));
            classes.push((class, payload));
        }
        _ if before.is_none_or(|before| before.status != after.status) => {
            let class = match after.status {
                OrderStatus::Filled => Some(EventClass::PinFilled),
                OrderStatus::Cancelled => Some(EventClass::PinWithdrawn),
                OrderStatus::Expired => Some(EventClass::PinExpired),
                OrderStatus::Open | OrderStatus::PartiallyFilled => None,
            };
            if let Some(class) = class {
                let mut payload = base;
                payload.insert("status".into(), json!(after.status.as_str()));
                classes.push((class, payload));
            }
        }
        _ => {}
    }
    classes
}

/// `|new - old| / old * 100 >= 1`, compared by cross-multiplication on exact
/// decimals: no division and no float. Non-positive prices never qualify.
pub fn price_move_is_material(old: Decimal, new: Decimal) -> bool {
    if old <= Decimal::ZERO || new <= Decimal::ZERO || old == new {
        return false;
    }
    let difference = (new - old).abs();
    match (
        difference.checked_mul(Decimal::ONE_HUNDRED),
        old.checked_mul(PIN_PRICE_THRESHOLD_PERCENT),
    ) {
        (Some(scaled), Some(threshold)) => scaled >= threshold,
        // A product too large for exact decimals is a move of at least 1 %.
        _ => true,
    }
}

fn slice_events(
    source: &SourceEvidence,
    index: usize,
    before: &SideAggregate,
    after: &SideAggregate,
    out: &mut Vec<DerivedEvent>,
) {
    let old = before.selected_best();
    let new = after.selected_best();
    if old == new {
        return;
    }
    let targets: Vec<_> = slice_targets(source, &after.slice).collect();
    if targets.is_empty() {
        return;
    }
    // A newly inserted admitted and qualified order on this side attributes
    // the move to itself; any other move carries the aggregate's provenance.
    let inserted = source.orders.iter().find(|transition| {
        transition.before.is_none()
            && transition.after.status.is_live()
            && transition.after.public_admitted
            && transition.after.benchmark_qualified
            && transition.after.side == after.side
            && transition.after.slice().as_ref() == Some(&after.slice)
    });
    let mut moved = match inserted {
        Some(transition) => order_payload(transition.order_id, &transition.after, None),
        None => aggregate_payload(after),
    };
    moved.insert("side".into(), json!(after.side.as_str()));
    moved.insert("old_price_per_mt_usd".into(), optional_number(old));
    moved.insert("new_price_per_mt_usd".into(), optional_number(new));
    for target in &targets {
        out.push(DerivedEvent {
            target: (*target).clone(),
            class: EventClass::SliceBestPriceMoved,
            key: slice_key(EventClass::SliceBestPriceMoved, after, index),
            payload: moved.clone(),
        });
    }
    if old.is_none() || new.is_some() {
        return;
    }
    // The quiet source is the lowest-id order on this side that was public
    // before and ended in this mutation.
    let ended = source
        .orders
        .iter()
        .filter(|transition| {
            transition.after.status.is_terminal()
                && transition.after.side == after.side
                && transition.after.slice().as_ref() == Some(&after.slice)
                && transition
                    .before
                    .as_ref()
                    .is_some_and(|before| before.public_admitted)
        })
        .min_by_key(|transition| transition.order_id);
    let Some(ended) = ended else {
        return;
    };
    let quiet = order_payload(ended.order_id, &ended.after, ended.before.as_ref());
    for target in targets {
        out.push(DerivedEvent {
            target: target.clone(),
            class: EventClass::SliceWentQuiet,
            key: slice_key(EventClass::SliceWentQuiet, after, index),
            payload: quiet.clone(),
        });
    }
}

/// The order provenance fields of an order-bearing payload.
fn order_payload(
    order_id: Uuid,
    after: &OrderEvidence,
    before: Option<&OrderEvidence>,
) -> Map<String, Value> {
    let (source_kind, demo_status) = match after.provenance.as_str() {
        "REAL" => ("LIVE_ORDER", "REAL_ONLY"),
        "DEMO" => ("DEMO_SEED", "DEMO_ONLY"),
        _ => ("UNKNOWN", "UNKNOWN"),
    };
    // A withdrawn order has no current public projection; its label is the
    // one the writer captured while it was public.
    let point_name = after
        .delivery_point_name
        .clone()
        .or_else(|| before.and_then(|before| before.delivery_point_name.clone()));
    let mut payload = Map::new();
    payload.insert("source_kind".into(), json!(source_kind));
    payload.insert(
        "scope".into(),
        json!(if after.delivery_point_id.is_some() {
            "DELIVERY_POINT"
        } else {
            "UNKNOWN"
        }),
    );
    payload.insert("demo_status".into(), json!(demo_status));
    payload.insert("is_demo_listing".into(), json!(after.provenance == "DEMO"));
    payload.insert(
        "unknown_count".into(),
        json!(u8::from(after.provenance == "UNKNOWN")),
    );
    payload.insert("market_product".into(), json!(after.market_product));
    payload.insert("delivery_point_id".into(), json!(after.delivery_point_id));
    payload.insert(
        "availability_window".into(),
        json!(after.availability_window),
    );
    payload.insert("observed_at".into(), instant(after.observed_at));
    payload.insert("market_product_code".into(), json!(after.market_product));
    payload.insert("delivery_point_name".into(), json!(point_name));
    payload.insert(
        "availability_window_code".into(),
        json!(after.availability_window),
    );
    payload.insert("order_id".into(), json!(order_id));
    payload
}

/// The selected evidence class of the slice side after the change. The
/// shared public relation admits only REAL and DEMO rows, so no UNKNOWN row
/// is ever counted here.
fn aggregate_payload(after: &SideAggregate) -> Map<String, Value> {
    let (source_kind, demo_status) = match after.selection() {
        Selection::Real => ("LIVE_ORDER", "REAL_ONLY"),
        Selection::Demo => ("DEMO_SEED", "DEMO_ONLY"),
        Selection::NoData => ("NO_DATA", "NOT_APPLICABLE"),
    };
    let mut payload = Map::new();
    payload.insert("source_kind".into(), json!(source_kind));
    payload.insert("demo_status".into(), json!(demo_status));
    payload.insert("scope".into(), json!("DELIVERY_POINT"));
    payload.insert("observed_at".into(), instant(after.selected_observed_at()));
    payload.insert(
        "market_product_code".into(),
        json!(after.slice.market_product),
    );
    payload.insert(
        "delivery_point_id".into(),
        json!(after.slice.delivery_point_id),
    );
    payload.insert(
        "availability_window_code".into(),
        json!(after.slice.availability_window),
    );
    payload.insert("real_count".into(), json!(after.real.order_count));
    payload.insert("demo_count".into(), json!(after.demo.order_count));
    payload.insert("unknown_count".into(), json!(0));
    payload
}

fn instant(value: Option<DateTime<Utc>>) -> Value {
    value.map_or(Value::Null, |value| {
        json!(value.to_rfc3339_opts(SecondsFormat::AutoSi, false))
    })
}

/// An exact JSON number with the decimal's own digits; no float conversion.
pub fn number(value: Decimal) -> Value {
    let text = value.normalize().to_string();
    Number::from_str(&text).map_or(Value::Null, Value::Number)
}

fn optional_number(value: Option<Decimal>) -> Value {
    value.map_or(Value::Null, number)
}

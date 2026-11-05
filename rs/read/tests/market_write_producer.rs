#![cfg(feature = "contract-tests")]
//! Pure producer rules: envelope decoding, event derivation and activation.
//! No database; every case goes through the same decoder the consumer uses.

use std::collections::BTreeSet;
use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::{Value, json};
use uuid::Uuid;
use verdaxis_read::market_contract::producer::{
    ActivationRefusal, DerivedEvent, EvidenceError, MAX_EVIDENCE_BYTES, MAX_ORDERS,
    ProducerActivation, decode, derive, number, price_move_is_material, retry_delay,
    schema_has_producer,
};

const PRODUCT: &str = "SCALAR_FUEL";
const RELEASE: &str = "0123456789abcdef0123456789abcdef01234567";

fn point() -> Uuid {
    Uuid::from_u128(0xd0)
}
fn order(n: u128) -> Uuid {
    Uuid::from_u128(0x100 + n)
}
fn pin(n: u128) -> Uuid {
    Uuid::from_u128(0x200 + n)
}
fn slice_target() -> Uuid {
    Uuid::from_u128(0x300)
}

struct State {
    status: &'static str,
    side: &'static str,
    price: &'static str,
    remaining: &'static str,
    admitted: bool,
    qualified: bool,
    provenance: &'static str,
}

fn live(price: &'static str, remaining: &'static str) -> State {
    State {
        status: "OPEN",
        side: "ASK",
        price,
        remaining,
        admitted: true,
        qualified: true,
        provenance: "REAL",
    }
}

impl State {
    fn with_status(mut self, status: &'static str) -> Self {
        self.status = status;
        self
    }
    fn hidden(mut self) -> Self {
        self.admitted = false;
        self.qualified = false;
        self
    }
    fn side(mut self, side: &'static str) -> Self {
        self.side = side;
        self
    }
    fn value(&self) -> Value {
        json!({"status":self.status,"side":self.side,"market_product":PRODUCT,
            "delivery_point_id":point(),"availability_window":"SPOT",
            "price_per_mt_usd":self.price,"quantity_mt":"10","remaining_quantity_mt":self.remaining,
            "provenance":self.provenance,"public_admitted":self.admitted,
            "benchmark_qualified":self.qualified,
            "public_projection":if self.admitted {json!({"delivery_point_name":"Rotterdam"})} else {Value::Null},
            "observed_at":"2026-10-30T10:00:00Z"})
    }
}

fn transition(id: Uuid, index: u32, before: Option<State>, after: State) -> Value {
    json!({"order_id":id,"transition_index":index,
        "before":before.map(|b| b.value()),"after":after.value()})
}

/// One side aggregate: (real count, real best), (demo count, demo best).
fn aggregate(side: &str, real: (u64, Option<&str>), demo: (u64, Option<&str>)) -> Value {
    json!({"market_product":PRODUCT,"delivery_point_id":point(),"availability_window":"SPOT",
        "side":side,"real_count":real.0,"demo_count":demo.0,
        "real":{"order_count":real.0,"best_price_per_mt_usd":real.1,"observed_at":"2026-10-30T09:00:00Z"},
        "demo":{"order_count":demo.0,"best_price_per_mt_usd":demo.1,"observed_at":"2026-10-30T08:00:00Z"},
        // The combined value blends both classes; the consumer must never read it.
        "best_price_per_mt_usd":"1"})
}

fn pin_target(target: Uuid, order: Uuid) -> Value {
    json!({"target_id":target,"watchlist_id":Uuid::from_u128(0x400),"observer_id":Uuid::from_u128(0x500),
        "target_type":"PIN","order_id":order,"market_product":PRODUCT,"delivery_point_id":point(),
        "availability_window":"SPOT"})
}

fn slice() -> Value {
    json!({"target_id":slice_target(),"watchlist_id":Uuid::from_u128(0x401),"observer_id":Uuid::from_u128(0x500),
        "target_type":"SLICE","order_id":null,"market_product":PRODUCT,"delivery_point_id":point(),
        "availability_window":"SPOT"})
}

fn envelope(
    orders: Vec<Value>,
    before: Vec<Value>,
    after: Vec<Value>,
    targets: Vec<Value>,
) -> Value {
    json!({"version":1,"mutation_id":Uuid::from_u128(0x900),"captured_at":"2026-10-30T10:00:00.5Z",
        "orders":orders,"aggregates_before":before,"aggregates_after":after,"targets":targets})
}

fn derived(value: &Value) -> Vec<DerivedEvent> {
    let bytes = i64::try_from(value.to_string().len()).unwrap();
    derive(&decode(bytes, Some(value)).expect("valid envelope"))
}

fn classes(events: &[DerivedEvent]) -> Vec<(Uuid, &'static str)> {
    events
        .iter()
        .map(|e| (e.target.target_id, e.class.as_str()))
        .collect()
}

fn decimal(text: &str) -> Decimal {
    Decimal::from_str(text).unwrap()
}

fn amend(before: State, after: State) -> Vec<DerivedEvent> {
    derived(&envelope(
        vec![transition(order(1), 0, Some(before), after)],
        vec![],
        vec![],
        vec![pin_target(pin(1), order(1))],
    ))
}

#[test]
fn pin_price_threshold_is_inclusive_one_percent() {
    let base = decimal("100.00");
    assert!(!price_move_is_material(base, decimal("100.99")));
    assert!(price_move_is_material(base, decimal("101.00")));
    assert!(price_move_is_material(base, decimal("101.01")));
    assert!(price_move_is_material(base, decimal("99.00")));
    assert!(!price_move_is_material(base, decimal("99.01")));
    assert!(!price_move_is_material(Decimal::ZERO, decimal("5")));
    assert!(!price_move_is_material(base, Decimal::ZERO));
    assert!(!price_move_is_material(decimal("-1"), decimal("5")));
    assert!(!price_move_is_material(base, base));
    // Through the derivation: 0.99 % is quiet, 1 % emits.
    assert!(amend(live("100", "10"), live("100.99", "10")).is_empty());
    assert_eq!(
        classes(&amend(live("100", "10"), live("101", "10"))),
        vec![(pin(1), "PIN_PRICE_CHANGED")]
    );
}

#[test]
fn pin_quantity_class_follows_the_new_status() {
    for (status, class) in [
        ("OPEN", "PIN_QUANTITY_CHANGED"),
        ("PARTIALLY_FILLED", "PIN_PARTIALLY_FILLED"),
        ("FILLED", "PIN_FILLED"),
        ("CANCELLED", "PIN_WITHDRAWN"),
        ("EXPIRED", "PIN_EXPIRED"),
    ] {
        let after = live("100", "4").with_status(status);
        let after = if status == "OPEN" || status == "PARTIALLY_FILLED" {
            after
        } else {
            after.hidden()
        };
        let events = amend(live("100", "10"), after);
        assert_eq!(classes(&events), vec![(pin(1), class)], "{status}");
        assert_eq!(
            events[0].payload["old_remaining_quantity_mt"].to_string(),
            "10"
        );
        assert_eq!(
            events[0].payload["new_remaining_quantity_mt"].to_string(),
            "4"
        );
        assert_eq!(events[0].payload["status"], status);
    }
}

#[test]
fn terminal_status_without_remainder_change_emits_status_only() {
    let events = amend(
        live("100", "10"),
        live("100", "10").with_status("CANCELLED").hidden(),
    );
    assert_eq!(classes(&events), vec![(pin(1), "PIN_WITHDRAWN")]);
    assert_eq!(events[0].payload["status"], "CANCELLED");
    assert!(events[0].payload.get("old_remaining_quantity_mt").is_none());
    // A live status change without a remainder change emits nothing.
    assert!(
        amend(
            live("100", "10"),
            live("100", "10").with_status("PARTIALLY_FILLED")
        )
        .is_empty()
    );
}

#[test]
fn non_admitted_evidence_emits_no_order_event() {
    // UNKNOWN provenance, an incomplete ASK, expired terms, owner rejection
    // and KYC loss all reach the writer's capture as "not publicly admitted".
    for after in [
        live("110", "8").hidden(),
        live("100", "10").with_status("PARTIALLY_FILLED").hidden(),
    ] {
        assert!(amend(live("100", "10").hidden(), after).is_empty());
    }
    // An order that leaves the public scope while still live derives nothing.
    assert!(amend(live("100", "10"), live("110", "8").hidden()).is_empty());
    // An inserted order outside the public scope derives no new-order event.
    let mut unknown = live("95", "10");
    unknown.provenance = "UNKNOWN";
    let events = derived(&envelope(
        vec![transition(order(2), 0, None, unknown.hidden())],
        vec![aggregate("ASK", (1, Some("100")), (0, None))],
        vec![aggregate("ASK", (1, Some("100")), (0, None))],
        vec![slice()],
    ));
    assert!(events.is_empty());
}

#[test]
fn admitted_before_withdrawal_still_emits() {
    let events = amend(
        live("100", "10"),
        live("100", "10").with_status("EXPIRED").hidden(),
    );
    assert_eq!(classes(&events), vec![(pin(1), "PIN_EXPIRED")]);
    // The label comes from the public projection captured before the change.
    assert_eq!(events[0].payload["delivery_point_name"], "Rotterdam");
    // A terminal change of an order that was never public emits nothing.
    assert!(
        amend(
            live("100", "10").hidden(),
            live("100", "10").with_status("CANCELLED").hidden()
        )
        .is_empty()
    );
}

#[test]
fn best_price_uses_real_before_demo_and_never_the_combined_value() {
    // DEMO best moves but REAL is selected: no event.
    let unchanged = derived(&envelope(
        vec![],
        vec![aggregate("ASK", (1, Some("100")), (1, Some("80")))],
        vec![aggregate("ASK", (1, Some("100")), (1, Some("70")))],
        vec![slice()],
    ));
    assert!(unchanged.is_empty());
    // REAL disappears: the selection falls to DEMO and the best moves 100 -> 70.
    let events = derived(&envelope(
        vec![],
        vec![aggregate("ASK", (1, Some("100")), (1, Some("70")))],
        vec![aggregate("ASK", (0, None), (1, Some("70")))],
        vec![slice()],
    ));
    assert_eq!(
        classes(&events),
        vec![(slice_target(), "SLICE_BEST_PRICE_MOVED")]
    );
    let payload = &events[0].payload;
    assert_eq!(payload["old_price_per_mt_usd"].to_string(), "100");
    assert_eq!(payload["new_price_per_mt_usd"].to_string(), "70");
    assert_eq!(payload["source_kind"], "DEMO_SEED");
    assert_eq!(payload["demo_status"], "DEMO_ONLY");
    assert_eq!(payload["observed_at"], "2026-10-30T08:00:00+00:00");
    assert_eq!(payload["real_count"], 0);
    assert_eq!(payload["demo_count"], 1);
    assert_eq!(payload["unknown_count"], 0);
}

#[test]
fn best_price_moves_by_inequality_on_each_side() {
    // Any change of the selected best emits, with no threshold, on either side.
    let events = derived(&envelope(
        vec![],
        vec![
            aggregate("BID", (1, Some("90")), (0, None)),
            aggregate("ASK", (1, Some("100")), (0, None)),
        ],
        vec![
            aggregate("BID", (1, Some("90.01")), (0, None)),
            aggregate("ASK", (1, Some("100.00")), (0, None)),
        ],
        vec![slice()],
    ));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].payload["side"], "BID");
    assert_eq!(
        events[0].key,
        format!("SLICE_BEST_PRICE_MOVED|{PRODUCT}:{}:SPOT|BID|0", point())
    );
}

#[test]
fn quiet_needs_a_terminal_order_on_that_side() {
    let base = |orders| {
        derived(&envelope(
            orders,
            vec![aggregate("ASK", (1, Some("100")), (0, None))],
            vec![aggregate("ASK", (0, None), (0, None))],
            vec![slice()],
        ))
    };
    // The quote left the scope while live: best-price only.
    let hidden = base(vec![transition(
        order(1),
        0,
        Some(live("100", "10")),
        live("100", "10").hidden(),
    )]);
    assert_eq!(
        classes(&hidden),
        vec![(slice_target(), "SLICE_BEST_PRICE_MOVED")]
    );
    // A terminal order on the other side does not make this side quiet.
    let other_side = base(vec![transition(
        order(1),
        0,
        Some(live("90", "10").side("BID")),
        live("90", "10")
            .side("BID")
            .with_status("CANCELLED")
            .hidden(),
    )]);
    assert_eq!(
        classes(&other_side),
        vec![(slice_target(), "SLICE_BEST_PRICE_MOVED")]
    );
    // Two ended quotes: the lowest order id is the quiet source.
    let ended = base(vec![
        transition(
            order(2),
            0,
            Some(live("100", "10")),
            live("100", "10").with_status("CANCELLED").hidden(),
        ),
        transition(
            order(1),
            1,
            Some(live("101", "10")),
            live("101", "10").with_status("EXPIRED").hidden(),
        ),
    ]);
    assert_eq!(
        classes(&ended),
        vec![
            (slice_target(), "SLICE_BEST_PRICE_MOVED"),
            (slice_target(), "SLICE_WENT_QUIET"),
        ]
    );
    assert_eq!(ended[1].payload["order_id"], order(1).to_string());
    assert_eq!(ended[0].payload["source_kind"], "NO_DATA");
}

#[test]
fn new_quote_emits_new_order_and_attributed_best_price() {
    let events = derived(&envelope(
        vec![transition(order(3), 0, None, live("95", "10"))],
        vec![aggregate("ASK", (1, Some("100")), (0, None))],
        vec![aggregate("ASK", (2, Some("95")), (0, None))],
        vec![slice()],
    ));
    assert_eq!(
        classes(&events),
        vec![
            (slice_target(), "SLICE_NEW_ORDER"),
            (slice_target(), "SLICE_BEST_PRICE_MOVED"),
        ]
    );
    assert_eq!(events[0].payload["price_per_mt_usd"].to_string(), "95");
    assert_eq!(events[1].payload["order_id"], order(3).to_string());
    assert_eq!(events[1].payload["source_kind"], "LIVE_ORDER");
    assert_eq!(
        events[1].payload["observed_at"],
        "2026-10-30T10:00:00+00:00"
    );
    // A new order that is public but not qualified derives no new-order event.
    let mut unqualified = live("95", "10");
    unqualified.qualified = false;
    let events = derived(&envelope(
        vec![transition(order(3), 0, None, unqualified)],
        vec![aggregate("ASK", (1, Some("100")), (0, None))],
        vec![aggregate("ASK", (1, Some("100")), (0, None))],
        vec![slice()],
    ));
    assert!(events.is_empty());
}

#[test]
fn no_benchmark_class_is_derived() {
    // A benchmark shift alone (same best price) derives nothing.
    let mut before = aggregate("ASK", (2, Some("100")), (0, None));
    let mut after = aggregate("ASK", (2, Some("100")), (0, None));
    before["benchmark_price_per_mt_usd"] = json!("100");
    after["benchmark_price_per_mt_usd"] = json!("150");
    let events = derived(&envelope(vec![], vec![before], vec![after], vec![slice()]));
    assert!(events.is_empty());
    let all = amend(
        live("100", "10"),
        live("150", "1").with_status("PARTIALLY_FILLED"),
    );
    assert!(
        all.iter()
            .all(|e| e.class.as_str() != "SLICE_BENCHMARK_MOVED")
    );
}

#[test]
fn derived_keys_are_stable_and_distinct() {
    let envelope = envelope(
        vec![
            transition(order(1), 0, Some(live("100", "10")), live("110", "8")),
            transition(order(2), 1, Some(live("100", "10")), live("100", "6")),
        ],
        vec![],
        vec![],
        vec![pin_target(pin(1), order(1)), pin_target(pin(2), order(2))],
    );
    let first = derived(&envelope);
    let second = derived(&envelope);
    assert_eq!(first, second);
    let keys: Vec<&str> = first.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(
        keys,
        vec![
            format!("PIN_PRICE_CHANGED|{}|ASK|0", order(1)),
            format!("PIN_QUANTITY_CHANGED|{}|ASK|0", order(1)),
            format!("PIN_QUANTITY_CHANGED|{}|ASK|1", order(2)),
        ]
    );
    let distinct: BTreeSet<_> = first.iter().map(|e| (e.target.target_id, &e.key)).collect();
    assert_eq!(distinct.len(), first.len());
    assert!(first.iter().all(|e| e.key.len() <= 255));
}

#[test]
fn identical_transitions_collapse() {
    // One target captured twice for the same order yields each key once.
    let events = derived(&envelope(
        vec![transition(
            order(1),
            0,
            Some(live("100", "10")),
            live("110", "8"),
        )],
        vec![],
        vec![],
        vec![pin_target(pin(1), order(1)), pin_target(pin(1), order(1))],
    ));
    assert_eq!(events.len(), 2);
}

#[test]
fn envelope_limits_refuse_before_decoding() {
    assert_eq!(
        decode(MAX_EVIDENCE_BYTES + 1, None),
        Err(EvidenceError::TooLarge)
    );
    // A stored size over the limit refuses even if a value were present.
    let small = envelope(vec![], vec![], vec![], vec![]);
    assert_eq!(
        decode(MAX_EVIDENCE_BYTES + 1, Some(&small)),
        Err(EvidenceError::TooLarge)
    );
    let orders: Vec<Value> = (0..=MAX_ORDERS as u128)
        .map(|n| transition(order(n), n as u32, Some(live("1", "1")), live("1", "1")))
        .collect();
    let large = envelope(orders, vec![], vec![], vec![]);
    assert_eq!(decode(1024, Some(&large)), Err(EvidenceError::OutOfBounds));
    // Duplicate transition indexes and mismatched aggregate positions are inconsistent.
    let duplicate = envelope(
        vec![
            transition(order(1), 0, None, live("1", "1")),
            transition(order(2), 0, None, live("1", "1")),
        ],
        vec![],
        vec![],
        vec![],
    );
    assert_eq!(
        decode(1024, Some(&duplicate)),
        Err(EvidenceError::Inconsistent)
    );
    let mismatched = envelope(
        vec![],
        vec![aggregate("ASK", (0, None), (0, None))],
        vec![aggregate("BID", (0, None), (0, None))],
        vec![],
    );
    assert_eq!(
        decode(1024, Some(&mismatched)),
        Err(EvidenceError::Inconsistent)
    );
    let malformed = json!({"version":1,"orders":"x"});
    assert_eq!(decode(64, Some(&malformed)), Err(EvidenceError::Malformed));
}

#[test]
fn unsupported_envelope_version_is_invalid() {
    let mut value = envelope(vec![], vec![], vec![], vec![]);
    value["version"] = json!(2);
    assert_eq!(
        decode(256, Some(&value)),
        Err(EvidenceError::UnsupportedVersion)
    );
    value.as_object_mut().unwrap().remove("version");
    assert_eq!(
        decode(256, Some(&value)),
        Err(EvidenceError::UnsupportedVersion)
    );
    assert_eq!(
        decode(8, Some(&json!("text"))),
        Err(EvidenceError::Malformed)
    );
}

#[test]
fn activation_requires_both_release_keys() {
    let instant = Some("2026-11-01T08:00:00+08:00");
    let active =
        ProducerActivation::from_release(Some("rust-market-v1"), instant, RELEASE).unwrap();
    assert_eq!(
        active.activated_at(),
        "2026-11-01T00:00:00Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap()
    );
    assert_eq!(active.release_sha(), RELEASE);
    for (producer, recorded, release, refusal) in [
        (None, instant, RELEASE, ActivationRefusal::ProducerMissing),
        (
            Some(""),
            instant,
            RELEASE,
            ActivationRefusal::ProducerMissing,
        ),
        (
            Some("legacy-python"),
            instant,
            RELEASE,
            ActivationRefusal::LegacyProducer,
        ),
        (
            Some("rust-market-v2"),
            instant,
            RELEASE,
            ActivationRefusal::UnknownProducer,
        ),
        (
            Some("rust-market-v1"),
            None,
            RELEASE,
            ActivationRefusal::ActivatedAtMissing,
        ),
        (
            Some("rust-market-v1"),
            Some(" "),
            RELEASE,
            ActivationRefusal::ActivatedAtMissing,
        ),
        (
            Some("rust-market-v1"),
            Some("2026-11-01"),
            RELEASE,
            ActivationRefusal::ActivatedAtInvalid,
        ),
        (
            Some("rust-market-v1"),
            Some("now"),
            RELEASE,
            ActivationRefusal::ActivatedAtInvalid,
        ),
        (
            Some("rust-market-v1"),
            instant,
            "test",
            ActivationRefusal::ReleaseInvalid,
        ),
        (
            Some("rust-market-v1"),
            instant,
            "0123456789ABCDEF0123456789ABCDEF01234567",
            ActivationRefusal::ReleaseInvalid,
        ),
    ] {
        assert_eq!(
            ProducerActivation::from_release(producer, recorded, release),
            Err(refusal),
            "{producer:?} {recorded:?} {release}"
        );
    }
}

#[test]
fn producer_is_inactive_when_activation_is_missing() {
    // Without both release keys there is no activation value, and the
    // consumer transaction cannot be called without one.
    assert!(ProducerActivation::from_release(None, None, RELEASE).is_err());
    assert!(schema_has_producer("p89_20261022_watchlist_producer"));
    assert!(schema_has_producer("p89_20261022_trade_freeze"));
    assert!(schema_has_producer("ob_20261031_offboarding"));
    assert!(schema_has_producer("sqlx_1"));
    assert!(!schema_has_producer("p5_20261020_read_indexes"));
    assert!(!schema_has_producer("unknown_revision"));
}

#[test]
fn payload_numbers_are_exact_json_numbers() {
    let precise = "123456789.123456789";
    assert_eq!(number(decimal(precise)).to_string(), precise);
    assert!(number(decimal(precise)).is_number());
    assert_eq!(number(decimal("100.00")).to_string(), "100");
    assert_eq!(number(decimal("0.10")).to_string(), "0.1");
    let events = amend(live("100.10", "10.5"), live("120.25", "7.125"));
    let price = &events[0].payload;
    assert!(price["new_price_per_mt_usd"].is_number());
    assert_eq!(price["new_price_per_mt_usd"].to_string(), "120.25");
    assert_eq!(price["old_price_per_mt_usd"].to_string(), "100.1");
    let quantity = &events[1].payload;
    assert_eq!(quantity["new_remaining_quantity_mt"].to_string(), "7.125");
    let text = Value::Object(quantity.clone()).to_string();
    assert!(
        text.contains("\"new_remaining_quantity_mt\":7.125"),
        "{text}"
    );
}

#[test]
fn order_payload_carries_the_order_provenance_fields() {
    let events = amend(live("100", "10"), live("110", "10"));
    let payload = &events[0].payload;
    for (field, value) in [
        ("source_kind", json!("LIVE_ORDER")),
        ("scope", json!("DELIVERY_POINT")),
        ("demo_status", json!("REAL_ONLY")),
        ("is_demo_listing", json!(false)),
        ("unknown_count", json!(0)),
        ("market_product", json!(PRODUCT)),
        ("market_product_code", json!(PRODUCT)),
        ("delivery_point_id", json!(point())),
        ("delivery_point_name", json!("Rotterdam")),
        ("availability_window", json!("SPOT")),
        ("availability_window_code", json!("SPOT")),
        ("observed_at", json!("2026-10-30T10:00:00+00:00")),
        ("order_id", json!(order(1))),
    ] {
        assert_eq!(payload[field], value, "{field}");
    }
}

#[test]
fn old_values_from_a_non_public_state_are_not_disclosed() {
    // A hidden order becomes public with a new price and remainder: its hidden
    // price and remainder are never named, so no PIN class carries them.
    assert!(amend(live("100", "10").hidden(), live("110", "8")).is_empty());
    assert!(
        amend(
            live("100", "10").hidden(),
            live("110", "8").with_status("PARTIALLY_FILLED")
        )
        .is_empty()
    );
    // The same change of a public order keeps both classes and old values.
    let public = amend(live("100", "10"), live("110", "8"));
    assert_eq!(public.len(), 2);
    assert_eq!(public[0].payload["old_price_per_mt_usd"].to_string(), "100");
    // A newly public order still reaches SLICE targets through the new-order
    // and best-price rules, which carry no hidden old value.
    let events = derived(&envelope(
        vec![transition(
            order(1),
            0,
            Some(live("100", "10").hidden()),
            live("95", "10"),
        )],
        vec![aggregate("ASK", (1, Some("100")), (0, None))],
        vec![aggregate("ASK", (2, Some("95")), (0, None))],
        vec![pin_target(pin(1), order(1)), slice()],
    ));
    assert_eq!(
        classes(&events),
        vec![(slice_target(), "SLICE_BEST_PRICE_MOVED")]
    );
    assert_eq!(events[0].payload["old_price_per_mt_usd"].to_string(), "100");
}

#[test]
fn retry_delay_backs_off_and_is_bounded() {
    use std::time::Duration;
    assert_eq!(retry_delay(0), Duration::from_secs(2));
    assert_eq!(retry_delay(1), Duration::from_secs(2));
    assert_eq!(retry_delay(2), Duration::from_secs(4));
    assert_eq!(retry_delay(3), Duration::from_secs(8));
    assert_eq!(retry_delay(4), Duration::from_secs(16));
    assert_eq!(retry_delay(5), Duration::from_secs(30));
    assert_eq!(retry_delay(u32::MAX), Duration::from_secs(30));
}

/// The producer reads the outbox and never writes or sequences it.
#[test]
fn producer_sources_never_write_or_sequence_the_outbox() {
    let sources = [
        include_str!("../src/order_write/producer/mod.rs"),
        include_str!("../src/order_write/producer/consume.rs"),
        include_str!("../src/order_write/producer/derive.rs"),
        include_str!("../src/order_write/producer/evidence.rs"),
    ];
    let sequencer_key = ["7264537260226625", "7_264_537_260_226_625"];
    let advisory = ["pg_", "advisory"].concat();
    for text in sources {
        let words = text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        for verb in ["insert into", "update", "delete from"] {
            assert!(
                !words.contains(&format!("{verb} market_event_outbox")),
                "{verb} on the outbox"
            );
        }
        assert!(!words.contains("stream_seq ="), "assigns a sequence value");
        assert!(!words.contains("nextval"), "allocates a sequence value");
        assert!(!words.contains(&advisory), "takes an advisory lock");
        for key in sequencer_key {
            assert!(!text.contains(key), "names the sequencer key");
        }
    }
}

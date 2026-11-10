//! The single order-book write boundary, `rs/read/src/order_write/`.
//!
//! The runtime database role holds full DML on the order-book tables
//! (`deploy/postgres/app_acl_policy.sql`), so PostgreSQL does not enforce the
//! boundary. One source check keeps every order-book and trade write, and the
//! acceptance-ordinal sequence, in the listed boundary files. The other cases
//! test the boundary's lock keys, request hashes and idempotency keys.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use regex::Regex;
use uuid::Uuid;

use verdaxis_read::order_write::{
    Canonical, CommandContext, CommittedCommand, IDEMPOTENCY_KEY_MAX, Operation, OrderCommand,
    SliceKey, command_request_hash, execute_order_command, idempotency_lock_key,
    idempotency_request_hash, market_slice_lock_key, market_slice_lock_sort_key,
    normalize_idempotency_key, normalize_slice_window, ordered_slice_lock_keys,
};

const PRODUCT: &str = "10000000-0000-4000-8000-0000000000a1";
const POINT: &str = "20000000-0000-4000-8000-0000000000b1";

fn uuid(text: &str) -> Uuid {
    Uuid::parse_str(text).expect("fixture uuid")
}

// ---------------------------------------------------------------------------
// The source boundary.
// ---------------------------------------------------------------------------

/// The files, relative to `rs/read`, that may write `orderbook_*` or `trades`
/// rows or draw from the acceptance-ordinal sequence. All are in the boundary.
const ORDER_BOOK_WRITERS: &[&str] = &[
    "src/order_write/amend.rs",
    "src/order_write/invalidation.rs",
    "src/order_write/inventory.rs",
    "src/order_write/matching.rs",
    "src/order_write/order_create.rs",
    "src/order_write/orders.rs",
    "src/order_write/reassignment.rs",
    "src/order_write/trade_lifecycle.rs",
    "src/order_write/trades.rs",
    "src/order_write/transaction.rs",
    "src/order_write/transaction/system/demo.rs",
];

fn source_files(directory: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            source_files(&path, found);
        } else if matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("rs" | "sql")
        ) {
            found.push(path);
        }
    }
}

#[test]
fn order_book_writes_stay_in_the_listed_boundary_files() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let order_book_write = Regex::new(
        r"(?i)\b(?:(?:INSERT\s+INTO|UPDATE|DELETE\s+FROM|TRUNCATE(?:\s+TABLE)?)\s+(?:public\.)?(?:orderbook_\w+|trades)\b|orderbook_acceptance_ordinal_seq)",
    )
    .unwrap();
    let mut files = Vec::new();
    for directory in ["src", "sql", "examples"] {
        source_files(&crate_root.join(directory), &mut files);
    }
    let writers: BTreeSet<String> = files
        .iter()
        .filter(|path| order_book_write.is_match(&std::fs::read_to_string(path).unwrap()))
        .map(|path| {
            path.strip_prefix(crate_root)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let allowed: BTreeSet<String> = ORDER_BOOK_WRITERS
        .iter()
        .map(|path| (*path).to_owned())
        .collect();
    assert_eq!(
        writers, allowed,
        "order-book writes must stay in the reviewed order_write files"
    );
}

// ---------------------------------------------------------------------------
// Lock keys.
// ---------------------------------------------------------------------------

#[test]
fn one_slice_identity_takes_one_lock_key() {
    let product = uuid(PRODUCT);
    let spot = normalize_slice_window("SPOT").unwrap();
    // Window aliases and padding name the same slice.
    assert_eq!(normalize_slice_window(" Spot ").unwrap(), spot);
    assert_eq!(
        normalize_slice_window("Q1_2026").unwrap(),
        normalize_slice_window("2026-Q1").unwrap()
    );
    let key = market_slice_lock_key(product, None, &spot);
    assert_eq!(market_slice_lock_key(product, None, &spot), key);
    // Each part of the identity changes the key.
    assert_ne!(
        market_slice_lock_key(product, Some(uuid(POINT)), &spot),
        key
    );
    let month = normalize_slice_window("2027-03").unwrap();
    assert_ne!(market_slice_lock_key(product, None, &month), key);
    assert_ne!(market_slice_lock_key(uuid(POINT), None, &spot), key);
}

#[test]
fn slice_locks_are_taken_in_one_order_without_duplicates() {
    let slice = |point: Option<&str>, window: &str| SliceKey {
        product_id: uuid(PRODUCT),
        delivery_point_id: point.map(uuid),
        availability_window: window.to_owned(),
    };
    let slices = vec![
        slice(None, "2026-Q1"),
        slice(Some(POINT), "SPOT"),
        slice(Some(POINT), "Spot"),
        slice(None, "2027-CAL"),
        slice(None, "SPOT"),
    ];
    let keys = ordered_slice_lock_keys(&slices).unwrap();

    // One key per slice identity, acquired in sort-key order.
    let mut expected: Vec<((String, String, String), i64)> = slices
        .iter()
        .map(|slice| {
            let window = normalize_slice_window(&slice.availability_window).unwrap();
            (
                market_slice_lock_sort_key(slice.product_id, slice.delivery_point_id, &window),
                market_slice_lock_key(slice.product_id, slice.delivery_point_id, &window),
            )
        })
        .collect();
    expected.sort();
    expected.dedup();
    assert_eq!(
        keys,
        expected.iter().map(|(_, key)| *key).collect::<Vec<_>>()
    );
    assert_eq!(keys.len(), 4);

    // Input order and repeats do not change the acquisition order.
    let mut reversed = slices.clone();
    reversed.reverse();
    assert_eq!(ordered_slice_lock_keys(&reversed).unwrap(), keys);
    let mut doubled = slices.clone();
    doubled.extend(slices.iter().cloned());
    assert_eq!(ordered_slice_lock_keys(&doubled).unwrap(), keys);
}

#[test]
fn idempotency_lock_keys_are_scoped_by_owner_operation_and_key() {
    let scope = uuid("30000000-0000-4000-8000-0000000000c1");
    let key = idempotency_lock_key(scope, "trade.confirm", "retry-key");
    assert_eq!(
        idempotency_lock_key(scope, "trade.confirm", "retry-key"),
        key
    );
    assert_ne!(
        idempotency_lock_key(uuid(PRODUCT), "trade.confirm", "retry-key"),
        key
    );
    assert_ne!(
        idempotency_lock_key(scope, "trade.decline", "retry-key"),
        key
    );
    assert_ne!(
        idempotency_lock_key(scope, "trade.confirm", "other-key"),
        key
    );
}

// ---------------------------------------------------------------------------
// Request hashes and idempotency keys.
// ---------------------------------------------------------------------------

#[test]
fn receipt_operations_are_the_six_order_and_trade_commands() {
    let names: BTreeSet<&str> = Operation::ALL
        .iter()
        .map(|operation| operation.as_str())
        .collect();
    assert_eq!(names.len(), 6);
    assert_eq!(Operation::OrderAmend.resource_type(), "order");
    assert_eq!(Operation::OrderCancel.resource_type(), "order");
    for operation in [
        Operation::TradeConfirm,
        Operation::TradeDecline,
        Operation::TradeDeliver,
        Operation::TradePay,
    ] {
        assert_eq!(operation.resource_type(), "trade");
    }
}

#[test]
fn request_hashes_are_stable_and_distinguish_payloads() {
    let amend = |price: &str| {
        Canonical::Object(BTreeMap::from([
            (
                "operation".to_owned(),
                Canonical::Text("order.amend".to_owned()),
            ),
            ("order_id".to_owned(), Canonical::Uuid(uuid(PRODUCT))),
            (
                "price_per_mt_usd".to_owned(),
                Canonical::Decimal(price.parse().unwrap()),
            ),
        ]))
    };
    let hash = command_request_hash(&amend("750.00"));
    assert_eq!(hash.len(), 64);
    assert!(hash.chars().all(|character| character.is_ascii_hexdigit()));
    assert_eq!(command_request_hash(&amend("750.00")), hash);
    assert_ne!(command_request_hash(&amend("751.00")), hash);

    let retry = |key: &str| {
        Canonical::Object(BTreeMap::from([(
            "key".to_owned(),
            Canonical::Text(key.to_owned()),
        )]))
    };
    let idempotency = idempotency_request_hash(&retry("a")).unwrap();
    assert_eq!(idempotency_request_hash(&retry("a")).unwrap(), idempotency);
    assert_ne!(idempotency_request_hash(&retry("b")).unwrap(), idempotency);
}

#[test]
fn command_timestamps_keep_microsecond_precision() {
    let payload = Canonical::Timestamp(
        chrono::DateTime::parse_from_rfc3339("2026-10-19T00:00:00.123000Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    );
    assert_eq!(
        verdaxis_read::order_write::python_json(&payload),
        "\"2026-10-19T00:00:00.123000Z\""
    );
}

#[test]
fn idempotency_hash_refuses_typed_values() {
    let payload = Canonical::Object(BTreeMap::from([
        ("order_id".to_owned(), Canonical::Uuid(uuid(PRODUCT))),
        (
            "price".to_owned(),
            Canonical::Decimal("750.00".parse().unwrap()),
        ),
    ]));
    assert!(idempotency_request_hash(&payload).is_err());
    let counted = Canonical::Object(BTreeMap::from([(
        "quantity".to_owned(),
        Canonical::Int(serde_json::json!(10).as_i64().unwrap().into()),
    )]));
    assert!(idempotency_request_hash(&counted).is_ok());
}

#[test]
fn idempotency_keys_are_trimmed_and_bounded_by_characters() {
    assert_eq!(normalize_idempotency_key("  padded  ").unwrap(), "padded");
    assert_eq!(normalize_idempotency_key("ключ-1").unwrap(), "ключ-1");
    assert!(normalize_idempotency_key("").is_err());
    assert!(normalize_idempotency_key("   ").is_err());
    assert!(normalize_idempotency_key(&"a".repeat(IDEMPOTENCY_KEY_MAX)).is_ok());
    assert!(normalize_idempotency_key(&"a".repeat(IDEMPOTENCY_KEY_MAX + 1)).is_err());
    // Bounded by characters, not bytes.
    let multibyte = "ü".repeat(IDEMPOTENCY_KEY_MAX);
    assert_eq!(multibyte.len(), IDEMPOTENCY_KEY_MAX * 2);
    assert!(normalize_idempotency_key(&multibyte).is_ok());
}

// ---------------------------------------------------------------------------
// The transaction shell.
// ---------------------------------------------------------------------------

/// The public entry point exists and keeps its reviewed signature.
#[test]
fn the_entry_point_signature_is_the_reviewed_one() {
    async fn call(
        state: &verdaxis_read::AppState,
        context: CommandContext,
        command: OrderCommand,
    ) -> Result<CommittedCommand, verdaxis_read::ApiError> {
        execute_order_command(state, context, command).await
    }
    let _ = call;
}

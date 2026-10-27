//! Exact money and quantity primitives, and the single SQLSTATE mapping
//! (Phases 8-9 plan §6.1, §8.5 and §11.2; Task 2).
//!
//! The test reaches the real boundary leaves through test-local module
//! declarations. The production module list is an integration-owner file, so
//! this test declares the three files it needs instead of editing it, and the
//! declarations keep working after the wiring lands. Each assertion names the
//! behaviour the plan fixes: the bounds and the stored scale, the one rounding
//! call site, checked overflow, the total SQLSTATE table and the error envelope.

#[allow(dead_code)]
#[path = "../src/order_write/authority.rs"]
mod authority;
#[allow(dead_code)]
#[path = "../src/order_write/decimal.rs"]
mod decimal;
#[allow(dead_code)]
#[path = "../src/order_write/error.rs"]
mod error;
#[allow(dead_code)]
#[path = "../src/order_write/locks.rs"]
mod locks;
mod market {
    pub use verdaxis_read::market::windows;
}
pub use verdaxis_read::{ApiError, ApiErrorKind};

/// The authority projection reads the verified access through this path.
mod auth {
    pub use verdaxis_read::auth::*;
}

use std::borrow::Cow;
use std::cmp::Ordering;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use rust_decimal::Decimal;

use authority::{AuthenticatedActor, SupportContextRef, WriteContext};
use decimal::{
    AggregateMoney, CommissionPercent, FeePerMt, Money, OrderQuantity, OrderRemainder, Price,
    StockQuantity, TradeQuantity, checked_multiply, cmp_percentage, quantize_cents,
};
use error::{MarketErrorCode, MarketWriteError};

fn exact(text: &str) -> Decimal {
    Decimal::from_str_exact(text).expect("a finite literal")
}

fn assert_refusal<T: std::fmt::Debug>(result: Result<T, MarketWriteError>, code: MarketErrorCode) {
    let error = result.expect_err("the domain must refuse this value");
    assert_eq!(error.code, code, "unexpected code: {error:?}");
    assert!(!error.message.is_empty(), "every refusal carries text");
}

/// A database failure without a live server. The SQLSTATE and the constraint
/// name are the only inputs the one mapping reads.
#[derive(Debug)]
struct FixtureDatabaseError {
    message: &'static str,
    code: Option<&'static str>,
    constraint: Option<&'static str>,
}

impl std::fmt::Display for FixtureDatabaseError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(self.message)
    }
}

impl std::error::Error for FixtureDatabaseError {}

impl sqlx::error::DatabaseError for FixtureDatabaseError {
    fn message(&self) -> &str {
        self.message
    }

    fn code(&self) -> Option<Cow<'_, str>> {
        self.code.map(Cow::Borrowed)
    }

    fn constraint(&self) -> Option<&str> {
        self.constraint
    }

    fn kind(&self) -> sqlx::error::ErrorKind {
        sqlx::error::ErrorKind::Other
    }

    fn as_error(&self) -> &(dyn std::error::Error + 'static + Send + Sync) {
        self
    }

    fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + 'static + Send + Sync) {
        self
    }

    fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + 'static + Send + Sync> {
        self
    }
}

fn sqlstate_error(
    code: Option<&'static str>,
    constraint: Option<&'static str>,
) -> MarketWriteError {
    MarketWriteError::from(sqlx::Error::Database(Box::new(FixtureDatabaseError {
        message: "server text that must never reach a client",
        code,
        constraint,
    })))
}

#[test]
fn decimal_rejects_float_exponent_and_scale() {
    // An exponent is a JSON-legal spelling of a float; no stored economic value
    // may use one, because the parser would then decide a scale by rounding.
    for text in ["1e3", "1E3", "1e-3", "10.5e2"] {
        assert_refusal(
            OrderQuantity::parse_exact(text),
            MarketErrorCode::QuantityScaleInvalid,
        );
    }
    // A leading plus, whitespace, a separator, a bare or trailing point,
    // negative zero and an empty literal are all refused.
    for text in [
        "+1.5", " 1.5", "1.5 ", "1,5", "1.", ".5", "", "1.2.3", "-0.00", "-0",
    ] {
        assert_refusal(
            OrderQuantity::parse_exact(text),
            MarketErrorCode::QuantityScaleInvalid,
        );
    }
    // The scale is the literal's own fraction length: a value that is exactly
    // representable but stores three decimals is refused, never rounded.
    assert_refusal(
        OrderQuantity::parse_exact("1.000"),
        MarketErrorCode::QuantityScaleInvalid,
    );
    assert_refusal(
        Price::parse_exact("1.234"),
        MarketErrorCode::PriceScaleInvalid,
    );
    assert_refusal(
        Money::parse_exact("1.001"),
        MarketErrorCode::ValueScaleInvalid,
    );

    // Two fraction digits parse exactly, and the serde path reads the same
    // literal text, so a JSON number never becomes an f64 on the way in.
    assert_eq!(
        OrderQuantity::parse_exact("0.5").unwrap().value(),
        Decimal::new(5, 1)
    );
    let precise: OrderQuantity = serde_json::from_str("0.07").expect("exact number");
    assert_eq!(precise.value().to_string(), "0.07");
    assert!(serde_json::from_str::<OrderQuantity>("1.25").is_ok());
    assert!(serde_json::from_str::<OrderQuantity>("1e3").is_err());
    assert!(serde_json::from_str::<OrderQuantity>("1.234").is_err());
    assert!(serde_json::from_str::<OrderQuantity>("\"1.25\"").is_err());
    assert!(serde_json::from_str::<OrderQuantity>("null").is_err());
}

#[test]
fn quantize_cents_is_half_up_at_midpoint() {
    // Midpoint away from zero, not banker's rounding: 10.005 and 10.025 both
    // round up, where nearest-even would give 10.00 and 10.02.
    for (raw, expected) in [
        ("10.005", "10.01"),
        ("10.015", "10.02"),
        ("10.025", "10.03"),
        ("0.005", "0.01"),
        ("1.004", "1.00"),
        ("-10.005", "-10.01"),
    ] {
        assert_eq!(
            quantize_cents(exact(raw)),
            exact(expected),
            "rounding {raw}"
        );
    }
    assert_eq!(quantize_cents(exact("9999000")), exact("9999000"));
}

#[test]
fn quantity_and_price_bounds_are_enforced() {
    assert_refusal(
        OrderQuantity::parse_exact("0"),
        MarketErrorCode::QuantityOutOfRange,
    );
    assert_refusal(
        OrderQuantity::parse_exact("-0.01"),
        MarketErrorCode::QuantityOutOfRange,
    );
    assert_refusal(
        OrderQuantity::parse_exact("100000.01"),
        MarketErrorCode::QuantityOutOfRange,
    );
    assert!(OrderQuantity::parse_exact("0.01").is_ok());
    assert!(OrderQuantity::parse_exact("100000").is_ok());

    assert_refusal(Price::parse_exact("0"), MarketErrorCode::PriceOutOfRange);
    assert_refusal(
        Price::parse_exact("1000000.01"),
        MarketErrorCode::PriceOutOfRange,
    );
    assert!(Price::parse_exact("1000000").is_ok());

    assert!(Money::parse_exact("100000000000").is_ok());
    assert_refusal(
        Money::parse_exact("100000000000.01"),
        MarketErrorCode::ValueOutOfRange,
    );
    assert!(AggregateMoney::parse_exact("10000000000000").is_ok());

    assert!(FeePerMt::parse_exact("100000").is_ok());
    assert_refusal(
        FeePerMt::parse_exact("100000.01"),
        MarketErrorCode::FeeRateInvalid,
    );

    // The legacy percentage keeps three stored fraction digits and no more.
    assert!(CommissionPercent::parse_exact("0.125").is_ok());
    assert!(CommissionPercent::parse_exact("100").is_ok());
    assert_refusal(
        CommissionPercent::parse_exact("0.1254"),
        MarketErrorCode::CommissionRateInvalid,
    );

    // A stored remainder outside its domain is inconsistent state, not a
    // request validation failure; stock stays non-negative.
    assert!(OrderRemainder::parse_exact("100000").is_ok());
    assert_refusal(
        OrderRemainder::parse_exact("0.001"),
        MarketErrorCode::InventoryInconsistent,
    );
    assert_refusal(
        StockQuantity::parse_exact("-0.01"),
        MarketErrorCode::QuantityOutOfRange,
    );

    assert_eq!(
        MarketErrorCode::QuantityOutOfRange.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        MarketErrorCode::InventoryInconsistent.status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        MarketErrorCode::ValueOverflow.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[test]
fn zero_stock_and_filled_remainder_are_valid() {
    // The named case: zero is inside the non-negative domains. Inventory is
    // created at zero stock and a filled order has a zero remainder.
    assert!(StockQuantity::parse_exact("0.00").is_ok());
    assert!(OrderRemainder::parse_exact("0.00").is_ok());
    assert!(StockQuantity::checked(Decimal::ZERO).is_ok());
    assert!(OrderRemainder::checked(Decimal::ZERO).is_ok());
    assert_eq!(
        StockQuantity::parse_exact("0").unwrap().value(),
        Decimal::ZERO
    );

    // The positive domains stay positive, so an order or trade cannot be zero.
    assert_refusal(
        OrderQuantity::parse_exact("0.00"),
        MarketErrorCode::QuantityOutOfRange,
    );
    assert_refusal(
        TradeQuantity::parse_exact("0"),
        MarketErrorCode::QuantityOutOfRange,
    );
    // A remainder can never go below zero.
    assert_refusal(
        OrderRemainder::parse_exact("-0.01"),
        MarketErrorCode::InventoryInconsistent,
    );
}

#[test]
fn maximum_delivery_product_and_both_fee_bases_are_exact() {
    let quantity = OrderQuantity::parse_exact("100000.00").expect("maximum quantity");
    let price = Price::parse_exact("1000000.00").expect("maximum price");
    let total = quantize_cents(
        checked_multiply(quantity.value(), price.value()).expect("the largest product is in range"),
    );
    assert_eq!(total, Decimal::from(100_000_000_000i64));
    assert!(Money::checked(total).is_ok());

    // Per-MT fee basis: quantity times a frozen fee rate.
    let fee = FeePerMt::parse_exact("99.99").expect("fee rate");
    let per_mt = quantize_cents(checked_multiply(quantity.value(), fee.value()).expect("fee"));
    assert_eq!(per_mt, exact("9999000.00"));
    assert!(Money::checked(per_mt).is_ok());

    // Legacy percentage basis, applied to the rounded total. Dividing by one
    // hundred is a scale shift, so it stays exact.
    let percent = CommissionPercent::parse_exact("0.125").expect("legacy percent");
    let legacy = quantize_cents(
        checked_multiply(total, percent.value()).expect("legacy commission") / Decimal::ONE_HUNDRED,
    );
    assert_eq!(legacy, Decimal::from(125_000_000i64));

    // The ten-percent delivery deviation is decided by cross-multiplication:
    // exactly ten percent is accepted, one hundredth above is not.
    assert_eq!(
        cmp_percentage(Decimal::from(10), Decimal::from(100), Decimal::from(10)).unwrap(),
        Ordering::Equal
    );
    assert_eq!(
        cmp_percentage(exact("10.01"), Decimal::from(100), Decimal::from(10)).unwrap(),
        Ordering::Greater
    );
    assert_eq!(
        cmp_percentage(exact("-10.00"), Decimal::from(100), Decimal::from(10)).unwrap(),
        Ordering::Equal
    );

    // One cent past a domain bound is refused; nothing is truncated into it.
    assert_refusal(
        Money::checked(total + Decimal::new(1, 2)),
        MarketErrorCode::ValueOutOfRange,
    );
}

#[test]
fn checked_arithmetic_refuses_overflow_at_the_bound() {
    // The coefficient really can overflow, so the refusal is observable.
    assert_eq!(Decimal::MAX.checked_mul(Decimal::from(2)), None);
    let error = checked_multiply(Decimal::MAX, Decimal::from(2))
        .expect_err("overflow is refused, never truncated or panicked");
    assert_eq!(error.code, MarketErrorCode::ValueOverflow);
    assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);

    // The largest permitted product is exact and far inside the coefficient.
    let product = checked_multiply(exact("100000"), exact("1000000")).expect("bounded product");
    assert_eq!(product, Decimal::from(100_000_000_000i64));
    // A sum of the bounded batch stays exact as well.
    assert_eq!(
        checked_multiply(product, Decimal::from(100)).expect("batch sum"),
        Decimal::from(10_000_000_000_000i64)
    );
}

#[test]
fn sqlstate_mapping_table_is_total() {
    for (sqlstate, constraint, status, code) in [
        (
            Some("40001"),
            None,
            StatusCode::SERVICE_UNAVAILABLE,
            MarketErrorCode::TransactionRetryExhausted,
        ),
        (
            Some("40P01"),
            None,
            StatusCode::SERVICE_UNAVAILABLE,
            MarketErrorCode::TransactionRetryExhausted,
        ),
        (
            Some("55P03"),
            None,
            StatusCode::CONFLICT,
            MarketErrorCode::MarketBusy,
        ),
        (
            Some("57014"),
            None,
            StatusCode::SERVICE_UNAVAILABLE,
            MarketErrorCode::MarketTimeout,
        ),
        // 23505 follows plan §11.2: only the named idempotency-key constraints
        // and the one active freeze map to their own code; every other unique
        // violation is an unclassified database failure.
        (
            Some("23505"),
            Some("uq_market_command_results_actor_operation_key"),
            StatusCode::CONFLICT,
            MarketErrorCode::IdempotencyConflict,
        ),
        (
            Some("23505"),
            Some("uq_orderbook_orders_org_operation_idempotency"),
            StatusCode::CONFLICT,
            MarketErrorCode::IdempotencyConflict,
        ),
        (
            Some("23505"),
            Some("uq_trades_initiator_operation_idempotency"),
            StatusCode::CONFLICT,
            MarketErrorCode::IdempotencyConflict,
        ),
        (
            Some("23505"),
            Some("uq_trade_freezes_one_active"),
            StatusCode::CONFLICT,
            MarketErrorCode::TradeAlreadyFrozen,
        ),
        (
            Some("23505"),
            Some("uq_orderbook_orders_acceptance_ordinal"),
            StatusCode::INTERNAL_SERVER_ERROR,
            MarketErrorCode::DatabaseError,
        ),
        (
            Some("23505"),
            Some("uq_orderbook_support_authorization"),
            StatusCode::INTERNAL_SERVER_ERROR,
            MarketErrorCode::DatabaseError,
        ),
        (
            Some("23505"),
            Some("some_other_unique_key"),
            StatusCode::INTERNAL_SERVER_ERROR,
            MarketErrorCode::DatabaseError,
        ),
        (
            Some("23514"),
            None,
            StatusCode::UNPROCESSABLE_ENTITY,
            MarketErrorCode::ValidationFailed,
        ),
        (
            Some("ZF001"),
            None,
            StatusCode::CONFLICT,
            MarketErrorCode::TradeFrozen,
        ),
        (
            Some("ZF002"),
            None,
            StatusCode::CONFLICT,
            MarketErrorCode::FreezeHistoryImmutable,
        ),
        (
            Some("ZF003"),
            None,
            StatusCode::CONFLICT,
            MarketErrorCode::FreezeHistoryImmutable,
        ),
        (
            Some("XX000"),
            None,
            StatusCode::INTERNAL_SERVER_ERROR,
            MarketErrorCode::DatabaseError,
        ),
        (
            Some(""),
            None,
            StatusCode::INTERNAL_SERVER_ERROR,
            MarketErrorCode::DatabaseError,
        ),
        (
            None,
            None,
            StatusCode::INTERNAL_SERVER_ERROR,
            MarketErrorCode::DatabaseError,
        ),
    ] {
        let error = sqlstate_error(sqlstate, constraint);
        assert_eq!(error.code, code, "SQLSTATE {sqlstate:?}");
        assert_eq!(error.status, status, "SQLSTATE {sqlstate:?}");
        assert_eq!(error.message, code.message(), "SQLSTATE {sqlstate:?}");
    }

    // A failure that never reached the server is a plain internal error and
    // never carries the protocol text.
    let protocol = MarketWriteError::from(sqlx::Error::Protocol("connection reset".into()));
    assert_eq!(protocol.code, MarketErrorCode::DatabaseError);
    assert_eq!(protocol.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!protocol.message.contains("connection reset"));

    // Only a serialization failure or a deadlock is retryable.
    for (sqlstate, retry) in [
        (Some("40001"), true),
        (Some("40P01"), true),
        (Some("55P03"), false),
        (Some("57014"), false),
        (Some("23505"), false),
        (Some("23514"), false),
        (Some("ZF001"), false),
        (Some("XX000"), false),
        (None, false),
    ] {
        assert_eq!(
            sqlstate_error(sqlstate, None).is_retryable(),
            retry,
            "SQLSTATE {sqlstate:?}"
        );
    }

    // Totality: every §11.3 code and the three §6.1 value codes are declared,
    // once, with a status and clear text.
    let mut expected = vec![
        "AUTH_CREDENTIALS_INVALID",
        "AUTH_PASSWORD_CHANGED",
        "AUTH_SESSION_REVOKED",
        "EXECUTION_NOT_AVAILABLE",
        "KYC_REQUIRED",
        "TRADE_PARTY_FORBIDDEN",
        "TRADE_PARTIES_INVALID",
        "TRADE_NOT_FOUND",
        "MARKET_BUSY",
        "MARKET_TIMEOUT",
        "IDEMPOTENCY_BUSY",
        "IDEMPOTENCY_CONFLICT",
        "IF_MATCH_MALFORMED",
        "IF_MATCH_REQUIRED",
        "RECEIPT_RESOURCE_MISMATCH",
        "REPLAY_PRINCIPAL_MISMATCH",
        "COMMIT_OUTCOME_UNKNOWN",
        "TRANSACTION_RETRY_EXHAUSTED",
        "ORDER_TERMINAL",
        "ORDER_NOT_FOUND",
        "ORDER_OWNER_FORBIDDEN",
        "ORDER_VERSION_STALE",
        "MARKET_SUPPORT_CONTEXT_INVALID",
        "NOT_FOUND",
        "ORDER_TERMS_REVIEW_REQUIRED",
        "LNG_TERMS_INCOMPATIBLE",
        "ORDER_UNAVAILABLE",
        "QUANTITY_EXCEEDS_REMAINING",
        "QUANTITY_OUT_OF_RANGE",
        "QUANTITY_SCALE_INVALID",
        "PRICE_OUT_OF_RANGE",
        "PRICE_SCALE_INVALID",
        "FEE_RATE_INVALID",
        "COMMISSION_RATE_INVALID",
        "INVENTORY_INCONSISTENT",
        "INVENTORY_RESERVED",
        "INVENTORY_IN_USE",
        "INVENTORY_REFERENCED",
        "INVENTORY_NOT_FOUND",
        "PORT_NOT_FOUND",
        "TRADE_NOT_PENDING",
        "TRADE_NOT_DELIVERED",
        "TRADE_FROZEN",
        "TRADE_ALREADY_FROZEN",
        "TRADE_NOT_FROZEN",
        "TRADE_NOT_SETTLEABLE",
        "FREEZE_HISTORY_IMMUTABLE",
        "DELIVERY_QUANTITY_INVALID",
        "DELIVERY_PRICE_DEVIATION",
        "ORDER_FIELD_NULL",
        "ORDER_IDENTITY_IMMUTABLE",
        "MATCH_FANOUT_EXCEEDED",
        "MARKET_SLICE_CHANGED",
        "POST_ONLY_WOULD_CROSS",
        "POST_ONLY_CHECK_INDETERMINATE",
        "MARKET_SUPPORT_LEGACY_MUTATION_RETIRED",
        "FORBIDDEN",
        "MARKET_SUPPORT_CAPABILITY_REQUIRED",
        "MARKET_SUPPORT_TARGET_INVALID",
        "CAPABILITY_ASSIGNMENT_NOT_FOUND",
        "ORGANIZATION_NOT_FOUND",
        "ORGANIZATION_NOT_APPROVED",
        "ORGANIZATION_NOT_REAL",
        "SUPPORT_CONTEXT_INVALID",
        "MARKET_SUPPORT_CONTEXT_REPLACEMENT_REQUIRED",
        "MARKET_SUPPORT_IDENTITIES_UNAVAILABLE",
        "VALIDATION_FAILED",
        "DATABASE_ERROR",
        "VALUE_OUT_OF_RANGE",
        "VALUE_SCALE_INVALID",
        "VALUE_OVERFLOW",
    ];
    let mut declared: Vec<&str> = MarketErrorCode::ALL
        .iter()
        .map(|code| code.as_str())
        .collect();
    declared.sort_unstable();
    expected.sort_unstable();
    assert_eq!(declared, expected);
    let count = declared.len();
    declared.dedup();
    assert_eq!(declared.len(), count, "every code is declared once");
    for code in MarketErrorCode::ALL {
        assert!(!code.message().is_empty(), "{} has no text", code.as_str());
        assert!(
            code.status().is_client_error() || code.status().is_server_error(),
            "{} has no status",
            code.as_str()
        );
        assert!(
            code.as_str()
                .chars()
                .all(|character| character.is_ascii_uppercase() || character == '_'),
            "{} is not an upper-snake code",
            code.as_str()
        );
    }

    // The pinned statuses the plan and Python both fix: a retired support
    // body is gone (410), and a post-only order that would cross the book is
    // a conflict (409), the same as its indeterminate sibling.
    assert_eq!(
        MarketErrorCode::MarketSupportLegacyMutationRetired.status(),
        StatusCode::GONE
    );
    assert_eq!(
        MarketErrorCode::PostOnlyWouldCross.status(),
        StatusCode::CONFLICT
    );
    // A missing inventory record and a missing port are each a 404 with its
    // own code.
    assert_eq!(
        MarketErrorCode::InventoryNotFound.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        MarketErrorCode::PortNotFound.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn error_body_is_detail_code_message() {
    let response = MarketWriteError::new(MarketErrorCode::MarketBusy).into_response();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("a bounded body");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("JSON body");
    let top = value.as_object().expect("an object body");
    assert_eq!(top.len(), 1, "the body carries only `detail`");
    let detail = top["detail"].as_object().expect("a detail object");
    assert_eq!(
        detail.keys().collect::<Vec<_>>(),
        ["code", "message"],
        "exactly the two declared fields"
    );
    assert_eq!(detail["code"], "MARKET_BUSY");
    assert!(
        detail["message"]
            .as_str()
            .is_some_and(|text| !text.is_empty())
    );

    // A database refusal never leaks its SQLSTATE or the server's text.
    let error = sqlstate_error(Some("23514"), None);
    let body = axum::body::to_bytes(error.into_response().into_body(), 64 * 1024)
        .await
        .expect("a bounded body");
    let text = String::from_utf8(body.to_vec()).expect("UTF-8 body");
    assert!(text.contains("VALIDATION_FAILED"), "{text}");
    assert!(!text.contains("23514"), "{text}");
    assert!(!text.contains("server text"), "{text}");
}

#[test]
fn market_write_authority_declares_the_shared_context_without_a_constructor() {
    // The three declarations exist and can be named by a later task.
    fn actor(_: &AuthenticatedActor) {}
    fn support(_: &SupportContextRef) {}
    fn context(_: &WriteContext) {}
    let _ = (actor, support, context);

    // PD89-12: private constructors and no construction path. A value that any
    // caller can mint is not authority evidence, so the module declares no
    // constructor, default or conversion for them.
    let source = include_str!("../src/order_write/authority.rs");
    for forbidden in [
        "fn new",
        "fn from",
        "impl Default",
        "impl From<",
        "Default for",
    ] {
        assert!(
            !source.contains(forbidden),
            "authority.rs adds a construction path: {forbidden}"
        );
    }
}

#[test]
fn support_context_refusals_carry_clearing_headers() {
    use axum::response::IntoResponse;
    for code in MarketErrorCode::ALL {
        let response = MarketWriteError::new(*code).into_response();
        let clears = code.as_str().starts_with("MARKET_SUPPORT_CONTEXT_");
        assert_eq!(
            response
                .headers()
                .contains_key("x-verdaxis-market-support-context-invalid"),
            clears
        );
        assert_eq!(
            response
                .headers()
                .contains_key("x-verdaxis-market-support-context-code"),
            clears
        );
        if clears {
            assert_eq!(
                response.headers()["x-verdaxis-market-support-context-invalid"],
                "true"
            );
            assert_eq!(
                response.headers()["x-verdaxis-market-support-context-code"],
                code.as_str()
            );
        }
    }
}

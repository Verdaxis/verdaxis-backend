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
mod auth {
    pub use verdaxis_read::auth::*;
}
pub use verdaxis_read::{ApiError, ApiErrorKind};

use decimal::{
    AggregateMoney, CommissionPercent, FeePerMt, Money, OrderQuantity, OrderRemainder, Price,
    StockQuantity, TradeQuantity, checked_add, checked_subtract, quantize_cents,
};
use proptest::{
    collection::vec,
    prelude::*,
    strategy::ValueTree,
    test_runner::{Config, RngAlgorithm, TestRng, TestRunner},
};
use rust_decimal::Decimal;

fn generated_sequence(seed: u64) -> Vec<(u8, u16)> {
    let mut bytes = [0_u8; 32];
    bytes[..8].copy_from_slice(&seed.to_le_bytes());
    let config = Config {
        failure_persistence: None,
        ..Config::default()
    };
    let mut runner = TestRunner::new_with_rng(config, TestRng::from_seed(RngAlgorithm::ChaCha, &bytes));
    vec((0_u8..12, 1_u16..1000), 40)
        .new_tree(&mut runner)
        .expect("bounded sequence")
        .current()
}

#[test]
fn generated_sequences_replay_from_their_seed() {
    for seed in [1, 17, 43, 1024, 65537, u64::MAX] {
        assert_eq!(generated_sequence(seed), generated_sequence(seed), "seed={seed}");
    }
    assert_ne!(generated_sequence(1), generated_sequence(17));
}

#[test]
fn conservation_suite_uses_phase_eight_interfaces() {
    let source = include_str!("market_write_pg_conservation.rs");
    for forbidden in [
        concat!("market_", "matching"),
        concat!("market_contract::", "matching"),
        concat!("routes::", "trades"),
    ] {
        assert!(!source.contains(forbidden), "unexpected interface: {forbidden}");
    }
}

#[test]
fn decimal_roundtrip_property() {
    macro_rules! check_domain {
        ($domain:ty, $maximum:expr, $scale:expr, $positive:expr) => {{
            let maximum: i64 = $maximum;
            let scale: u32 = $scale;
            let first = if $positive { 1 } else { 0 };
            let limit = maximum * 10_i64.pow(scale);
            for coefficient in [first, 1, limit / 2, limit - 1, limit] {
                let input = Decimal::new(coefficient, scale);
                let text = input.to_string();
                let parsed = <$domain>::parse_exact(&text).expect(&text).value();
                assert_eq!(parsed, input, "{}: {text}", stringify!($domain));
                assert_eq!(<$domain>::parse_exact(&parsed.to_string()).unwrap().value(), input);
                assert!(<$domain>::parse_exact(&format!("{text}0")).is_err(), "{}: {text}", stringify!($domain));
            }
        }};
    }
    check_domain!(OrderQuantity, 100_000, 2, true);
    check_domain!(TradeQuantity, 100_000, 2, true);
    check_domain!(OrderRemainder, 100_000, 2, false);
    check_domain!(StockQuantity, 100_000, 2, false);
    check_domain!(Price, 1_000_000, 2, true);
    check_domain!(Money, 100_000_000_000, 2, false);
    check_domain!(AggregateMoney, 10_000_000_000_000, 2, false);
    check_domain!(FeePerMt, 100_000, 2, false);
    check_domain!(CommissionPercent, 100, 3, false);
}

#[test]
fn quantization_and_stock_arithmetic_are_exact() {
    for cents in 0..10_000_i64 {
        let midpoint = Decimal::new(cents * 10 + 5, 3);
        let rounded = quantize_cents(midpoint);
        assert_eq!(rounded, Decimal::new(cents + 1, 2));
        assert_eq!(quantize_cents(rounded), rounded);

        let stock = Decimal::new(cents, 2);
        let delta = Decimal::new(cents / 2, 2);
        let sum = checked_add(stock, delta).unwrap();
        assert_eq!(checked_subtract(sum, delta).unwrap(), stock);
        assert_eq!(sum.scale(), 2);
    }
}

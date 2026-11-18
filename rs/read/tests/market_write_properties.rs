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
#[allow(dead_code)]
#[path = "../src/order_write/party_policy.rs"]
mod party_policy;
mod kyc {
    pub use verdaxis_read::kyc::policy;
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

#[test]
fn decimal_roundtrip_property() {
    macro_rules! check_domain {
        ($domain:ty, $maximum:expr, $scale:expr, $positive:expr) => {{
            let maximum: i64 = $maximum;
            let scale: u32 = $scale;
            let first = if $positive { 1 } else { 0 };
            let limit = maximum * 10_i64.pow(scale);
            let mut runner = TestRunner::new_with_rng(
                Config {
                    failure_persistence: None,
                    ..Config::default()
                },
                TestRng::from_seed(RngAlgorithm::ChaCha, &[37; 32]),
            );
            let generated = vec(first..=limit, 1024)
                .new_tree(&mut runner)
                .unwrap()
                .current();
            for coefficient in [first, 1, limit / 2, limit - 1, limit]
                .into_iter()
                .chain(generated)
            {
                let input = Decimal::new(coefficient, scale);
                let text = input.to_string();
                let parsed = <$domain>::parse_exact(&text).expect(&text).value();
                assert_eq!(parsed, input, "{}: {text}", stringify!($domain));
                assert_eq!(
                    <$domain>::parse_exact(&parsed.to_string()).unwrap().value(),
                    input
                );
                assert!(
                    <$domain>::parse_exact(&format!("{text}0")).is_err(),
                    "{}: {text}",
                    stringify!($domain)
                );
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

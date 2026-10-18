//! Exact economic values remain independent of decimal wire spelling.
use std::cmp::Ordering;
use verdaxis_read::market::amount::{trade_total, Amount};

fn amount(text: &str) -> Amount {
    Amount::parse(text).unwrap()
}

#[test]
fn decimal_replacement_preserves_value_and_admission() {
    let wide = amount("123456789012345678901234567890.123456789");
    assert_eq!(
        wide.compare(&amount("123456789012345678901234567890.1234567890")),
        Ordering::Equal
    );
    assert_eq!(amount("-0.00").compare(&amount("0")), Ordering::Equal);
    assert!(Amount::parse("1e-1999999999999999997").is_ok());
    assert!(Amount::parse("1e-1999999999999999998").is_err());
    assert!(Amount::parse("1e1000000000000000000").is_err());
    for invalid in ["NaN", "Infinity", "", "1.2.3", "--1"] {
        assert!(Amount::parse(invalid).is_err());
    }
}

#[test]
fn domain_rounding_keeps_half_up_display_and_intermediate_value() {
    assert_eq!(amount("1.005").quantize(2).unwrap(), "1.01");
    assert_eq!(amount("-1.005").quantize(2).unwrap(), "-1.01");
    let rounded = amount("9999999999999999999999999999").add(&amount("0.5"));
    assert_eq!(
        rounded
            .subtract(&amount("9999999999999999999999999999"))
            .compare(&amount("1")),
        Ordering::Equal
    );
    assert_eq!(
        amount("1")
            .divide(&amount("3"))
            .unwrap()
            .compare(&amount("0.3333333333333333333333333333")),
        Ordering::Equal
    );
    assert!(amount("1").divide(&amount("0")).is_err());
}

#[test]
fn public_spelling_is_simple_and_trade_products_keep_economic_rounding() {
    assert_eq!(amount("750.00").text(), "75e1");
    assert_eq!(amount("-0.00").text(), "0");
    assert_eq!(amount("1e-10000").text(), "1e-10000");
    assert_eq!(
        amount(&trade_total("99999999999999999999999999995").unwrap())
            .compare(&amount("100000000000000000000000000000")),
        Ordering::Equal
    );
}

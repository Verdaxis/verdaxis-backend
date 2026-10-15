use rust_decimal::Decimal;
use std::str::FromStr;
use verdaxis_read::routes::curves_canonical::curve_projection::midpoint;

#[test]
fn canonical_midpoint_requires_both_sides_and_rounds_half_even() {
    let decimal = |value| Decimal::from_str(value).unwrap();
    assert_eq!(
        midpoint(Some(decimal("1.00")), Some(decimal("1.01"))),
        Some(decimal("1.00"))
    );
    assert_eq!(
        midpoint(Some(decimal("1.01")), Some(decimal("1.02"))),
        Some(decimal("1.02"))
    );
    assert_eq!(midpoint(Some(decimal("1.00")), None), None);
}

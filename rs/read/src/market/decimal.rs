//! Decimal wire text for the reviewed-terms digest.
//!
//! This is the only place the digest spelling of a number is decided. It
//! matches Python's `_decimal_text`: zero is exactly `"0"`, every other value
//! is normalized, and `f`-style formatting never uses an exponent. Wire
//! amounts keep their stored database scale; only the digest text normalizes.

use rust_decimal::Decimal;

/// Normalized decimal text. Zero has the single spelling `"0"`.
pub fn decimal_text(value: Decimal) -> String {
    if value.is_zero() {
        "0".to_owned()
    } else {
        value.normalize().to_string()
    }
}

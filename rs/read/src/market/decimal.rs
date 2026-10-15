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
        canonical_decimal_text(&value.to_string()).expect("a Decimal has valid finite text")
    }
}

/// Python Decimal.normalize() under the default 28-digit, half-even context,
/// followed by fixed-point formatting. Never narrow a database NUMERIC first.
pub fn canonical_decimal_text(text: &str) -> Result<String, String> {
    let text = text.trim();
    let negative = text.starts_with('-');
    let unsigned = text.trim_start_matches(['-', '+']);
    let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (
            mantissa,
            exponent
                .parse::<i32>()
                .map_err(|_| "invalid decimal exponent")?,
        ),
        None => (unsigned, 0),
    };
    let mut parts = mantissa.split('.');
    let integer = parts.next().unwrap_or_default();
    let fraction = parts.next().unwrap_or_default();
    if parts.next().is_some()
        || integer.len() + fraction.len() == 0
        || !integer
            .bytes()
            .chain(fraction.bytes())
            .all(|c| c.is_ascii_digit())
    {
        return Err("invalid decimal coefficient".to_owned());
    }
    let coefficient = format!("{integer}{fraction}");
    let mut digits = coefficient.trim_start_matches('0').as_bytes().to_vec();
    if digits.is_empty() {
        return Ok("0".to_owned());
    }
    let mut exponent = exponent
        .checked_sub(i32::try_from(fraction.len()).map_err(|_| "decimal scale overflow")?)
        .ok_or("decimal scale overflow")?;
    const PYTHON_PRECISION: usize = 28;
    if digits.len() > PYTHON_PRECISION {
        let discarded = &digits[PYTHON_PRECISION..];
        let round_up = discarded[0] > b'5'
            || (discarded[0] == b'5'
                && (discarded[1..].iter().any(|c| *c != b'0')
                    || (digits[PYTHON_PRECISION - 1] - b'0') % 2 == 1));
        exponent += (digits.len() - PYTHON_PRECISION) as i32;
        digits.truncate(PYTHON_PRECISION);
        if round_up {
            let mut carry = true;
            for digit in digits.iter_mut().rev() {
                if *digit == b'9' {
                    *digit = b'0';
                } else {
                    *digit += 1;
                    carry = false;
                    break;
                }
            }
            if carry {
                digits.insert(0, b'1');
            }
        }
    }
    while digits.last() == Some(&b'0') {
        digits.pop();
        exponent += 1;
    }
    // PostgreSQL NUMERIC supports far fewer digits than Python's exponent limit.
    // Bound allocation without silently replacing a non-null value with null.
    if exponent.unsigned_abs() > 1_000_000 {
        return Err("decimal exponent outside Python context".to_owned());
    }
    let digits = String::from_utf8(digits).expect("ASCII digits");
    let point = digits.len() as i32 + exponent;
    let body = if exponent >= 0 {
        format!("{digits}{}", "0".repeat(exponent as usize))
    } else if point <= 0 {
        format!("0.{}{digits}", "0".repeat((-point) as usize))
    } else {
        let point = point as usize;
        format!("{}.{}", &digits[..point], &digits[point..])
    };
    Ok(format!("{}{body}", if negative { "-" } else { "" }))
}

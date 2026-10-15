//! Decimal wire text for the reviewed-terms digest.
//!
//! This is the only place the digest spelling of a number is decided. It
//! matches Python's `_decimal_text`: zero is exactly `"0"`, every other value
//! is normalized, and `f`-style formatting never uses an exponent. Wire
//! amounts keep their stored database scale; only the digest text normalizes.

use rust_decimal::Decimal;
use std::cmp::Ordering;

/// Exact finite stored terms, independent of rust_decimal's coefficient/scale limits.
/// Bounds in the typed schemas are integers; comparisons never round the value.
#[derive(Clone, Debug)]
pub(crate) struct ExactDecimal {
    negative: bool,
    digits: String,
    exponent: i64,
}

impl ExactDecimal {
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let negative = text.starts_with('-');
        let unsigned = text.strip_prefix(['-', '+']).unwrap_or(text);
        let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
            Some((mantissa, exponent)) => (
                mantissa,
                exponent
                    .parse::<i64>()
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
            return Err("invalid decimal coefficient".into());
        }
        let coefficient = format!("{integer}{fraction}");
        let digits = coefficient.trim_start_matches('0').to_owned();
        let exponent = exponent
            .checked_sub(fraction.len() as i64)
            .ok_or("decimal scale overflow")?;
        exponent
            .checked_add(digits.len() as i64)
            .ok_or("decimal magnitude overflow")?;
        Ok(Self {
            negative: negative && !digits.is_empty(),
            digits,
            exponent,
        })
    }

    /// Pydantic checks decimal_places on Decimal.normalize() in its 28-digit context.
    /// Range checks above still use the exact, unrounded value.
    pub(crate) fn python_decimal_places(&self) -> u64 {
        if self.digits.is_empty() {
            return 0;
        }
        let mut digits = self.digits.as_bytes().to_vec();
        let mut exponent = self.exponent;
        const PRECISION: usize = 28;
        if digits.len() > PRECISION {
            let discarded = &digits[PRECISION..];
            let round_up = discarded[0] > b'5'
                || (discarded[0] == b'5'
                    && (discarded[1..].iter().any(|d| *d != b'0')
                        || (digits[PRECISION - 1] - b'0') % 2 == 1));
            exponent += (digits.len() - PRECISION) as i64;
            digits.truncate(PRECISION);
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
        if exponent < 0 {
            exponent.unsigned_abs()
        } else {
            0
        }
    }

    fn compare_integer(&self, bound: i32) -> Ordering {
        let other = Self::parse(&bound.to_string()).expect("integer is a finite decimal");
        if self.negative != other.negative {
            return if self.negative {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        let magnitude = if self.digits.is_empty() || other.digits.is_empty() {
            self.digits
                .is_empty()
                .cmp(&other.digits.is_empty())
                .reverse()
        } else {
            let left_size = self.digits.len() as i64 + self.exponent;
            let right_size = other.digits.len() as i64 + other.exponent;
            left_size.cmp(&right_size).then_with(|| {
                let width = self.digits.len().max(other.digits.len());
                self.digits
                    .bytes()
                    .chain(std::iter::repeat(b'0'))
                    .take(width)
                    .cmp(
                        other
                            .digits
                            .bytes()
                            .chain(std::iter::repeat(b'0'))
                            .take(width),
                    )
            })
        };
        if self.negative {
            magnitude.reverse()
        } else {
            magnitude
        }
    }
}

impl PartialEq<i32> for ExactDecimal {
    fn eq(&self, other: &i32) -> bool {
        self.compare_integer(*other) == Ordering::Equal
    }
}
impl PartialOrd<i32> for ExactDecimal {
    fn partial_cmp(&self, other: &i32) -> Option<Ordering> {
        Some(self.compare_integer(*other))
    }
}

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

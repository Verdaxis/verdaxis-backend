//! Exact public-market and voyage amounts with economic rounding.
//!
//! Existing formulas round intermediate results to 28 significant digits,
//! half-even. Removing that step changes economic values, not only formatting.
//! Final display quantization is half-up; public spelling is independent.
use num_bigint::BigInt;
use num_traits::{Signed, Zero};
use std::cmp::Ordering;

const PRECISION: usize = 28;
// The 64-bit Python constructor domain differs from the default arithmetic
// context. Keep input exponents intact; clamp only arithmetic zero/underflow.
const MIN_CONSTRUCTOR_EXPONENT: i64 = -1_999_999_999_999_999_997;
const MAX_CONSTRUCTOR_EXPONENT: i64 = 999_999_999_999_999_999;
const ETINY: i64 = -1_000_026;
const EMAX: i64 = 999_999;

#[derive(Clone, Debug)]
pub struct Amount {
    coefficient: BigInt,
    exponent: i64,
    negative_zero: bool,
}

fn power(exponent: u32) -> BigInt {
    BigInt::from(10).pow(exponent)
}

fn rounded_division(value: &BigInt, divisor: &BigInt, half_up: bool) -> BigInt {
    let magnitude = value.abs();
    let mut quotient = &magnitude / divisor;
    let remainder = &magnitude % divisor;
    let comparison = (remainder * 2u8).cmp(divisor);
    if comparison == Ordering::Greater
        || (comparison == Ordering::Equal && (half_up || &quotient % 2u8 != BigInt::zero()))
    {
        quotient += 1;
    }
    if value.is_negative() {
        -quotient
    } else {
        quotient
    }
}

// Never materialize zeros implied by an untrusted exponent.
fn round_power(value: &BigInt, shift: i64, half_up: bool) -> BigInt {
    if value.is_zero() || shift > value.abs().to_string().len() as i64 {
        return BigInt::zero();
    }
    rounded_division(value, &power(shift as u32), half_up)
}

fn aligned(value: &BigInt, source: i64, target: i64) -> BigInt {
    if value.is_zero() {
        return BigInt::zero();
    }
    if source >= target {
        return value * power((source - target) as u32);
    }
    let shift = target - source;
    let sign = if value.is_negative() { -1 } else { 1 };
    if shift >= value.abs().to_string().len() as i64 {
        return BigInt::from(sign);
    }
    let divisor = power(shift as u32);
    let quotient = value / &divisor;
    if value % divisor == BigInt::zero() {
        quotient
    } else if &quotient % 10u8 == BigInt::zero() {
        quotient + sign
    } else {
        quotient
    }
}

impl Amount {
    pub fn compare(&self, other: &Self) -> Ordering {
        let left_sign = self.coefficient.sign();
        let right_sign = other.coefficient.sign();
        if left_sign != right_sign {
            return self.coefficient.cmp(&other.coefficient);
        }
        if self.coefficient.is_zero() {
            return Ordering::Equal;
        }
        let left = self.coefficient.abs().to_string();
        let right = other.coefficient.abs().to_string();
        let magnitude = (self.exponent + left.len() as i64)
            .cmp(&(other.exponent + right.len() as i64))
            .then_with(|| {
                let width = left.len().max(right.len());
                left.bytes()
                    .chain(std::iter::repeat(b'0'))
                    .take(width)
                    .cmp(right.bytes().chain(std::iter::repeat(b'0')).take(width))
            });
        if self.coefficient.is_negative() {
            magnitude.reverse()
        } else {
            magnitude
        }
    }

    pub fn ceil_days(&self) -> Result<i64, String> {
        use num_traits::ToPrimitive;
        if self.coefficient.is_zero() {
            return Ok(0);
        }
        let digits = self.coefficient.abs().to_string().len() as i64;
        if self.exponent + digits > 19 {
            return Err("calendar duration overflow".into());
        }
        if self.exponent + digits <= 0 {
            return Ok(i64::from(self.coefficient.is_positive()));
        }
        let value = if self.exponent >= 0 {
            &self.coefficient * power(self.exponent as u32)
        } else {
            let divisor = power((-self.exponent) as u32);
            let quotient = &self.coefficient / &divisor;
            if self.coefficient.is_positive() && &self.coefficient % divisor != BigInt::zero() {
                quotient + 1
            } else {
                quotient
            }
        };
        value
            .to_i64()
            .ok_or_else(|| "calendar duration overflow".into())
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let (mantissa, exponent) = text.split_once(['e', 'E']).unwrap_or((text, "0"));
        let exponent: i64 = exponent.parse().map_err(|_| "invalid decimal exponent")?;
        let negative = mantissa.starts_with('-');
        let unsigned = mantissa.strip_prefix(['-', '+']).unwrap_or(mantissa);
        let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
        let digits = format!("{whole}{fraction}");
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("invalid decimal coefficient".into());
        }
        let exponent = exponent
            .checked_sub(fraction.len().try_into().map_err(|_| "decimal scale")?)
            .ok_or("decimal scale")?;
        let significant = digits.trim_start_matches('0').len().max(1) as i64;
        if !(MIN_CONSTRUCTOR_EXPONENT..=MAX_CONSTRUCTOR_EXPONENT).contains(&exponent)
            || exponent + significant - 1 > MAX_CONSTRUCTOR_EXPONENT
        {
            return Err("decimal exceeds arithmetic bounds".into());
        }
        let mut coefficient =
            BigInt::parse_bytes(digits.as_bytes(), 10).ok_or("decimal coefficient")?;
        if negative {
            coefficient = -coefficient;
        }
        Ok(Self {
            negative_zero: negative && coefficient.is_zero(),
            coefficient,
            exponent,
        })
    }

    fn context(mut self) -> Self {
        if self.coefficient.is_zero() {
            self.exponent = self.exponent.clamp(ETINY, EMAX);
            return self;
        }
        let digits = self.coefficient.abs().to_string().len() as i64;
        // Round once, at the stricter of precision and subnormal exponent.
        let shift = (digits - PRECISION as i64)
            .max(ETINY - self.exponent)
            .max(0);
        if shift > 0 {
            self.negative_zero = self.coefficient.is_negative();
            self.coefficient = round_power(&self.coefficient, shift, false);
            self.exponent += shift;
            if self.coefficient.abs().to_string().len() > PRECISION {
                self.coefficient /= 10;
                self.exponent += 1;
            }
        }
        self
    }

    pub fn add(&self, other: &Self) -> Self {
        let preferred = self.exponent.min(other.exponent);
        let adjusted = |value: &Self| {
            if value.coefficient.is_zero() {
                ETINY - 1
            } else {
                value.exponent + value.coefficient.abs().to_string().len() as i64 - 1
            }
        };
        // Two guard digits plus a sticky digit suffice when exponents are far
        // apart. Close operands retain all digits, including cancellation.
        let left_adjusted = adjusted(self);
        let right_adjusted = adjusted(other);
        let cancellation = self.coefficient.sign() != other.coefficient.sign()
            && !self.coefficient.is_zero()
            && !other.coefficient.is_zero()
            && (left_adjusted - right_adjusted).abs() <= 1;
        let floor = if cancellation {
            preferred
        } else {
            (left_adjusted.max(right_adjusted) - PRECISION as i64 - 2).max(ETINY - 2)
        };
        let exponent = preferred.max(floor);
        let coefficient = aligned(&self.coefficient, self.exponent, exponent)
            + aligned(&other.coefficient, other.exponent, exponent);
        Self {
            coefficient,
            exponent: if self.coefficient.is_zero() && other.coefficient.is_zero() {
                preferred
            } else {
                exponent
            },
            negative_zero: self.negative_zero && other.negative_zero,
        }
        .context()
    }

    pub fn subtract(&self, other: &Self) -> Self {
        let mut negative = other.clone();
        negative.coefficient = -negative.coefficient;
        negative.negative_zero = !negative.negative_zero && negative.coefficient.is_zero();
        self.add(&negative)
    }

    pub fn multiply(&self, other: &Self) -> Self {
        Self {
            coefficient: &self.coefficient * &other.coefficient,
            exponent: self.exponent + other.exponent,
            negative_zero: (self.coefficient.is_negative() || self.negative_zero)
                != (other.coefficient.is_negative() || other.negative_zero),
        }
        .context()
    }

    pub fn divide(&self, other: &Self) -> Result<Self, String> {
        if other.coefficient.is_zero() {
            return Err("division by zero".into());
        }
        let negative = (self.coefficient.is_negative() || self.negative_zero)
            != (other.coefficient.is_negative() || other.negative_zero);
        let numerator = self.coefficient.abs();
        let denominator = other.coefficient.abs();
        let preferred = self.exponent - other.exponent;
        if numerator.is_zero() {
            return Ok(Self {
                coefficient: BigInt::zero(),
                exponent: preferred.clamp(ETINY, EMAX),
                negative_zero: negative,
            });
        }
        let mut adjusted =
            numerator.to_string().len() as i64 - denominator.to_string().len() as i64;
        let less = if adjusted >= 0 {
            numerator < &denominator * power(adjusted as u32)
        } else {
            &numerator * power((-adjusted) as u32) < denominator
        };
        if less {
            adjusted -= 1;
        }
        let shift = (PRECISION as i64 - 1 - adjusted).min(preferred - ETINY);
        if shift < 0 && -shift > numerator.to_string().len() as i64 + 1 {
            return Ok(Self {
                coefficient: BigInt::zero(),
                exponent: ETINY,
                negative_zero: negative,
            });
        }
        let (scaled, divisor) = if shift >= 0 {
            (numerator * power(shift as u32), denominator)
        } else {
            (numerator, denominator * power((-shift) as u32))
        };
        let exact = &scaled % &divisor == BigInt::zero();
        let mut coefficient = rounded_division(&scaled, &divisor, false);
        let mut exponent = preferred - shift;
        if exact {
            while exponent < preferred && &coefficient % 10u8 == BigInt::zero() {
                coefficient /= 10;
                exponent += 1;
            }
        }
        if negative {
            coefficient = -coefficient;
        }
        Ok(Self {
            coefficient,
            exponent,
            negative_zero: negative,
        }
        .context())
    }

    pub fn quantize(&self, scale: u32) -> Result<String, String> {
        let target = -i64::from(scale);
        let shift = self.exponent - target;
        let coefficient = if self.coefficient.is_zero() {
            BigInt::zero()
        } else if shift >= 0 {
            if shift + self.coefficient.abs().to_string().len() as i64 > PRECISION as i64 {
                return Err("quantize exceeds context precision".into());
            }
            &self.coefficient * power(shift as u32)
        } else {
            round_power(&self.coefficient, -shift, true)
        };
        if coefficient.abs().to_string().len() > PRECISION {
            return Err("quantize exceeds context precision".into());
        }
        let negative = coefficient.is_negative();
        let digits = coefficient.abs().to_string();
        let width = scale as usize;
        let padded = format!("{:0>width$}", digits, width = width + 1);
        let split = padded.len() - width;
        let sign = if negative { "-" } else { "" };
        if width == 0 {
            Ok(format!("{sign}{padded}"))
        } else {
            Ok(format!("{sign}{}.{}", &padded[..split], &padded[split..]))
        }
    }

    pub fn text(&self) -> String {
        if self.coefficient.is_zero() {
            return "0".into();
        }
        let digits = self.coefficient.abs().to_string();
        let trimmed = digits.trim_end_matches('0');
        let exponent = self.exponent + (digits.len() - trimmed.len()) as i64;
        let sign = if self.coefficient.is_negative() {
            "-"
        } else {
            ""
        };
        format!("{sign}{trimmed}e{exponent}")
    }
}

/// Round an exact SQL quantity/price product once at the economic boundary.
pub fn trade_total(exact_product: &str) -> Result<String, String> {
    Ok(Amount::parse(exact_product)?.context().text())
}

/// Add aggregate quantities with the same intermediate economic rounding.
pub fn sum_quantities(left: &str, right: &str) -> Result<String, String> {
    Ok(Amount::parse(left)?.add(&Amount::parse(right)?).text())
}

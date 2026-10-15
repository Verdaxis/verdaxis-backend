//! Python Decimal's 28-significant-digit, half-even arithmetic context.
//! Input scale is retained; half-up is used only for output quantization.
use num_bigint::BigInt;
use num_traits::{Signed, Zero};
use std::cmp::Ordering;

const PRECISION: usize = 28;
// Python's default arithmetic context permits exponents down to Etiny
// (Emin - precision + 1). Full constructor-domain parity remains separate.
const MAX_EXPONENT: i32 = 1_000_026;

#[derive(Clone, Debug)]
pub struct Decimal {
    coefficient: BigInt,
    exponent: i32,
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

impl Decimal {
    pub fn compare(&self, other: &Self) -> Ordering {
        let exponent = self.exponent.min(other.exponent);
        (&self.coefficient * power((self.exponent - exponent) as u32))
            .cmp(&(&other.coefficient * power((other.exponent - exponent) as u32)))
    }

    pub fn ceil_days(&self) -> Result<i64, String> {
        use num_traits::ToPrimitive;
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
        let exponent: i32 = exponent.parse().map_err(|_| "invalid decimal exponent")?;
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
        if exponent.unsigned_abs() > MAX_EXPONENT as u32 || digits.len() > MAX_EXPONENT as usize {
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
        let digits = self.coefficient.abs().to_string().len();
        if digits > PRECISION {
            let shift = (digits - PRECISION) as u32;
            self.coefficient = rounded_division(&self.coefficient, &power(shift), false);
            self.exponent += shift as i32;
            if self.coefficient.abs().to_string().len() > PRECISION {
                self.coefficient /= 10;
                self.exponent += 1;
            }
        }
        self
    }

    pub fn add(&self, other: &Self) -> Self {
        let exponent = self.exponent.min(other.exponent);
        let coefficient = &self.coefficient * power((self.exponent - exponent) as u32)
            + &other.coefficient * power((other.exponent - exponent) as u32);
        Self {
            coefficient,
            exponent,
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
                exponent: preferred,
                negative_zero: negative,
            });
        }
        let mut adjusted =
            numerator.to_string().len() as i32 - denominator.to_string().len() as i32;
        let less = if adjusted >= 0 {
            numerator < &denominator * power(adjusted as u32)
        } else {
            &numerator * power((-adjusted) as u32) < denominator
        };
        if less {
            adjusted -= 1;
        }
        let shift = PRECISION as i32 - 1 - adjusted;
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
        let target = -(i32::try_from(scale).map_err(|_| "invalid output scale")?);
        let shift = self.exponent - target;
        let coefficient = if shift >= 0 {
            &self.coefficient * power(shift as u32)
        } else {
            rounded_division(&self.coefficient, &power((-shift) as u32), true)
        };
        if coefficient.abs().to_string().len() > PRECISION {
            return Err("quantize exceeds context precision".into());
        }
        Ok(Self {
            coefficient,
            exponent: target,
            negative_zero: self.coefficient.is_negative() || self.negative_zero,
        }
        .text())
    }

    pub fn text(&self) -> String {
        let digits = self.coefficient.abs().to_string();
        let sign = if self.coefficient.is_negative()
            || (self.coefficient.is_zero() && self.negative_zero)
        {
            "-"
        } else {
            ""
        };
        let adjusted = self.exponent + digits.len() as i32 - 1;
        if self.exponent > 0 || adjusted < -6 {
            let mantissa = if digits.len() == 1 {
                digits.clone()
            } else {
                format!("{}.{}", &digits[..1], &digits[1..])
            };
            return format!("{sign}{mantissa}E{adjusted:+}");
        }
        if self.exponent == 0 {
            return format!("{sign}{digits}");
        }
        let point = digits.len() as i32 + self.exponent;
        if point > 0 {
            format!(
                "{sign}{}.{}",
                &digits[..point as usize],
                &digits[point as usize..]
            )
        } else {
            format!("{sign}0.{}{digits}", "0".repeat((-point) as usize))
        }
    }
}

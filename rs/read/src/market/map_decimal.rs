//! Exact nonnegative order economics for the compact map reducer.
//! SQL NUMERIC is unbounded; Python rounds each addition to 28 digits.
use crate::market::decimal::python_product_text;
use std::cmp::Ordering;

struct Amount {
    digits: Vec<u8>,
    scale: i64,
}

impl Amount {
    fn parse(text: &str) -> Result<Self, String> {
        let text = text.strip_prefix('+').unwrap_or(text);
        let (mantissa, exponent) = match text.split_once(['e', 'E']) {
            Some((mantissa, exponent)) => (
                mantissa,
                exponent.parse::<i32>().map_err(|_| "invalid exponent")?,
            ),
            None => (text, 0),
        };
        let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        let coefficient = format!("{whole}{fraction}");
        if coefficient.is_empty() || !coefficient.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("expected nonnegative order amount".into());
        }
        let digits = coefficient.bytes().map(|byte| byte - b'0').collect();
        let scale = fraction.len() as i64 - i64::from(exponent);
        Ok(Self { digits, scale })
    }
    fn aligned(&self, scale: i64) -> Vec<u8> {
        let mut digits = self.digits.clone();
        digits.extend(std::iter::repeat_n(0, (scale - self.scale) as usize));
        let leading = digits
            .iter()
            .position(|digit| *digit != 0)
            .unwrap_or(digits.len().saturating_sub(1));
        digits.drain(..leading);
        digits
    }
}

pub fn compare(left: &str, right: &str) -> Result<Ordering, String> {
    let left = Amount::parse(left)?;
    let right = Amount::parse(right)?;
    let scale = left.scale.max(right.scale);
    let left = left.aligned(scale);
    let right = right.aligned(scale);
    Ok(left.len().cmp(&right.len()).then_with(|| left.cmp(&right)))
}

pub fn add(left: &str, right: &str) -> Result<String, String> {
    let left = Amount::parse(left)?;
    let right = Amount::parse(right)?;
    let scale = left.scale.max(right.scale);
    let left = left.aligned(scale);
    let right = right.aligned(scale);
    let width = left.len().max(right.len());
    let mut sum = Vec::with_capacity(width + 1);
    let mut carry = 0;
    for index in 0..width {
        let a = left.len().checked_sub(index + 1).map_or(0, |i| left[i]);
        let b = right.len().checked_sub(index + 1).map_or(0, |i| right[i]);
        let value = a + b + carry;
        sum.push(value % 10);
        carry = value / 10;
    }
    if carry != 0 {
        sum.push(carry);
    }
    sum.reverse();
    let digits: String = sum
        .into_iter()
        .map(|digit| char::from(b'0' + digit))
        .collect();
    python_product_text(&format!("{digits}E{}", -scale))
}

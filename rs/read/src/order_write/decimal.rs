//! Exact money and quantity primitives for the write path (Phases 8-9 plan
//! §6.1; Task 2).
//!
//! Every stored or computed economic value is a [`Decimal`] newtype with one
//! bound and one maximum stored scale. Parsing reads the JSON literal as text,
//! so a binary float never reaches a domain value, and no constructor rounds:
//! a value with too many fraction digits is refused, not truncated. The only
//! rounding call site in the write path is [`quantize_cents`], and every
//! multiplication, addition or subtraction goes through a `checked_*` form.
//!
//! Stock and remainders allow zero, because inventory is created at zero stock
//! and a filled order has a zero remainder. Order, trade and price values must
//! stay positive, because every creation path refuses zero.
// The declaration-only test target includes this leaf directly. Task 3 removes
// this allowance when the test target uses the wired exports.
#![allow(dead_code)]
#![deny(clippy::float_arithmetic)]

use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Deserializer, de};

use super::error::{MarketErrorCode, MarketWriteError};

/// One bound-and-scale domain of plan §6.1.
///
/// The bound is an integer, so every domain is a `const`, the boundary values
/// are exact, and a domain carries the two refusal codes it reports.
#[derive(Clone, Copy, Debug)]
pub struct Domain {
    minimum: i64,
    exclusive_minimum: bool,
    maximum: i64,
    scale: u32,
    out_of_range: MarketErrorCode,
    scale_invalid: MarketErrorCode,
}

impl Domain {
    /// `0 < value ≤ maximum`: order and trade quantities, unit prices.
    const fn positive(
        maximum: i64,
        scale: u32,
        out_of_range: MarketErrorCode,
        scale_invalid: MarketErrorCode,
    ) -> Self {
        Self {
            minimum: 0,
            exclusive_minimum: true,
            maximum,
            scale,
            out_of_range,
            scale_invalid,
        }
    }

    /// `0 ≤ value ≤ maximum`: stock, remainders, money and rates.
    const fn non_negative(
        maximum: i64,
        scale: u32,
        out_of_range: MarketErrorCode,
        scale_invalid: MarketErrorCode,
    ) -> Self {
        Self {
            minimum: 0,
            exclusive_minimum: false,
            maximum,
            scale,
            out_of_range,
            scale_invalid,
        }
    }

    /// Parse one JSON literal exactly, then apply the bound and the scale.
    pub fn parse(self, raw: &str) -> Result<Decimal, MarketWriteError> {
        self.check(literal(raw, self.scale_invalid)?)
    }

    /// Check a value that came from a computed result or a stored row.
    ///
    /// The scale is a stored-column rule, so a computed intermediate (a product
    /// before [`quantize_cents`]) is checked with [`checked_multiply`] instead.
    pub fn check(self, value: Decimal) -> Result<Decimal, MarketWriteError> {
        let minimum = Decimal::from(self.minimum);
        let below = if self.exclusive_minimum {
            value <= minimum
        } else {
            value < minimum
        };
        if below || value > Decimal::from(self.maximum) {
            return Err(MarketWriteError::new(self.out_of_range));
        }
        if value.scale() > self.scale {
            return Err(MarketWriteError::new(self.scale_invalid));
        }
        Ok(value)
    }
}

/// The literal grammar a domain accepts.
///
/// An optional `-`, then digits with at most one decimal point that has at
/// least one fraction digit. Exponent notation, a leading `+`, whitespace, a
/// digit separator, a bare point and negative zero are refused: JSON can spell
/// them, but no stored economic value may.
fn literal(raw: &str, refusal: MarketErrorCode) -> Result<Decimal, MarketWriteError> {
    let refused = || MarketWriteError::new(refusal);
    let body = match raw.as_bytes() {
        [b'-', rest @ ..] => rest,
        [b'+', ..] | [] => return Err(refused()),
        rest => rest,
    };
    let (mut digits, mut fraction, mut point) = (0usize, 0usize, false);
    for &byte in body {
        match byte {
            b'0'..=b'9' if point => fraction += 1,
            b'0'..=b'9' => digits += 1,
            b'.' if !point => point = true,
            _ => return Err(refused()),
        }
    }
    if digits == 0 || (point && fraction == 0) {
        return Err(refused());
    }
    // `from_str_exact` never rounds; a value outside the 96-bit coefficient is
    // an invalid literal rather than a silently narrowed one.
    let value = Decimal::from_str_exact(raw).map_err(|_| refused())?;
    if value.is_zero() && raw.starts_with('-') {
        return Err(refused());
    }
    Ok(value)
}

/// One domain value. The inner `Decimal` is exact: it is never rounded here.
pub trait ExactValue: Sized {
    /// Parse one JSON literal exactly, applying the domain bound and scale.
    fn parse_exact(raw: &str) -> Result<Self, MarketWriteError>;
}

/// Declare one domain newtype with its parser, checked constructor, exact
/// accessor and serde reader.
///
/// Deserialization goes through [`serde_json::Number`] and its text form, so a
/// number that the tokenizer kept as text keeps every digit, and a value that
/// arrived as a binary float is refused by the same literal grammar.
macro_rules! exact_domain {
    ($(#[$meta:meta])* $name:ident, $domain:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug)]
        pub struct $name(Decimal);

        impl $name {
            /// The §6.1 bound and scale this domain enforces.
            pub const DOMAIN: Domain = $domain;

            /// Parse one JSON literal exactly, applying the bound and scale.
            pub fn parse_exact(raw: &str) -> Result<Self, MarketWriteError> {
                Self::DOMAIN.parse(raw).map(Self)
            }

            /// Check a computed or stored value against the same domain.
            pub fn checked(value: Decimal) -> Result<Self, MarketWriteError> {
                Self::DOMAIN.check(value).map(Self)
            }

            /// The exact value. It is never rounded.
            pub const fn value(self) -> Decimal {
                self.0
            }
        }

        impl ExactValue for $name {
            fn parse_exact(raw: &str) -> Result<Self, MarketWriteError> {
                $name::parse_exact(raw)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let number = serde_json::Number::deserialize(deserializer)?;
                ExactValue::parse_exact(&number.to_string()).map_err(de::Error::custom)
            }
        }
    };
}

// The §6.1 domains. A bound is an integer so the domain is a `const`, and a
// refusal names its own code: an invalid stored remainder is inconsistent
// state (409), not a bad request (422).
const ORDER_QUANTITY: Domain = Domain::positive(
    100_000,
    2,
    MarketErrorCode::QuantityOutOfRange,
    MarketErrorCode::QuantityScaleInvalid,
);
/// An invalid stored remainder is inconsistent state, not a bad request.
const ORDER_REMAINDER: Domain = Domain::non_negative(
    100_000,
    2,
    MarketErrorCode::InventoryInconsistent,
    MarketErrorCode::InventoryInconsistent,
);
const STOCK_QUANTITY: Domain = Domain::non_negative(
    100_000,
    2,
    MarketErrorCode::QuantityOutOfRange,
    MarketErrorCode::QuantityScaleInvalid,
);
const PRICE: Domain = Domain::positive(
    1_000_000,
    2,
    MarketErrorCode::PriceOutOfRange,
    MarketErrorCode::PriceScaleInvalid,
);
const MONEY: Domain = Domain::non_negative(
    100_000_000_000,
    2,
    MarketErrorCode::ValueOutOfRange,
    MarketErrorCode::ValueScaleInvalid,
);
const AGGREGATE_MONEY: Domain = Domain::non_negative(
    10_000_000_000_000,
    2,
    MarketErrorCode::ValueOutOfRange,
    MarketErrorCode::ValueScaleInvalid,
);
const FEE_PER_MT: Domain = Domain::non_negative(
    100_000,
    2,
    MarketErrorCode::FeeRateInvalid,
    MarketErrorCode::FeeRateInvalid,
);
const COMMISSION_PERCENT: Domain = Domain::non_negative(
    100,
    3,
    MarketErrorCode::CommissionRateInvalid,
    MarketErrorCode::CommissionRateInvalid,
);

exact_domain! {
    /// The quantity of one order or trade: `0 < value ≤ 100000`, two decimals.
    OrderQuantity, ORDER_QUANTITY
}
exact_domain! {
    /// The quantity of one confirmed trade: the same domain as [`OrderQuantity`].
    TradeQuantity, ORDER_QUANTITY
}
exact_domain! {
    /// A live or historical order remainder: `0 ≤ value ≤ 100000`, two decimals.
    OrderRemainder, ORDER_REMAINDER
}
exact_domain! {
    /// Current, incoming or reserved stock and one ledger delta.
    StockQuantity, STOCK_QUANTITY
}
exact_domain! {
    /// A price per metric tonne: `0 < value ≤ 1000000`, two decimals.
    Price, PRICE
}
exact_domain! {
    /// A persisted trade total or fee amount: `0 ≤ value ≤ 10¹¹`, two decimals.
    Money, MONEY
}
exact_domain! {
    /// A bounded batch sum or a legacy commission aggregate: `0 ≤ value ≤ 10¹³`.
    AggregateMoney, AGGREGATE_MONEY
}
exact_domain! {
    /// The frozen seller fee per metric tonne: `0 ≤ value ≤ 100000`, two decimals.
    FeePerMt, FEE_PER_MT
}
exact_domain! {
    /// The frozen legacy commission percentage: `0 ≤ value ≤ 100`, three decimals.
    CommissionPercent, COMMISSION_PERCENT
}
/// One multiplication in the economic path.
///
/// A checked overflow becomes 422 `VALUE_OVERFLOW` for a value derived from the
/// request; a value derived from stored rows is checked against its domain
/// afterwards, which is where 409 `INVENTORY_INCONSISTENT` comes from. It never
/// panics and never truncates.
pub fn checked_multiply(left: Decimal, right: Decimal) -> Result<Decimal, MarketWriteError> {
    left.checked_mul(right)
        .ok_or_else(|| MarketWriteError::new(MarketErrorCode::ValueOverflow))
}

/// One addition in the economic path, with the same overflow rule.
pub fn checked_add(left: Decimal, right: Decimal) -> Result<Decimal, MarketWriteError> {
    left.checked_add(right)
        .ok_or_else(|| MarketWriteError::new(MarketErrorCode::ValueOverflow))
}

/// One subtraction in the economic path, with the same overflow rule.
pub fn checked_subtract(left: Decimal, right: Decimal) -> Result<Decimal, MarketWriteError> {
    left.checked_sub(right)
        .ok_or_else(|| MarketWriteError::new(MarketErrorCode::ValueOverflow))
}

/// Round to cents: `final_total_usd = quantize_cents(quantity × price)`.
///
/// This is the one rounding call site of the write path. `MidpointAwayFromZero`
/// is HALF_UP for positive values, which is what the delivery and commission
/// rules require; the read side's benchmark quantisation is a separate,
/// long-standing display rule and is not reused here.
pub fn quantize_cents(value: Decimal) -> Decimal {
    value.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
}

/// Compare `part` against `limit_percent` of `whole` without dividing.
///
/// `|final − original| × 100 ≤ original × limit` is the delivery-price rule:
/// cross-multiplication keeps exactly ten percent accepted, where a division
/// would leave a remainder to decide the boundary.
pub fn cmp_percentage(
    part: Decimal,
    whole: Decimal,
    limit_percent: Decimal,
) -> Result<std::cmp::Ordering, MarketWriteError> {
    let limit = checked_multiply(whole, limit_percent)?;
    Ok(checked_multiply(part.abs(), Decimal::ONE_HUNDRED)?.cmp(&limit))
}

//! Fuel-mix and reporting-year rules for the compliance reads.
//!
//! The rules are small enough to keep as plain functions: a mix is a short map
//! of known fuel names to fractions that must be close to one and is
//! normalized exactly once. The bounds are safety limits, not measured
//! throughput figures:
//!
//! * at most [`MAX_MIX_ENTRIES`] entries, counted before any key is read;
//! * each fuel name at most [`MAX_FUEL_KEY_BYTES`] bytes and in the known
//!   table, so no request falls back to an invented intensity;
//! * each coefficient at most [`MAX_COEFFICIENT_DIGITS`] digits, a scale of at
//!   most [`MAX_FUEL_FRACTION_SCALE`] and a value in `0..=1.05`, read from the
//!   literal text so an exponent cannot underflow to zero unnoticed;
//! * a positive total whose distance from one is at most
//!   [`FRACTION_TOLERANCE`], matching the five percent tolerance the current
//!   user interface offers.
//!
//! The coefficient text is the JSON number spelling. Callers pass the exact
//! token, never a parsed float: `1e-13` is refused even though it rounds to
//! zero.
use crate::compliance::scoring::known_fuel_intensity;
use crate::market::fueleu::{fueleu_target, FIRST_REPORTING_YEAR, LAST_REPORTING_YEAR};
use crate::Json;
use rust_decimal::Decimal;
use std::fmt;

/// Largest accepted number of fuels in one mix.
pub const MAX_MIX_ENTRIES: usize = 32;

/// Largest accepted UTF-8 length of one fuel name.
pub const MAX_FUEL_KEY_BYTES: usize = 64;

/// Largest accepted count of coefficient digits in one written fraction.
pub const MAX_COEFFICIENT_DIGITS: usize = 18;

/// Largest accepted number of decimal places in one written fraction.
pub const MAX_FUEL_FRACTION_SCALE: u32 = 12;

/// Largest accepted single share. A share above this is not a fraction.
pub const MAX_FUEL_FRACTION: Decimal = Decimal::from_parts(105, 0, 0, false, 2);

/// Accepted distance of the raw total from one, before normalization.
pub const FRACTION_TOLERANCE: Decimal = Decimal::from_parts(5, 0, 0, false, 2);

/// Reporting year used when the request does not state one.
pub const DEFAULT_REPORTING_YEAR: i32 = 2026;

/// The error code every input violation of this module carries.
pub const INVALID_INPUT_CODE: &str = "INVALID_INPUT";

/// Which mix or year rule refused the input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MixErrorKind {
    NotAFuelMix,
    TooManyFuels,
    FuelKeyTooLong,
    UnknownFuel,
    DuplicateFuel,
    InvalidCoefficient,
    CoefficientTooLong,
    ExponentOutOfRange,
    FractionOutOfRange,
    FractionSumNotPositive,
    FractionSumOutOfTolerance,
    YearOutOfRange,
}

/// A refused mix or year, with the client-visible message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationError {
    pub kind: MixErrorKind,
    pub message: String,
}

impl ValidationError {
    fn new(kind: MixErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ValidationError {}

/// A validated mix: known fuels only, each share normalized so the shares sum
/// to one.
#[derive(Clone, Debug, PartialEq)]
pub struct FuelMix {
    entries: Vec<(String, Decimal)>,
}

impl FuelMix {
    /// The assumed mix for a vessel that declares none.
    pub fn default_vlsfo() -> Self {
        Self {
            entries: vec![("VLSFO".to_owned(), Decimal::ONE)],
        }
    }

    pub fn entries(&self) -> &[(String, Decimal)] {
        &self.entries
    }

    /// The normalized share of one fuel, if the mix contains it.
    pub fn fraction(&self, fuel: &str) -> Option<Decimal> {
        self.entries
            .iter()
            .find(|(name, _)| name == fuel)
            .map(|(_, fraction)| *fraction)
    }
}

/// One written fuel share, before validation.
#[derive(Clone, Debug)]
pub struct RawMixEntry {
    pub fuel: String,
    pub coefficient: String,
}

impl RawMixEntry {
    pub fn new(fuel: impl Into<String>, coefficient: impl Into<String>) -> Self {
        Self {
            fuel: fuel.into(),
            coefficient: coefficient.into(),
        }
    }
}

/// Validate and normalize one mix.
pub fn validate_fuel_mix(entries: &[RawMixEntry]) -> Result<FuelMix, ValidationError> {
    // The container bound comes first: a request with too many entries is a
    // shape violation whatever the names contain.
    if entries.len() > MAX_MIX_ENTRIES {
        return Err(ValidationError::new(
            MixErrorKind::TooManyFuels,
            format!("fuel_mix accepts at most {MAX_MIX_ENTRIES} fuels"),
        ));
    }
    let mut parsed: Vec<(String, Decimal)> = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.fuel.len() > MAX_FUEL_KEY_BYTES {
            return Err(ValidationError::new(
                MixErrorKind::FuelKeyTooLong,
                format!("each fuel name is at most {MAX_FUEL_KEY_BYTES} bytes"),
            ));
        }
        if known_fuel_intensity(&entry.fuel).is_none() {
            return Err(ValidationError::new(
                MixErrorKind::UnknownFuel,
                format!("unknown fuel '{}'", entry.fuel),
            ));
        }
        if parsed.iter().any(|(fuel, _)| fuel == &entry.fuel) {
            return Err(ValidationError::new(
                MixErrorKind::DuplicateFuel,
                format!("fuel '{}' appears more than once", entry.fuel),
            ));
        }
        parsed.push((
            entry.fuel.clone(),
            parse_coefficient(&entry.fuel, &entry.coefficient)?,
        ));
    }
    let total: Decimal = parsed.iter().map(|(_, fraction)| *fraction).sum();
    if total <= Decimal::ZERO {
        return Err(ValidationError::new(
            MixErrorKind::FractionSumNotPositive,
            "fuel_mix must contain a positive share".to_owned(),
        ));
    }
    if (total - Decimal::ONE).abs() > FRACTION_TOLERANCE {
        return Err(ValidationError::new(
            MixErrorKind::FractionSumOutOfTolerance,
            "fuel_mix shares must sum to 1 within 0.05".to_owned(),
        ));
    }
    // Normalize once, so the weighted intensity uses the stated shares.
    Ok(FuelMix {
        entries: parsed
            .into_iter()
            .map(|(fuel, fraction)| (fuel, fraction / total))
            .collect(),
    })
}

/// Read a mix straight from the request document.
///
/// The coefficient text is the number as written. `serde_json` keeps that
/// spelling, so the lexical bounds above see the real exponent.
pub fn fuel_mix_from_json(value: &Json) -> Result<FuelMix, ValidationError> {
    let object = value.as_object().ok_or_else(|| {
        ValidationError::new(
            MixErrorKind::NotAFuelMix,
            "fuel_mix must be an object of fuel names to fractions".to_owned(),
        )
    })?;
    if object.len() > MAX_MIX_ENTRIES {
        return Err(ValidationError::new(
            MixErrorKind::TooManyFuels,
            format!("fuel_mix accepts at most {MAX_MIX_ENTRIES} fuels"),
        ));
    }
    let entries: Vec<RawMixEntry> = object
        .iter()
        .map(|(fuel, coefficient)| {
            let token = coefficient
                .as_number()
                .map(ToString::to_string)
                .ok_or_else(|| {
                    ValidationError::new(
                        MixErrorKind::InvalidCoefficient,
                        format!("fuel_mix['{fuel}'] must be a decimal number"),
                    )
                })?;
            Ok(RawMixEntry::new(fuel.clone(), token))
        })
        .collect::<Result<_, ValidationError>>()?;
    validate_fuel_mix(&entries)
}

fn parse_coefficient(fuel: &str, text: &str) -> Result<Decimal, ValidationError> {
    let invalid = || {
        ValidationError::new(
            MixErrorKind::InvalidCoefficient,
            format!("fuel_mix['{fuel}'] must be a decimal number"),
        )
    };
    let text = text.trim();
    let (mantissa, exponent) = match text.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i64>().map_err(|_| invalid())?),
        None => (text, 0),
    };
    if !(-i64::from(MAX_FUEL_FRACTION_SCALE)..=0).contains(&exponent) {
        return Err(exponent_out_of_range());
    }
    let (negative, unsigned) = match mantissa.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, mantissa.strip_prefix('+').unwrap_or(mantissa)),
    };
    let (integer, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if (integer.is_empty() && fraction.is_empty())
        || !integer
            .bytes()
            .chain(fraction.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return Err(invalid());
    }
    let digits = format!("{integer}{fraction}");
    if digits.trim_start_matches('0').len() > MAX_COEFFICIENT_DIGITS {
        return Err(ValidationError::new(
            MixErrorKind::CoefficientTooLong,
            format!("each fuel fraction has at most {MAX_COEFFICIENT_DIGITS} digits"),
        ));
    }
    // The exact scale of the written value: fraction digits less the written
    // exponent. A lexical extreme lands far outside the accepted range.
    let scale = i64::try_from(fraction.len())
        .ok()
        .and_then(|length| length.checked_sub(exponent))
        .ok_or_else(exponent_out_of_range)?;
    if !(0..=i64::from(MAX_FUEL_FRACTION_SCALE)).contains(&scale) {
        return Err(exponent_out_of_range());
    }
    let coefficient = digits
        .parse::<i128>()
        .map(|value| if negative { -value } else { value })
        .map_err(|_| invalid())?;
    let value = Decimal::from_i128_with_scale(coefficient, scale as u32);
    if value < Decimal::ZERO || value > MAX_FUEL_FRACTION {
        return Err(ValidationError::new(
            MixErrorKind::FractionOutOfRange,
            format!("each fuel fraction is between 0 and {MAX_FUEL_FRACTION}"),
        ));
    }
    Ok(value)
}

fn exponent_out_of_range() -> ValidationError {
    ValidationError::new(
        MixErrorKind::ExponentOutOfRange,
        format!("each fuel fraction has at most {MAX_FUEL_FRACTION_SCALE} decimal places"),
    )
}

/// A reporting year that the shared FuelEU schedule covers.
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub struct ReportingYear(i32);

impl ReportingYear {
    pub fn new(year: i64) -> Result<Self, ValidationError> {
        let year = i32::try_from(year).map_err(|_| Self::out_of_range())?;
        if !(FIRST_REPORTING_YEAR..=LAST_REPORTING_YEAR).contains(&year) {
            return Err(Self::out_of_range());
        }
        Ok(Self(year))
    }

    pub fn get(self) -> i32 {
        self.0
    }

    /// The shared limit for this year. The constructor already proved the
    /// schedule covers it.
    pub fn target(self) -> Decimal {
        fueleu_target(self.0).expect("a validated reporting year has a schedule step")
    }

    fn out_of_range() -> ValidationError {
        ValidationError::new(
            MixErrorKind::YearOutOfRange,
            format!("year must be between {FIRST_REPORTING_YEAR} and {LAST_REPORTING_YEAR}"),
        )
    }
}

impl Default for ReportingYear {
    fn default() -> Self {
        Self(DEFAULT_REPORTING_YEAR)
    }
}

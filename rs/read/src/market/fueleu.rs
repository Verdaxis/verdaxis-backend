//! Shared FuelEU Maritime reference value and reporting-year intensity limits.
//!
//! Regulation (EU) 2023/1805, Article 4(2) sets each limit as a percentage
//! reduction from the reference value. This module keeps the reference and the
//! reductions once and derives every limit from them, so every consumer uses
//! the same values. The limit is a regulatory intensity, not a fuel certificate
//! or a monetary benefit.
use crate::mcp::decimal::Decimal as McpDecimal;
use rust_decimal::Decimal;
use std::fmt;

/// Lifecycle GHG intensity reference, 91.16 gCO2eq/MJ.
pub const FUELEU_REFERENCE_GHG: Decimal = Decimal::from_parts(9116, 0, 0, false, 2);

/// First reporting year that the regulation covers.
pub const FIRST_REPORTING_YEAR: i32 = 2025;

/// Last reporting year that the shared schedule accepts. Calendar years have
/// four digits; the limit stays at its 2050 step after 2050.
pub const LAST_REPORTING_YEAR: i32 = 9999;

/// First year of each step and its reduction from the reference, as a fraction.
const REDUCTIONS: [(i32, Decimal); 6] = [
    (2025, Decimal::from_parts(2, 0, 0, false, 2)), // 2 %
    (2030, Decimal::from_parts(6, 0, 0, false, 2)), // 6 %
    (2035, Decimal::from_parts(145, 0, 0, false, 3)), // 14.5 %
    (2040, Decimal::from_parts(31, 0, 0, false, 2)), // 31 %
    (2045, Decimal::from_parts(62, 0, 0, false, 2)), // 62 %
    (2050, Decimal::from_parts(80, 0, 0, false, 2)), // 80 %
];

/// The reporting year is outside `FIRST_REPORTING_YEAR..=LAST_REPORTING_YEAR`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct YearError;

impl fmt::Display for YearError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "FuelEU reporting year must be between {FIRST_REPORTING_YEAR} and {LAST_REPORTING_YEAR}"
        )
    }
}

impl std::error::Error for YearError {}

/// GHG intensity limit in gCO2eq/MJ for a reporting year.
pub fn fueleu_target(year: i32) -> Result<Decimal, YearError> {
    if !(FIRST_REPORTING_YEAR..=LAST_REPORTING_YEAR).contains(&year) {
        return Err(YearError);
    }
    let &(_, reduction) = REDUCTIONS
        .iter()
        .rev()
        .find(|(first_year, _)| year >= *first_year)
        .expect("the schedule starts at the first reporting year");
    // The reference has two decimal places and each reduction at most three,
    // so the product is exact.
    Ok(FUELEU_REFERENCE_GHG * (Decimal::ONE - reduction))
}

/// The reference value as the exact decimal type of the MCP calculations.
pub fn mcp_reference() -> McpDecimal {
    to_mcp(FUELEU_REFERENCE_GHG)
}

/// The reporting-year limit as the exact decimal type of the MCP calculations.
pub fn mcp_target(year: i32) -> Result<McpDecimal, YearError> {
    fueleu_target(year).map(to_mcp)
}

fn to_mcp(value: Decimal) -> McpDecimal {
    // Only the short exact values of this module cross this adapter.
    McpDecimal::parse(&value.to_string()).expect("a FuelEU value is a valid exact decimal")
}

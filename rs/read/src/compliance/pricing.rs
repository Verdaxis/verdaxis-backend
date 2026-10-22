//! Pure declared-lifecycle comparison and response assumptions.
use crate::market::fueleu::YearError;
use rust_decimal::Decimal;
use serde::Serialize;
use std::fmt;

pub struct LifecycleOverlayCore;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ListingOverlay {
    pub financial_benefit_status: FinancialBenefitStatus,
    pub penalty_avoided_eur_per_mt: Decimal,
    pub penalty_avoided_usd_per_mt: Decimal,
    pub tco2e_avoided_per_mt: Decimal,
    pub ci_gco2_mj: Decimal,
    pub ci_basis: ValueBasis,
    pub lcv_mj_kg: Decimal,
    pub lcv_basis: ValueBasis,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FinancialBenefitStatus {
    Unpriced,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ValueBasis {
    Listing,
    ProductDefault,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OverlayAssumptions {
    pub eur_usd_rate: Decimal,
    pub vlsfo_baseline_gco2_mj: Decimal,
    pub ghgie_actual_gco2_mj: Decimal,
    pub fleet_intensity_basis: FleetIntensityBasis,
    pub fleet_vessel_count: u64,
    pub penalty_eur_per_tonne: Decimal,
    pub year: i32,
    pub year_target: Decimal,
    pub excluded_factors: [&'static str; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FleetIntensityBasis {
    DefaultVlsfo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PricingError {
    ReportingYear(YearError),
    ExchangeRate,
    ArithmeticRange,
}

impl fmt::Display for PricingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReportingYear(error) => error.fmt(formatter),
            Self::ExchangeRate => formatter.write_str("Exchange rate must be positive"),
            Self::ArithmeticRange => {
                formatter.write_str("Lifecycle comparison is outside the numeric range")
            }
        }
    }
}

impl std::error::Error for PricingError {}

impl LifecycleOverlayCore {
    pub fn compute(
        _market_product: Option<&str>,
        _listing_ci_gco2_mj: Option<Decimal>,
        _listing_lcv_mj_kg: Option<Decimal>,
    ) -> Result<Option<ListingOverlay>, PricingError> {
        todo!("declared lifecycle comparison")
    }

    pub fn assumptions(
        _year: i32,
        _fleet_vessel_count: u64,
        _eur_usd_rate: Option<Decimal>,
    ) -> Result<OverlayAssumptions, PricingError> {
        todo!("overlay assumptions")
    }
}

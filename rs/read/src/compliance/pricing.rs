//! Pure declared-lifecycle comparison and response assumptions.
use crate::market::fueleu::{fueleu_target, YearError, FUELEU_REFERENCE_GHG};
use rust_decimal::{Decimal, RoundingStrategy};
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
    /// Decimal inputs are finite by construction. Missing CI is never inferred
    /// from a product label; a supplied nonpositive LCV does not use a default.
    pub fn compute(
        market_product: Option<&str>,
        listing_ci_gco2_mj: Option<Decimal>,
        listing_lcv_mj_kg: Option<Decimal>,
    ) -> Result<Option<ListingOverlay>, PricingError> {
        let Some(ci) = listing_ci_gco2_mj else {
            return Ok(None);
        };
        let (lcv, lcv_basis) = match listing_lcv_mj_kg {
            Some(lcv) => (lcv, ValueBasis::Listing),
            None => match market_product {
                Some("BIO_METHANOL" | "E_METHANOL") => {
                    (Decimal::new(199, 1), ValueBasis::ProductDefault)
                }
                Some("BIO_ETHANOL" | "SYNTHETIC_ETHANOL") => {
                    (Decimal::new(268, 1), ValueBasis::ProductDefault)
                }
                _ => return Ok(None),
            },
        };
        if lcv <= Decimal::ZERO {
            return Ok(None);
        }

        // g/MJ * MJ/kg * 1000 kg/MT / 1_000_000 g/tonne = tonnes/MT.
        // Apply the net 0.001 conversion before multiplying, to avoid a large
        // intermediate grams value. Physical differences round HALF_UP.
        let displacement = if ci >= FUELEU_REFERENCE_GHG {
            Decimal::ZERO
        } else {
            FUELEU_REFERENCE_GHG
                .checked_sub(ci)
                .and_then(|difference| difference.checked_mul(Decimal::new(1, 3)))
                .and_then(|difference| difference.checked_mul(lcv))
                .ok_or(PricingError::ArithmeticRange)?
                .round_dp_with_strategy(3, RoundingStrategy::MidpointAwayFromZero)
        };
        Ok(Some(ListingOverlay {
            financial_benefit_status: FinancialBenefitStatus::Unpriced,
            penalty_avoided_eur_per_mt: Decimal::ZERO,
            penalty_avoided_usd_per_mt: Decimal::ZERO,
            tco2e_avoided_per_mt: displacement,
            ci_gco2_mj: ci,
            ci_basis: ValueBasis::Listing,
            lcv_mj_kg: lcv,
            lcv_basis,
        }))
    }

    /// Vessel count and FX are annotations, not measured intensity or an
    /// entitlement to cash benefits. Reporting targets use the shared schedule.
    pub fn assumptions(
        year: i32,
        fleet_vessel_count: u64,
        eur_usd_rate: Option<Decimal>,
    ) -> Result<OverlayAssumptions, PricingError> {
        let year_target = fueleu_target(year).map_err(PricingError::ReportingYear)?;
        let eur_usd_rate = eur_usd_rate.unwrap_or(Decimal::new(108, 2));
        if eur_usd_rate <= Decimal::ZERO {
            return Err(PricingError::ExchangeRate);
        }
        Ok(OverlayAssumptions {
            eur_usd_rate,
            vlsfo_baseline_gco2_mj: FUELEU_REFERENCE_GHG,
            ghgie_actual_gco2_mj: FUELEU_REFERENCE_GHG,
            fleet_intensity_basis: FleetIntensityBasis::DefaultVlsfo,
            fleet_vessel_count,
            penalty_eur_per_tonne: Decimal::new(2400, 0),
            year,
            year_target,
            excluded_factors: [
                "RFNBO_MULTIPLIER",
                "DEFICIT_ESCALATION",
                "EXTRA_EU_VOYAGE_SCOPE",
            ],
        })
    }
}

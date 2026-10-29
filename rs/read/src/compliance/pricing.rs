//! Pure declared-lifecycle comparison and response assumptions.
use crate::market::fueleu::{
    FUELEU_PENALTY_EUR_PER_TONNE, FUELEU_REFERENCE_GHG, YearError, fueleu_target,
};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::Serialize;
use std::fmt;

const METHANOL_LCV_MJ_KG: Decimal = Decimal::from_parts(199, 0, 0, false, 1);
const ETHANOL_LCV_MJ_KG: Decimal = Decimal::from_parts(268, 0, 0, false, 1);
const DEFAULT_EUR_USD_RATE: Decimal = Decimal::from_parts(108, 0, 0, false, 2);
const GRAMS_PER_KG_TO_TONNES_PER_MT: Decimal = Decimal::from_parts(1, 0, 0, false, 3);

pub struct LifecycleOverlayCore;

/// The route selects this physical family from catalog policy, not user text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlcoholFamily {
    Methanol,
    Ethanol,
}

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
        alcohol_family: Option<AlcoholFamily>,
        listing_ci_gco2_mj: Option<Decimal>,
        listing_lcv_mj_kg: Option<Decimal>,
    ) -> Result<Option<ListingOverlay>, PricingError> {
        let Some(ci) = listing_ci_gco2_mj else {
            return Ok(None);
        };
        let (lcv, lcv_basis) = match listing_lcv_mj_kg {
            Some(lcv) => (lcv, ValueBasis::Listing),
            None => match alcohol_family {
                Some(AlcoholFamily::Methanol) => (METHANOL_LCV_MJ_KG, ValueBasis::ProductDefault),
                Some(AlcoholFamily::Ethanol) => (ETHANOL_LCV_MJ_KG, ValueBasis::ProductDefault),
                None => return Ok(None),
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
                .and_then(|difference| difference.checked_mul(GRAMS_PER_KG_TO_TONNES_PER_MT))
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
        let eur_usd_rate = eur_usd_rate.unwrap_or(DEFAULT_EUR_USD_RATE);
        if eur_usd_rate <= Decimal::ZERO {
            return Err(PricingError::ExchangeRate);
        }
        Ok(OverlayAssumptions {
            eur_usd_rate,
            vlsfo_baseline_gco2_mj: FUELEU_REFERENCE_GHG,
            ghgie_actual_gco2_mj: FUELEU_REFERENCE_GHG,
            fleet_intensity_basis: FleetIntensityBasis::DefaultVlsfo,
            fleet_vessel_count,
            penalty_eur_per_tonne: FUELEU_PENALTY_EUR_PER_TONNE,
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

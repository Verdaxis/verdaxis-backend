//! Compliance score core: FuelEU intensity, ETS status and CII contribution.
//!
//! The core is pure: no database, no clock, no provider and no request. It
//! combines three contributions at fixed weights (FuelEU 50 %, EU ETS 30 %,
//! CII 20 %) and reports the regulatory intensity band the client displays.
//!
//! Two rules matter for the meaning of the numbers:
//!
//! * a well-to-wake intensity never establishes in-scope combustion emissions,
//!   so an absent emissions input stays [`CalculationStatus::Unpriced`] with
//!   null cost and the neutral ETS contribution;
//! * the FuelEU deficit penalty is illustrative for an assumed annual energy,
//!   excludes banking, borrowing and pooling, and is not a measured liability.
//!
//! Reporting-year limits come from [`crate::market::fueleu`]; this module holds
//! no second schedule and no second reference value.
use crate::Json;
use crate::compliance::validation::{FuelMix, ReportingYear};
use crate::market::fueleu::FUELEU_REFERENCE_GHG;
use rust_decimal::{Decimal, RoundingStrategy, prelude::ToPrimitive};
use serde_json::json;

/// Annex IV Part B deficit penalty, EUR per tonne of VLSFO equivalent.
pub const FUELEU_PENALTY_PER_TONNE: Decimal = Decimal::from_parts(2_400, 0, 0, false, 0);

/// Illustrative annual energy for a vessel that declares no consumption.
pub const DEFAULT_ANNUAL_ENERGY_MJ: Decimal = Decimal::from_parts(300_000_000, 0, 0, false, 0);

/// Illustrative EU allowance price, EUR per tonne of CO2.
pub const DEFAULT_ETS_PRICE_EUR: Decimal = Decimal::from_parts(68, 0, 0, false, 0);

/// Benchmark annual emissions of the reference container ship, tonnes of CO2.
const ETS_BENCHMARK_CO2_TONNES: Decimal = Decimal::from_parts(50_000, 0, 0, false, 0);

/// Energy content used to turn a deficit intensity into a fuel quantity.
const VLSFO_ENERGY_MJ_PER_TONNE: Decimal = Decimal::from_parts(41_000, 0, 0, false, 0);

const FUEL_EU_WEIGHT: Decimal = Decimal::from_parts(5, 0, 0, false, 1);
const EU_ETS_WEIGHT: Decimal = Decimal::from_parts(3, 0, 0, false, 1);
const CII_WEIGHT: Decimal = Decimal::from_parts(2, 0, 0, false, 1);

/// Lifecycle intensities, gCO2eq/MJ, in the order the client shows them.
///
/// These are fuel-category assumptions, not verified consignment figures. The
/// two fossil distillates use the shared reference value so no second copy of
/// that number exists.
pub const FUEL_INTENSITIES: [(&str, Decimal); 9] = [
    ("VLSFO", FUELEU_REFERENCE_GHG),
    ("LSMGO", FUELEU_REFERENCE_GHG),
    ("LNG", Decimal::from_parts(6_900, 0, 0, false, 2)),
    ("Bio-LNG", Decimal::from_parts(2_800, 0, 0, false, 2)),
    ("Methanol", Decimal::from_parts(3_100, 0, 0, false, 2)),
    ("E-Methanol", Decimal::from_parts(800, 0, 0, false, 2)),
    ("Ammonia", Decimal::from_parts(1_800, 0, 0, false, 2)),
    ("Biofuel", Decimal::from_parts(3_500, 0, 0, false, 2)),
    ("Hydrogen", Decimal::from_parts(1_000, 0, 0, false, 2)),
];

/// The intensity of one known fuel, or `None` for an unknown name.
pub fn known_fuel_intensity(fuel: &str) -> Option<Decimal> {
    FUEL_INTENSITIES
        .iter()
        .find(|(name, _)| *name == fuel)
        .map(|(_, intensity)| *intensity)
}

/// Weighted lifecycle intensity of a validated mix, in gCO2eq/MJ.
///
/// A [`FuelMix`] holds known fuels only, so each lookup has a table entry.
pub fn weighted_intensity(mix: &FuelMix) -> Decimal {
    mix.entries()
        .iter()
        .map(|(fuel, fraction)| {
            known_fuel_intensity(fuel).expect("a validated mix holds known fuels") * fraction
        })
        .sum()
}

/// FuelEU contribution for a reduction from the reporting-year limit.
///
/// Bands are inclusive at their boundary and expressed in percent, so a mix
/// exactly on the limit scores 70.
pub fn fueleu_band(reduction_pct: Decimal) -> u8 {
    if reduction_pct >= Decimal::from(20) {
        100
    } else if reduction_pct >= Decimal::from(10) {
        90
    } else if reduction_pct >= Decimal::from(5) {
        80
    } else if reduction_pct >= Decimal::ZERO {
        70
    } else if reduction_pct >= Decimal::from(-5) {
        40
    } else if reduction_pct >= Decimal::from(-10) {
        20
    } else {
        0
    }
}

/// A scoring input or calculation that cannot produce a valid decimal result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScoreError {
    NegativeInput,
    ArithmeticOverflow,
}

impl std::fmt::Display for ScoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::NegativeInput => "Scoring amounts must be nonnegative",
            Self::ArithmeticOverflow => "Scoring amount exceeds the supported decimal range",
        })
    }
}

impl std::error::Error for ScoreError {}

/// FuelEU result for one mix and reporting year.
pub fn fueleu_result(
    mix: &FuelMix,
    year: ReportingYear,
    total_energy_mj: Option<Decimal>,
) -> Result<FuelEu, ScoreError> {
    let target = year.target();
    let intensity = weighted_intensity(mix);
    let energy = total_energy_mj.unwrap_or(DEFAULT_ANNUAL_ENERGY_MJ);
    let reduction_pct = rounded_2((target - intensity) / target * Decimal::from(100));
    if energy < Decimal::ZERO {
        return Err(ScoreError::NegativeInput);
    }
    let balance = (target - intensity)
        .checked_mul(energy)
        .ok_or(ScoreError::ArithmeticOverflow)?;
    let estimated_penalty_eur = if balance < Decimal::ZERO && intensity > Decimal::ZERO {
        // Annex IV Part B: the deficit expressed as VLSFO-equivalent tonnes,
        // priced per tonne. A validated mix always has a positive intensity.
        rounded_2(
            (-balance)
                .checked_mul(FUELEU_PENALTY_PER_TONNE)
                .and_then(|deficit| deficit.checked_div(intensity * VLSFO_ENERGY_MJ_PER_TONNE))
                .ok_or(ScoreError::ArithmeticOverflow)?,
        )
    } else {
        rounded_2(Decimal::ZERO)
    };
    Ok(FuelEu {
        ghg_intensity_gco2_mj: rounded_2(intensity),
        target_intensity_gco2_mj: target,
        reduction_pct,
        compliance_balance_gco2: rounded_2(balance),
        estimated_penalty_eur,
        score: fueleu_band(reduction_pct),
    })
}

/// The share of emissions covered by the EU ETS in a given emissions year.
pub fn ets_phase_in(year: i32) -> Decimal {
    match year {
        2024 => Decimal::from(40),
        2025 => Decimal::from(70),
        _ => Decimal::from(100),
    }
}

/// EU ETS estimate for declared in-scope emissions.
///
/// The caller must establish voyage coverage and all applicable gases; a
/// lifecycle intensity is not an emissions input.
pub fn ets_result(
    total_co2_tonnes: Option<Decimal>,
    price_per_tonne_eur: Decimal,
    year: i32,
) -> Result<EuEts, ScoreError> {
    if price_per_tonne_eur < Decimal::ZERO
        || total_co2_tonnes.is_some_and(|value| value < Decimal::ZERO)
    {
        return Err(ScoreError::NegativeInput);
    }
    let phase_in_pct = ets_phase_in(year);
    let Some(tonnes) = total_co2_tonnes else {
        return Ok(EuEts {
            calculation_status: CalculationStatus::Unpriced,
            total_co2_tonnes: None,
            ets_price_per_tonne_eur: price_per_tonne_eur,
            phase_in_pct,
            estimated_cost_eur: None,
            score: 50,
        });
    };
    let cost = tonnes
        .checked_mul(price_per_tonne_eur)
        .and_then(|cost| cost.checked_mul(phase_in_pct / Decimal::from(100)))
        .ok_or(ScoreError::ArithmeticOverflow)?;
    let benchmark = ETS_BENCHMARK_CO2_TONNES
        .checked_mul(price_per_tonne_eur)
        .ok_or(ScoreError::ArithmeticOverflow)?;
    let score = if benchmark > Decimal::ZERO {
        let ratio = cost
            .checked_div(benchmark)
            .ok_or(ScoreError::ArithmeticOverflow)?;
        if ratio <= Decimal::new(3, 1) {
            95
        } else if ratio <= Decimal::new(5, 1) {
            80
        } else if ratio <= Decimal::new(7, 1) {
            60
        } else if ratio <= Decimal::new(9, 1) {
            40
        } else {
            20
        }
    } else {
        50
    };
    Ok(EuEts {
        calculation_status: CalculationStatus::Scenario,
        total_co2_tonnes: Some(tonnes),
        ets_price_per_tonne_eur: price_per_tonne_eur,
        phase_in_pct,
        estimated_cost_eur: Some(rounded_2(cost)),
        score,
    })
}

/// CII result for a rating. An absent rating is the C band; an unrecognised
/// code keeps its normalized text and the neutral contribution.
pub fn cii_result(rating: Option<&str>) -> Cii {
    let rating = rating
        .map(str::to_uppercase)
        .unwrap_or_else(|| "C".to_owned());
    let score = match rating.as_str() {
        "A" => 100,
        "B" => 80,
        "C" => 60,
        "D" => 30,
        "E" => 10,
        _ => 50,
    };
    Cii { rating, score }
}

/// Status and traffic light for an overall score.
pub fn status_for(overall: u8) -> (ComplianceStatus, TrafficLight) {
    match overall {
        90.. => (ComplianceStatus::Excellent, TrafficLight::Green),
        70..=89 => (ComplianceStatus::Good, TrafficLight::Green),
        50..=69 => (ComplianceStatus::Adequate, TrafficLight::Amber),
        30..=49 => (ComplianceStatus::AtRisk, TrafficLight::Amber),
        _ => (ComplianceStatus::Critical, TrafficLight::Red),
    }
}

/// The complete assessment shared by the vessel, fleet and scenario reads.
pub fn compliance_score(input: ScoreInput<'_>) -> Result<ComplianceScore, ScoreError> {
    let fueleu = fueleu_result(input.fuel_mix, input.year, input.total_energy_mj)?;
    let eu_ets = ets_result(
        input.total_co2_tonnes,
        DEFAULT_ETS_PRICE_EUR,
        input.year.get(),
    )?;
    let cii = cii_result(input.cii_rating);
    let weighted = Decimal::from(u32::from(fueleu.score)) * FUEL_EU_WEIGHT
        + Decimal::from(u32::from(eu_ets.score)) * EU_ETS_WEIGHT
        + Decimal::from(u32::from(cii.score)) * CII_WEIGHT;
    let overall_score = weighted
        .round_dp_with_strategy(0, RoundingStrategy::MidpointNearestEven)
        .to_u8()
        .expect("a weighted score lies between 0 and 100");
    let (status, traffic_light) = status_for(overall_score);
    let mut recommendations = Vec::new();
    if fueleu.score < 70 {
        recommendations.push(
            "Consider increasing biofuel or methanol blend to meet FuelEU targets".to_owned(),
        );
    }
    if eu_ets.score < 50 {
        recommendations
            .push("High ETS exposure: evaluate alternative fuels to reduce CO2 costs".to_owned());
    }
    if cii.score < 60 {
        recommendations.push(
            "CII rating declining: implement speed reduction or efficiency measures".to_owned(),
        );
    }
    if fueleu.estimated_penalty_eur > Decimal::ZERO {
        recommendations.push(format!(
            "Illustrative FuelEU deficit penalty: EUR {}. Verify annual eligible energy and flexibility measures.",
            fueleu
                .estimated_penalty_eur
                .round_dp_with_strategy(0, RoundingStrategy::MidpointNearestEven)
        ));
    }
    Ok(ComplianceScore {
        vessel_id: input.vessel_id.to_owned(),
        vessel_name: input.vessel_name.to_owned(),
        overall_score,
        status,
        traffic_light,
        fueleu,
        eu_ets,
        cii,
        recommendations,
    })
}

/// FuelEU contribution of one assessment.
pub struct FuelEu {
    pub ghg_intensity_gco2_mj: Decimal,
    pub target_intensity_gco2_mj: Decimal,
    pub reduction_pct: Decimal,
    pub compliance_balance_gco2: Decimal,
    pub estimated_penalty_eur: Decimal,
    pub score: u8,
}

/// Whether the ETS estimate is priced for declared emissions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CalculationStatus {
    Unpriced,
    Scenario,
}

impl CalculationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unpriced => "UNPRICED",
            Self::Scenario => "SCENARIO",
        }
    }
}

/// EU ETS contribution of one assessment.
pub struct EuEts {
    pub calculation_status: CalculationStatus,
    pub total_co2_tonnes: Option<Decimal>,
    pub ets_price_per_tonne_eur: Decimal,
    pub phase_in_pct: Decimal,
    pub estimated_cost_eur: Option<Decimal>,
    pub score: u8,
}

/// CII contribution of one assessment.
pub struct Cii {
    pub rating: String,
    pub score: u8,
}

/// Overall assessment band.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComplianceStatus {
    Excellent,
    Good,
    Adequate,
    AtRisk,
    Critical,
}

impl ComplianceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Excellent => "EXCELLENT",
            Self::Good => "GOOD",
            Self::Adequate => "ADEQUATE",
            Self::AtRisk => "AT_RISK",
            Self::Critical => "CRITICAL",
        }
    }
}

/// Overall traffic light.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrafficLight {
    Green,
    Amber,
    Red,
}

impl TrafficLight {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Green => "GREEN",
            Self::Amber => "AMBER",
            Self::Red => "RED",
        }
    }
}

/// One complete compliance assessment.
pub struct ComplianceScore {
    pub vessel_id: String,
    pub vessel_name: String,
    pub overall_score: u8,
    pub status: ComplianceStatus,
    pub traffic_light: TrafficLight,
    pub fueleu: FuelEu,
    pub eu_ets: EuEts,
    pub cii: Cii,
    pub recommendations: Vec<String>,
}

impl ComplianceScore {
    /// The response body the three score reads return.
    pub fn to_json(&self) -> Json {
        json!({
            "vessel_id": self.vessel_id,
            "vessel_name": self.vessel_name,
            "overall_score": self.overall_score,
            "status": self.status.as_str(),
            "traffic_light": self.traffic_light.as_str(),
            "fueleu": {
                "ghg_intensity_gco2_mj": self.fueleu.ghg_intensity_gco2_mj.to_string(),
                "target_intensity_gco2_mj": self.fueleu.target_intensity_gco2_mj.to_string(),
                "reduction_pct": self.fueleu.reduction_pct.to_string(),
                "compliance_balance_gco2": self.fueleu.compliance_balance_gco2.to_string(),
                "estimated_penalty_eur": self.fueleu.estimated_penalty_eur.to_string(),
                "score": self.fueleu.score,
            },
            "eu_ets": {
                "calculation_status": self.eu_ets.calculation_status.as_str(),
                "total_co2_tonnes": self.eu_ets.total_co2_tonnes.map(|value| value.to_string()),
                "ets_price_per_tonne_eur": self.eu_ets.ets_price_per_tonne_eur.to_string(),
                "phase_in_pct": self.eu_ets.phase_in_pct.to_string(),
                "estimated_cost_eur": self.eu_ets.estimated_cost_eur.map(|value| value.to_string()),
                "score": self.eu_ets.score,
            },
            "cii": {"rating": self.cii.rating, "score": self.cii.score},
            "recommendations": self.recommendations,
        })
    }
}

/// Inputs of one assessment. The mix is already validated and the year is
/// already inside the shared schedule. Amounts are checked before arithmetic.
pub struct ScoreInput<'a> {
    pub vessel_id: &'a str,
    pub vessel_name: &'a str,
    pub fuel_mix: &'a FuelMix,
    pub cii_rating: Option<&'a str>,
    pub total_co2_tonnes: Option<Decimal>,
    pub year: ReportingYear,
    pub total_energy_mj: Option<Decimal>,
}

fn rounded_2(value: Decimal) -> Decimal {
    let mut rounded = value.round_dp_with_strategy(2, RoundingStrategy::MidpointNearestEven);
    rounded.rescale(2);
    rounded
}

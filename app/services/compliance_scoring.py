"""
Vessel compliance scoring engine.

Calculates a single compliance score (0-100) for each vessel based on:
- FuelEU Maritime GHG intensity compliance
- EU ETS emission costs
- CII (Carbon Intensity Indicator) rating
- Overall fuel mix optimization

Score interpretation:
- 90-100: Excellent (GREEN) - Well below regulatory limits
- 70-89: Good (GREEN) - Compliant with margin
- 50-69: Adequate (AMBER) - Compliant but close to limits
- 30-49: At Risk (AMBER) - Approaching non-compliance
- 0-29: Critical (RED) - Non-compliant or projected non-compliance
"""
from decimal import Decimal
from dataclasses import dataclass, field
from typing import Optional
from enum import Enum

from app.services.fueleu import FUELEU_TARGETS, fueleu_year_target


class ComplianceStatus(str, Enum):
    EXCELLENT = "EXCELLENT"
    GOOD = "GOOD"
    ADEQUATE = "ADEQUATE"
    AT_RISK = "AT_RISK"
    CRITICAL = "CRITICAL"


class TrafficLight(str, Enum):
    GREEN = "GREEN"
    AMBER = "AMBER"
    RED = "RED"


@dataclass
class FuelEUResult:
    """FuelEU Maritime compliance calculation result."""
    ghg_intensity_gco2_mj: Decimal  # Vessel's actual GHG intensity
    target_intensity_gco2_mj: Decimal  # Regulatory target for the year
    reduction_pct: Decimal  # % below target (positive = compliant)
    compliance_balance_gco2: Decimal  # Surplus (positive) or deficit (negative)
    estimated_penalty_eur: Decimal  # EUR 0 if compliant
    score: int  # 0-100 contribution to overall score


@dataclass
class EUETSResult:
    """EU ETS maritime emission cost estimate."""
    total_co2_tonnes: Optional[Decimal]
    ets_price_per_tonne_eur: Decimal = Decimal("68")  # Illustrative EUA price, not a live quote
    phase_in_pct: Decimal = Decimal("100")  # 2026 emissions, surrendered in 2027
    estimated_cost_eur: Optional[Decimal] = None
    calculation_status: str = "UNPRICED"
    score: int = 50  # 0-100


@dataclass
class CIIResult:
    """CII rating assessment."""
    rating: str  # A, B, C, D, E
    attained_cii: Optional[Decimal] = None
    required_cii: Optional[Decimal] = None
    score: int = 50


@dataclass
class ComplianceScore:
    """Complete compliance assessment for a vessel."""
    vessel_id: str
    vessel_name: str
    overall_score: int  # 0-100
    status: ComplianceStatus
    traffic_light: TrafficLight
    fueleu: FuelEUResult
    eu_ets: EUETSResult
    cii: CIIResult
    recommendations: list[str] = field(default_factory=list)


# FuelEU penalty: EUR 2,400 per tonne of VLSFO equivalent
FUELEU_PENALTY_PER_TONNE = Decimal("2400")

# Fuel GHG intensities (gCO2eq/MJ, well-to-wake)
FUEL_GHG_INTENSITIES = {
    "VLSFO": Decimal("91.16"),
    "LSMGO": Decimal("91.16"),
    "LNG": Decimal("69.00"),
    "Bio-LNG": Decimal("28.00"),
    "Methanol": Decimal("31.00"),   # Bio-methanol (green)
    "E-Methanol": Decimal("8.00"),  # Renewable electricity methanol
    "Ammonia": Decimal("18.00"),    # Green ammonia
    "Biofuel": Decimal("35.00"),    # Typical biodiesel
    "Hydrogen": Decimal("10.00"),   # Green hydrogen
}

# Default energy consumption per vessel type (MJ/year estimate)
DEFAULT_ENERGY_MJ = {
    "container": Decimal("500000000"),
    "bulk_carrier": Decimal("300000000"),
    "tanker": Decimal("400000000"),
    "general_cargo": Decimal("200000000"),
    "default": Decimal("300000000"),
}


def calculate_fueleu_score(
    fuel_mix: dict[str, Decimal],  # {"Methanol": Decimal("0.3"), "VLSFO": Decimal("0.7")}
    year: int = 2026,
    total_energy_mj: Optional[Decimal] = None,
) -> FuelEUResult:
    """Calculate FuelEU Maritime compliance for a vessel's fuel mix."""
    target = fueleu_year_target(year)
    total_energy = total_energy_mj or DEFAULT_ENERGY_MJ["default"]

    # Calculate weighted average GHG intensity
    weighted_intensity = Decimal("0")
    for fuel, fraction in fuel_mix.items():
        intensity = FUEL_GHG_INTENSITIES.get(fuel, Decimal("91.16"))
        weighted_intensity += intensity * fraction

    # Reduction percentage (positive = compliant)
    if target > 0:
        reduction_pct = ((target - weighted_intensity) / target * 100).quantize(Decimal("0.01"))
    else:
        reduction_pct = Decimal("0")

    # Compliance balance in gCO2 (positive = surplus, negative = deficit)
    compliance_balance = (target - weighted_intensity) * total_energy

    # Annex IV Part B: first-year deficit penalty for the assumed eligible
    # annual energy. This scenario excludes banking, borrowing and pooling.
    if compliance_balance < 0:
        penalty = (
            -compliance_balance
            * FUELEU_PENALTY_PER_TONNE
            / (weighted_intensity * Decimal("41000"))
        )
    else:
        penalty = Decimal("0")

    # Score (0-100)
    if reduction_pct >= Decimal("20"):
        score = 100
    elif reduction_pct >= Decimal("10"):
        score = 90
    elif reduction_pct >= Decimal("5"):
        score = 80
    elif reduction_pct >= Decimal("0"):
        score = 70  # Just compliant
    elif reduction_pct >= Decimal("-5"):
        score = 40  # Slightly over
    elif reduction_pct >= Decimal("-10"):
        score = 20
    else:
        score = 0

    return FuelEUResult(
        ghg_intensity_gco2_mj=weighted_intensity.quantize(Decimal("0.01")),
        target_intensity_gco2_mj=target,
        reduction_pct=reduction_pct,
        compliance_balance_gco2=compliance_balance.quantize(Decimal("0.01")),
        estimated_penalty_eur=penalty.quantize(Decimal("0.01")),
        score=score,
    )


def calculate_ets_score(
    total_co2_tonnes: Optional[Decimal],
    ets_price_eur: Decimal = Decimal("68"),
    year: int = 2026,
) -> EUETSResult:
    """Price declared in-scope emissions for the emissions year, not surrender year.

    This simplified scenario requires the caller to establish voyage coverage
    and all applicable gases; WtW CI cannot establish those emissions.
    """
    # Phase-in schedule
    phase_in = {2024: Decimal("40"), 2025: Decimal("70"), 2026: Decimal("100")}
    pct = phase_in.get(year, Decimal("100"))

    if total_co2_tonnes is None:
        return EUETSResult(
            total_co2_tonnes=None,
            ets_price_per_tonne_eur=ets_price_eur,
            phase_in_pct=pct,
            estimated_cost_eur=None,
            calculation_status="UNPRICED",
            score=50,
        )

    cost = total_co2_tonnes * ets_price_eur * pct / Decimal("100")

    # Score: lower cost per tonne = better (benchmark against VLSFO vessel)
    # A typical 5000 TEU container ship emits ~50,000 tCO2/year
    benchmark_cost = Decimal("50000") * ets_price_eur
    if benchmark_cost > 0:
        ratio = cost / benchmark_cost
        if ratio <= Decimal("0.3"):
            score = 95
        elif ratio <= Decimal("0.5"):
            score = 80
        elif ratio <= Decimal("0.7"):
            score = 60
        elif ratio <= Decimal("0.9"):
            score = 40
        else:
            score = 20
    else:
        score = 50

    return EUETSResult(
        total_co2_tonnes=total_co2_tonnes,
        calculation_status="SCENARIO",
        ets_price_per_tonne_eur=ets_price_eur,
        phase_in_pct=pct,
        estimated_cost_eur=cost.quantize(Decimal("0.01")),
        score=score,
    )


def calculate_cii_score(cii_rating: Optional[str]) -> CIIResult:
    """Score based on CII rating."""
    rating_scores = {"A": 100, "B": 80, "C": 60, "D": 30, "E": 10}
    rating = (cii_rating or "C").upper()
    score = rating_scores.get(rating, 50)
    return CIIResult(rating=rating, score=score)


def calculate_compliance_score(
    vessel_id: str,
    vessel_name: str,
    fuel_mix: dict[str, Decimal] = None,
    cii_rating: str = None,
    total_co2_tonnes: Decimal = None,
    vessel_type: str = "default",
    year: int = 2026,
) -> ComplianceScore:
    """
    Calculate overall compliance score for a vessel.

    Weights: FuelEU 50%, EU ETS 30%, CII 20%
    """
    # Default to 100% VLSFO if no fuel mix provided
    if not fuel_mix:
        fuel_mix = {"VLSFO": Decimal("1.0")}

    # Vessel category and WtW CI cannot establish in-scope combustion
    # emissions. An absent emissions input stays unpriced, never inferred.
    fueleu = calculate_fueleu_score(fuel_mix, year)
    eu_ets = calculate_ets_score(total_co2_tonnes, year=year)
    cii = calculate_cii_score(cii_rating)

    # Weighted average (FuelEU 50%, ETS 30%, CII 20%)
    overall = round(fueleu.score * Decimal("0.5") + eu_ets.score * Decimal("0.3") + cii.score * Decimal("0.2"))

    # Determine status and traffic light
    if overall >= 90:
        status = ComplianceStatus.EXCELLENT
        light = TrafficLight.GREEN
    elif overall >= 70:
        status = ComplianceStatus.GOOD
        light = TrafficLight.GREEN
    elif overall >= 50:
        status = ComplianceStatus.ADEQUATE
        light = TrafficLight.AMBER
    elif overall >= 30:
        status = ComplianceStatus.AT_RISK
        light = TrafficLight.AMBER
    else:
        status = ComplianceStatus.CRITICAL
        light = TrafficLight.RED

    # Generate recommendations
    recommendations = []
    if fueleu.score < 70:
        recommendations.append("Consider increasing bio-fuel or methanol blend to meet FuelEU targets")
    if eu_ets.score < 50:
        recommendations.append("High ETS exposure — evaluate alternative fuel options to reduce CO2 costs")
    if cii.score < 60:
        recommendations.append("CII rating declining — implement speed reduction or efficiency measures")
    if fueleu.estimated_penalty_eur > 0:
        recommendations.append(f"Illustrative FuelEU deficit penalty: EUR {fueleu.estimated_penalty_eur:,.0f}. Verify annual eligible energy and flexibility measures.")

    return ComplianceScore(
        vessel_id=vessel_id,
        vessel_name=vessel_name,
        overall_score=overall,
        status=status,
        traffic_light=light,
        fueleu=fueleu,
        eu_ets=eu_ets,
        cii=cii,
        recommendations=recommendations,
    )

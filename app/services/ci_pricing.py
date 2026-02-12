"""
Carbon Intensity (CI) adjusted pricing service.

Calculates the effective price of a marine fuel by accounting for the
compliance cost differential based on carbon intensity.

Reference:
- FuelEU Maritime reference value: 91 gCO2eq/MJ (VLSFO baseline, 2025)
- EU ETS carbon price: ~65 EUR/tCO2 (~70 USD) - configurable
- FuelEU penalty: 2,400 EUR/tonne of VLSFO equivalent - configurable
"""
from decimal import Decimal, ROUND_HALF_UP

from app.schemas.orderbook import CIAdjustedPrice

# FuelEU Maritime 2025 reference GHG intensity (gCO2eq/MJ)
FUELEU_REFERENCE_GHG = Decimal("91.0")

# EU ETS carbon price in USD/tCO2 (approximate, should be configurable later)
EU_ETS_PRICE_USD = Decimal("70.0")

# Conversion: 1 tonne CO2 = 1_000_000 grams
GRAMS_PER_TONNE = Decimal("1000000")


def calculate_ci_adjusted_price(
    base_price_per_mt: Decimal,
    carbon_intensity_gco2_mj: Decimal,
    energy_density_mj_kg: Decimal,
) -> CIAdjustedPrice:
    """
    Calculate CI-adjusted effective price.

    The compliance cost differential represents the value of avoiding
    carbon costs when using a lower-CI fuel instead of VLSFO.

    Formula:
        ghg_reduction = (ref_ci - fuel_ci) / ref_ci * 100
        co2_avoided_per_mt = (ref_ci - fuel_ci) * energy_density * 1000 / 1_000_000  (tCO2/mt)
        compliance_savings = co2_avoided_per_mt * eu_ets_price
        effective_price = base_price - compliance_savings (savings reduce effective cost)

    If fuel CI >= reference CI, compliance_cost = 0 (no savings).
    """
    ghg_reduction_pct = Decimal("0")
    compliance_cost_per_mt = Decimal("0")

    if carbon_intensity_gco2_mj < FUELEU_REFERENCE_GHG:
        # GHG reduction percentage
        ghg_reduction_pct = (
            (FUELEU_REFERENCE_GHG - carbon_intensity_gco2_mj)
            / FUELEU_REFERENCE_GHG
            * Decimal("100")
        ).quantize(Decimal("0.1"), rounding=ROUND_HALF_UP)

        # CO2 avoided per metric tonne of fuel
        co2_avoided_per_mt = (
            (FUELEU_REFERENCE_GHG - carbon_intensity_gco2_mj)
            * energy_density_mj_kg
            * Decimal("1000")
            / GRAMS_PER_TONNE
        )

        # Compliance savings (negative cost = benefit)
        compliance_cost_per_mt = -(co2_avoided_per_mt * EU_ETS_PRICE_USD).quantize(
            Decimal("0.01"), rounding=ROUND_HALF_UP
        )

    effective_price = (base_price_per_mt + compliance_cost_per_mt).quantize(
        Decimal("0.01"), rounding=ROUND_HALF_UP
    )

    return CIAdjustedPrice(
        base_price_per_mt=base_price_per_mt,
        carbon_intensity_gco2_mj=carbon_intensity_gco2_mj,
        fueleu_ghg_intensity=FUELEU_REFERENCE_GHG,
        compliance_cost_per_mt=compliance_cost_per_mt,
        effective_price_per_mt=effective_price,
        ghg_reduction_pct=ghg_reduction_pct,
    )

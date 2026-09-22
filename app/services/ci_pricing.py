"""Declared lifecycle-intensity comparison without inferred cash benefits.

WtW intensity does not establish the combustion emissions, voyage scope,
biomass eligibility, emissions year, EUA price or FX needed for maritime
EU ETS. Legacy monetary fields remain zero until these inputs are supported.
"""
from decimal import Decimal, ROUND_HALF_UP

from app.schemas.orderbook import CIAdjustedPrice
from app.services.fueleu import FUELEU_REFERENCE_GHG


def calculate_ci_adjusted_price(
    base_price_per_mt: Decimal,
    carbon_intensity_gco2_mj: Decimal,
    energy_density_mj_kg: Decimal,
) -> CIAdjustedPrice:
    """Compare declared WtW CI with the reference; leave the price unchanged.

    The energy-density argument is retained for existing callers. Neither
    it nor a low lifecycle CI is proof of an ETS or FuelEU cash benefit.
    """
    ghg_reduction_pct = max(
        Decimal("0"),
        (FUELEU_REFERENCE_GHG - carbon_intensity_gco2_mj)
        / FUELEU_REFERENCE_GHG
        * Decimal("100"),
    ).quantize(Decimal("0.1"), rounding=ROUND_HALF_UP)

    return CIAdjustedPrice(
        base_price_per_mt=base_price_per_mt,
        carbon_intensity_gco2_mj=carbon_intensity_gco2_mj,
        fueleu_ghg_intensity=FUELEU_REFERENCE_GHG,
        compliance_cost_per_mt=Decimal("0.00"),
        effective_price_per_mt=base_price_per_mt.quantize(
            Decimal("0.01"), rounding=ROUND_HALF_UP
        ),
        ghg_reduction_pct=ghg_reduction_pct,
    )

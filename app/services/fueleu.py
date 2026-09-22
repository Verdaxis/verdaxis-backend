"""Shared FuelEU intensity limits from Regulation (EU) 2023/1805, Article 4.

Source: https://eur-lex.europa.eu/eli/reg/2023/1805/oj/eng
The reporting-year limit is a reduction from 91.16 gCO2e/MJ, not a
fuel-specific certificate or a monetary benefit.
"""
from decimal import Decimal

FUELEU_REFERENCE_GHG = Decimal("91.16")
FUELEU_REDUCTIONS = {
    2025: Decimal("0.02"),
    2030: Decimal("0.06"),
    2035: Decimal("0.145"),
    2040: Decimal("0.31"),
    2045: Decimal("0.62"),
    2050: Decimal("0.80"),
}
FUELEU_TARGETS = {
    year: FUELEU_REFERENCE_GHG * (Decimal("1") - reduction)
    for year, reduction in FUELEU_REDUCTIONS.items()
}


def fueleu_year_target(year: int) -> Decimal:
    """Return the exact limit for a reporting year from 2025 onwards."""
    if year < 2025:
        raise ValueError("FuelEU reporting starts in 2025")
    for first_year, target in reversed(tuple(FUELEU_TARGETS.items())):
        if year >= first_year:
            return target
    raise AssertionError("FuelEU target schedule does not cover the year")

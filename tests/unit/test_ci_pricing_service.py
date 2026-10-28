"""Lifecycle reductions alone must never reduce an executable fuel price."""
from decimal import Decimal

import pytest

from app.services.ci_pricing import calculate_ci_adjusted_price


@pytest.mark.parametrize("ci", ["0", "8", "15.3", "30", "91.16", "95"])
def test_lifecycle_ci_does_not_create_ets_savings(ci):
    result = calculate_ci_adjusted_price(
        base_price_per_mt=Decimal("540.00"),
        carbon_intensity_gco2_mj=Decimal(ci),
        energy_density_mj_kg=Decimal("19.9"),
    )
    assert result.compliance_cost_per_mt == Decimal("0.00")
    assert result.effective_price_per_mt == Decimal("540.00")
    assert result.fueleu_ghg_intensity == Decimal("91.16")
    assert result.carbon_intensity_gco2_mj == Decimal(ci)


def test_declared_lifecycle_reduction_is_preserved():
    result = calculate_ci_adjusted_price(
        base_price_per_mt=Decimal("1100"),
        carbon_intensity_gco2_mj=Decimal("20"),
        energy_density_mj_kg=Decimal("37"),
    )
    assert result.ghg_reduction_pct == Decimal("78.1")

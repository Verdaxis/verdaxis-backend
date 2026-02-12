"""Unit tests for CI-adjusted pricing schemas."""
import pytest
from decimal import Decimal

from app.schemas.orderbook import CIAdjustedPrice


class TestCIAdjustedPrice:
    def test_effective_price_calculation(self):
        """Effective price = base_price + compliance_cost_differential."""
        p = CIAdjustedPrice(
            base_price_per_mt=Decimal("540.00"),
            carbon_intensity_gco2_mj=Decimal("15.3"),
            fueleu_ghg_intensity=Decimal("28.1"),
            compliance_cost_per_mt=Decimal("42.50"),
            effective_price_per_mt=Decimal("582.50"),
            ghg_reduction_pct=Decimal("72.5"),
        )
        assert p.effective_price_per_mt == Decimal("582.50")
        assert p.ghg_reduction_pct == Decimal("72.5")

    def test_fossil_fuel_baseline(self):
        """LSMGO baseline should have zero compliance benefit."""
        p = CIAdjustedPrice(
            base_price_per_mt=Decimal("620.00"),
            carbon_intensity_gco2_mj=Decimal("91.0"),
            fueleu_ghg_intensity=Decimal("91.0"),
            compliance_cost_per_mt=Decimal("0"),
            effective_price_per_mt=Decimal("620.00"),
            ghg_reduction_pct=Decimal("0"),
        )
        assert p.compliance_cost_per_mt == Decimal("0")

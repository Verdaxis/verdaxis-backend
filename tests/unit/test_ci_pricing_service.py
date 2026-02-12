"""Unit tests for CI pricing calculation service."""
import pytest
from decimal import Decimal

from app.services.ci_pricing import calculate_ci_adjusted_price


class TestCIPricingService:
    def test_green_methanol_reduces_effective_cost(self):
        """Green methanol (CI ~15 gCO2/MJ) should show compliance savings."""
        result = calculate_ci_adjusted_price(
            base_price_per_mt=Decimal("540.00"),
            carbon_intensity_gco2_mj=Decimal("15.3"),
            energy_density_mj_kg=Decimal("19.9"),
        )
        assert result.base_price_per_mt == Decimal("540.00")
        assert result.carbon_intensity_gco2_mj == Decimal("15.3")
        assert result.ghg_reduction_pct > Decimal("0")
        assert result.effective_price_per_mt is not None

    def test_fossil_lsmgo_baseline(self):
        """LSMGO (CI ~91 gCO2/MJ) is the reference -- zero compliance benefit."""
        result = calculate_ci_adjusted_price(
            base_price_per_mt=Decimal("620.00"),
            carbon_intensity_gco2_mj=Decimal("91.0"),
            energy_density_mj_kg=Decimal("42.7"),
        )
        assert result.ghg_reduction_pct == Decimal("0")
        assert result.compliance_cost_per_mt == Decimal("0")
        assert result.effective_price_per_mt == Decimal("620.00")

    def test_bio_methanol_intermediate(self):
        """Bio-methanol (~30 gCO2/MJ) should have moderate savings."""
        result = calculate_ci_adjusted_price(
            base_price_per_mt=Decimal("580.00"),
            carbon_intensity_gco2_mj=Decimal("30.0"),
            energy_density_mj_kg=Decimal("19.9"),
        )
        assert Decimal("0") < result.ghg_reduction_pct < Decimal("100")

    def test_ammonia_green(self):
        """Green ammonia (~8 gCO2/MJ) should have large savings."""
        result = calculate_ci_adjusted_price(
            base_price_per_mt=Decimal("450.00"),
            carbon_intensity_gco2_mj=Decimal("8.0"),
            energy_density_mj_kg=Decimal("18.6"),
        )
        assert result.ghg_reduction_pct > Decimal("80")

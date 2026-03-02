"""Unit tests for vessel compliance scoring engine."""
import pytest
from decimal import Decimal

from app.services.compliance_scoring import (
    calculate_fueleu_score,
    calculate_ets_score,
    calculate_cii_score,
    calculate_compliance_score,
    ComplianceStatus,
    TrafficLight,
    FUELEU_TARGETS,
    FUEL_GHG_INTENSITIES,
)


class TestFuelEUScore:
    """Tests for FuelEU Maritime compliance scoring."""

    def test_100pct_vlsfo_barely_noncompliant(self):
        """100% VLSFO (91.16 gCO2/MJ) is above the 2026 target (89.34).
        Should be non-compliant with a penalty and a score below 70."""
        result = calculate_fueleu_score(
            fuel_mix={"VLSFO": Decimal("1.0")},
            year=2026,
        )
        # VLSFO intensity (91.16) > target (89.34), so reduction_pct is negative
        assert result.ghg_intensity_gco2_mj == Decimal("91.16")
        assert result.target_intensity_gco2_mj == Decimal("89.34")
        assert result.reduction_pct < Decimal("0")
        assert result.estimated_penalty_eur > Decimal("0")
        # Slightly over target -> score 40 (within -5% band)
        assert result.score == 40

    def test_50pct_methanol_50pct_vlsfo_high_score(self):
        """50% Bio-methanol (31 gCO2/MJ) + 50% VLSFO (91.16 gCO2/MJ)
        = weighted 61.08 gCO2/MJ, well below 89.34 target."""
        result = calculate_fueleu_score(
            fuel_mix={"Methanol": Decimal("0.5"), "VLSFO": Decimal("0.5")},
            year=2026,
        )
        expected_intensity = Decimal("31.00") * Decimal("0.5") + Decimal("91.16") * Decimal("0.5")
        assert result.ghg_intensity_gco2_mj == expected_intensity.quantize(Decimal("0.01"))
        assert result.reduction_pct > Decimal("0")
        assert result.estimated_penalty_eur == Decimal("0")
        # ~31.6% reduction -> score 100
        assert result.score == 100

    def test_100pct_e_methanol_score_100(self):
        """100% E-Methanol (8 gCO2/MJ) is massively below any target. Score = 100."""
        result = calculate_fueleu_score(
            fuel_mix={"E-Methanol": Decimal("1.0")},
            year=2026,
        )
        assert result.ghg_intensity_gco2_mj == Decimal("8.00")
        assert result.reduction_pct > Decimal("90")
        assert result.estimated_penalty_eur == Decimal("0")
        assert result.score == 100

    def test_100pct_lng_compliant(self):
        """100% LNG (69 gCO2/MJ) is below the 2026 target (89.34). Score >= 70."""
        result = calculate_fueleu_score(
            fuel_mix={"LNG": Decimal("1.0")},
            year=2026,
        )
        assert result.ghg_intensity_gco2_mj == Decimal("69.00")
        assert result.reduction_pct > Decimal("0")
        assert result.estimated_penalty_eur == Decimal("0")
        assert result.score >= 70

    def test_2030_target_stricter(self):
        """The 2030 target (80.04) is stricter than 2026 (89.34).
        100% LNG should still be compliant in 2030 but with less margin."""
        result_2026 = calculate_fueleu_score(
            fuel_mix={"LNG": Decimal("1.0")},
            year=2026,
        )
        result_2030 = calculate_fueleu_score(
            fuel_mix={"LNG": Decimal("1.0")},
            year=2030,
        )
        assert result_2030.target_intensity_gco2_mj == Decimal("80.04")
        # Both compliant, but 2026 has more reduction margin
        assert result_2026.reduction_pct > result_2030.reduction_pct

    def test_2025_vs_2026_same_target(self):
        """2025 and 2026 have the same target: 89.34 gCO2/MJ."""
        result_2025 = calculate_fueleu_score(
            fuel_mix={"VLSFO": Decimal("1.0")},
            year=2025,
        )
        result_2026 = calculate_fueleu_score(
            fuel_mix={"VLSFO": Decimal("1.0")},
            year=2026,
        )
        assert result_2025.target_intensity_gco2_mj == result_2026.target_intensity_gco2_mj

    def test_penalty_calculation_nonzero(self):
        """When intensity exceeds target, penalty should be positive."""
        result = calculate_fueleu_score(
            fuel_mix={"VLSFO": Decimal("1.0")},
            year=2026,
        )
        # VLSFO 91.16 > target 89.34 => penalty > 0
        assert result.estimated_penalty_eur > Decimal("0")
        # Penalty should be a reasonable number (not astronomical)
        assert result.estimated_penalty_eur < Decimal("10000000")

    def test_custom_energy_consumption(self):
        """Passing custom total_energy_mj changes the compliance balance magnitude."""
        small_energy = Decimal("100000000")
        large_energy = Decimal("500000000")

        result_small = calculate_fueleu_score(
            fuel_mix={"Methanol": Decimal("1.0")},
            year=2026,
            total_energy_mj=small_energy,
        )
        result_large = calculate_fueleu_score(
            fuel_mix={"Methanol": Decimal("1.0")},
            year=2026,
            total_energy_mj=large_energy,
        )
        # Same fuel mix -> same score, same reduction_pct
        assert result_small.score == result_large.score
        assert result_small.reduction_pct == result_large.reduction_pct
        # But larger energy -> larger compliance balance
        assert abs(result_large.compliance_balance_gco2) > abs(result_small.compliance_balance_gco2)

    def test_unknown_fuel_defaults_to_vlsfo_intensity(self):
        """An unknown fuel type should default to VLSFO intensity (91.16)."""
        result = calculate_fueleu_score(
            fuel_mix={"SomeFutureFuel": Decimal("1.0")},
            year=2026,
        )
        assert result.ghg_intensity_gco2_mj == Decimal("91.16")

    def test_score_bands_boundary(self):
        """Verify score bands at reduction percentage boundaries."""
        # Build a fuel mix that achieves exactly 0% reduction
        # Target is 89.34, so need intensity == 89.34
        # Use a blend of E-Methanol (8) and VLSFO (91.16) to hit 89.34
        # 8*x + 91.16*(1-x) = 89.34 -> 8x + 91.16 - 91.16x = 89.34
        # -83.16x = -1.82 -> x = 1.82/83.16 = 0.02189...
        # At exactly 0% reduction, score should be 70
        result = calculate_fueleu_score(
            fuel_mix={"E-Methanol": Decimal("0.02189"), "VLSFO": Decimal("0.97811")},
            year=2026,
        )
        # Should be very close to 0% reduction (compliant but barely)
        assert result.score == 70


class TestEUETSScore:
    """Tests for EU ETS maritime cost scoring."""

    def test_low_emissions_high_score(self):
        """Low CO2 emissions relative to benchmark should score high."""
        result = calculate_ets_score(
            total_co2_tonnes=Decimal("10000"),
            ets_price_eur=Decimal("68"),
            year=2026,
        )
        # 10,000 / 50,000 benchmark = 0.2 ratio -> score 95
        assert result.score == 95
        assert result.phase_in_pct == Decimal("100")

    def test_high_emissions_low_score(self):
        """High CO2 emissions should score low."""
        result = calculate_ets_score(
            total_co2_tonnes=Decimal("55000"),
            ets_price_eur=Decimal("68"),
            year=2026,
        )
        # 55,000 / 50,000 = 1.1 ratio -> score 20
        assert result.score == 20

    def test_2024_phase_in_40pct(self):
        """In 2024, only 40% of emissions are covered by ETS."""
        result = calculate_ets_score(
            total_co2_tonnes=Decimal("50000"),
            ets_price_eur=Decimal("68"),
            year=2024,
        )
        assert result.phase_in_pct == Decimal("40")
        expected_cost = Decimal("50000") * Decimal("68") * Decimal("40") / Decimal("100")
        assert result.estimated_cost_eur == expected_cost.quantize(Decimal("0.01"))

    def test_2025_phase_in_70pct(self):
        """In 2025, 70% of emissions are covered."""
        result = calculate_ets_score(
            total_co2_tonnes=Decimal("50000"),
            year=2025,
        )
        assert result.phase_in_pct == Decimal("70")

    def test_2026_phase_in_100pct(self):
        """In 2026, 100% of emissions are covered."""
        result = calculate_ets_score(
            total_co2_tonnes=Decimal("50000"),
            year=2026,
        )
        assert result.phase_in_pct == Decimal("100")

    def test_cost_calculation_correct(self):
        """Verify cost = tonnes * price * phase_in_pct / 100."""
        result = calculate_ets_score(
            total_co2_tonnes=Decimal("30000"),
            ets_price_eur=Decimal("70"),
            year=2026,
        )
        expected = Decimal("30000") * Decimal("70") * Decimal("100") / Decimal("100")
        assert result.estimated_cost_eur == expected.quantize(Decimal("0.01"))


class TestCIIScore:
    """Tests for CII rating scoring."""

    def test_rating_a_score_100(self):
        assert calculate_cii_score("A").score == 100

    def test_rating_b_score_80(self):
        assert calculate_cii_score("B").score == 80

    def test_rating_c_score_60(self):
        assert calculate_cii_score("C").score == 60

    def test_rating_d_score_30(self):
        assert calculate_cii_score("D").score == 30

    def test_rating_e_score_10(self):
        assert calculate_cii_score("E").score == 10

    def test_none_defaults_to_c(self):
        """When no CII rating provided, default to C (score 60)."""
        result = calculate_cii_score(None)
        assert result.rating == "C"
        assert result.score == 60

    def test_lowercase_normalized(self):
        """Lowercase input should be normalized to uppercase."""
        result = calculate_cii_score("a")
        assert result.rating == "A"
        assert result.score == 100


class TestOverallComplianceScore:
    """Tests for the combined compliance score calculation."""

    def test_weighting_50_30_20(self):
        """Overall score = FuelEU(50%) + ETS(30%) + CII(20%)."""
        # Use known inputs to compute expected score
        score = calculate_compliance_score(
            vessel_id="test-001",
            vessel_name="Test Vessel",
            fuel_mix={"E-Methanol": Decimal("1.0")},  # FuelEU score = 100
            cii_rating="A",  # CII score = 100
            total_co2_tonnes=Decimal("10000"),  # ETS score = 95 (low emissions)
            year=2026,
        )
        # 100*0.5 + 95*0.3 + 100*0.2 = 50 + 28.5 + 20 = 98
        assert score.overall_score == 98
        assert score.status == ComplianceStatus.EXCELLENT
        assert score.traffic_light == TrafficLight.GREEN

    def test_default_vlsfo_no_fuel_mix(self):
        """When no fuel_mix is provided, defaults to 100% VLSFO."""
        score = calculate_compliance_score(
            vessel_id="test-002",
            vessel_name="Default Vessel",
        )
        # VLSFO is slightly non-compliant with FuelEU -> score around 40
        assert score.fueleu.ghg_intensity_gco2_mj == Decimal("91.16")
        # FuelEU score should be 40 (slightly over target)
        assert score.fueleu.score == 40

    def test_traffic_light_green_excellent(self):
        """Score >= 90 -> EXCELLENT, GREEN."""
        score = calculate_compliance_score(
            vessel_id="t",
            vessel_name="t",
            fuel_mix={"E-Methanol": Decimal("1.0")},
            cii_rating="A",
            total_co2_tonnes=Decimal("10000"),
        )
        assert score.traffic_light == TrafficLight.GREEN
        assert score.status == ComplianceStatus.EXCELLENT

    def test_traffic_light_green_good(self):
        """Score 70-89 -> GOOD, GREEN."""
        score = calculate_compliance_score(
            vessel_id="t",
            vessel_name="t",
            fuel_mix={"LNG": Decimal("1.0")},  # FuelEU score = 100
            cii_rating="C",  # CII score = 60
            total_co2_tonnes=Decimal("35000"),  # ETS score = 60 (0.7 ratio)
        )
        # 100*0.5 + 60*0.3 + 60*0.2 = 50 + 18 + 12 = 80
        assert score.traffic_light == TrafficLight.GREEN
        assert score.status == ComplianceStatus.GOOD
        assert 70 <= score.overall_score < 90

    def test_traffic_light_amber_adequate(self):
        """Score 50-69 -> ADEQUATE, AMBER."""
        score = calculate_compliance_score(
            vessel_id="t",
            vessel_name="t",
            fuel_mix={"LNG": Decimal("1.0")},  # FuelEU score ~80-90
            cii_rating="D",  # CII score = 30
            total_co2_tonnes=Decimal("40000"),  # ETS score ~40-60
        )
        assert score.traffic_light == TrafficLight.AMBER
        assert score.status in (ComplianceStatus.ADEQUATE, ComplianceStatus.AT_RISK)

    def test_traffic_light_red_critical(self):
        """Very poor vessel should be RED/CRITICAL."""
        score = calculate_compliance_score(
            vessel_id="t",
            vessel_name="t",
            fuel_mix={"VLSFO": Decimal("1.0")},
            cii_rating="E",  # CII score = 10
            total_co2_tonnes=Decimal("60000"),  # ETS score = 20
        )
        # FuelEU=40*0.5 + ETS=20*0.3 + CII=10*0.2 = 20+6+2 = 28
        assert score.overall_score == 28
        assert score.traffic_light == TrafficLight.RED
        assert score.status == ComplianceStatus.CRITICAL

    def test_recommendations_generated_for_poor_scores(self):
        """Poor compliance should generate actionable recommendations."""
        score = calculate_compliance_score(
            vessel_id="t",
            vessel_name="t",
            fuel_mix={"VLSFO": Decimal("1.0")},
            cii_rating="E",
            total_co2_tonnes=Decimal("60000"),
        )
        assert len(score.recommendations) > 0
        # Should have FuelEU recommendation (score < 70)
        assert any("FuelEU" in r or "fuel" in r.lower() for r in score.recommendations)
        # Should have CII recommendation (score < 60)
        assert any("CII" in r for r in score.recommendations)
        # Should have penalty warning (VLSFO is over target)
        assert any("penalty" in r.lower() for r in score.recommendations)

    def test_no_recommendations_for_excellent_vessel(self):
        """Excellent compliance should have zero recommendations."""
        score = calculate_compliance_score(
            vessel_id="t",
            vessel_name="t",
            fuel_mix={"E-Methanol": Decimal("1.0")},
            cii_rating="A",
            total_co2_tonnes=Decimal("10000"),
        )
        assert len(score.recommendations) == 0

    def test_vessel_type_affects_default_co2(self):
        """Different vessel types should get different default CO2 estimates."""
        container = calculate_compliance_score(
            vessel_id="c",
            vessel_name="Container Ship",
            vessel_type="container",
        )
        bulk = calculate_compliance_score(
            vessel_id="b",
            vessel_name="Bulk Carrier",
            vessel_type="bulk_carrier",
        )
        # Container (500M MJ) > Bulk (300M MJ) -> more CO2
        assert container.eu_ets.total_co2_tonnes > bulk.eu_ets.total_co2_tonnes

    def test_unknown_vessel_type_uses_default(self):
        """Unknown vessel type should use the default energy assumption."""
        score = calculate_compliance_score(
            vessel_id="u",
            vessel_name="Unknown Type",
            vessel_type="submarine",
        )
        # Should not crash, should use default
        assert score.overall_score >= 0
        assert score.overall_score <= 100

    def test_scenario_different_years(self):
        """Same fuel mix should score differently across years as targets tighten."""
        mix = {"LNG": Decimal("0.7"), "VLSFO": Decimal("0.3")}
        score_2026 = calculate_compliance_score(
            vessel_id="s",
            vessel_name="s",
            fuel_mix=mix,
            year=2026,
        )
        score_2030 = calculate_compliance_score(
            vessel_id="s",
            vessel_name="s",
            fuel_mix=mix,
            year=2030,
        )
        # Stricter 2030 target should result in lower FuelEU score or same
        assert score_2026.fueleu.score >= score_2030.fueleu.score

    def test_vessel_id_and_name_preserved(self):
        """Output should echo the vessel_id and vessel_name inputs."""
        score = calculate_compliance_score(
            vessel_id="IMO-1234567",
            vessel_name="MV Compliance Queen",
        )
        assert score.vessel_id == "IMO-1234567"
        assert score.vessel_name == "MV Compliance Queen"

    def test_empty_fuel_mix_defaults_to_vlsfo(self):
        """Passing an empty dict should default to 100% VLSFO."""
        score = calculate_compliance_score(
            vessel_id="e",
            vessel_name="Empty Mix",
            fuel_mix={},
        )
        assert score.fueleu.ghg_intensity_gco2_mj == Decimal("91.16")

    def test_all_fuel_types_recognized(self):
        """All fuels in FUEL_GHG_INTENSITIES should be individually testable."""
        for fuel_name, expected_intensity in FUEL_GHG_INTENSITIES.items():
            result = calculate_fueleu_score(
                fuel_mix={fuel_name: Decimal("1.0")},
                year=2026,
            )
            assert result.ghg_intensity_gco2_mj == expected_intensity.quantize(Decimal("0.01")), (
                f"Fuel {fuel_name}: expected {expected_intensity}, got {result.ghg_intensity_gco2_mj}"
            )

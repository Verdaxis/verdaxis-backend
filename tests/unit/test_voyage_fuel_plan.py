"""Pure voyage calculations, independent of market I/O and the distance table."""
from datetime import date, timedelta
from decimal import Decimal as D

import pytest

from app.schemas.voyage import (
    IntensitySource, MarketAssumptions, PlannerFuel, PriceLookup, PricePoint,
    PriceSource, SeaRoute, StrategyCompleteness, VesselInput, VoyageLegInput,
    VoyagePlanRequest,
)
from app.services.sea_distances import sea_distance
from app.services.voyage_fuel_plan import (
    VoyagePlanError, build_voyage_plan, required_price_lookups,
)

TODAY = date(2026, 10, 14)


def golden_request(**assumption_overrides):
    assumptions = dict(
        vlsfo_price_usd_per_mt={"Shanghai": D("650"), "Singapore": D("907")},
        eua_price_eur_per_tco2=D("80"),
        fueleu_pool_price_eur_per_tco2e=D("109"),
        usd_per_eur=D("1.10"),
    )
    assumptions.update(assumption_overrides)
    return VoyagePlanRequest(
        vessel=VesselInput(
            speed_knots=D("14"), sea_consumption_mt_per_day=D("40"),
            berth_consumption_mt_per_day=D("5"),
            compatible_fuels=("VLSFO", "UCOME_B100"),
        ),
        legs=(
            VoyageLegInput(from_port="Shanghai", to_port="Singapore", distance_nm=D("2200"), berth_days_at_destination=D("1")),
            VoyageLegInput(from_port="Singapore", to_port="Rotterdam", distance_nm=D("8300"), berth_days_at_destination=D("2")),
        ),
        start_date=date(2026, 11, 1),
        assumptions=MarketAssumptions(**assumptions),
    )


def golden_prices(request, **overrides):
    (lookup,) = required_price_lookups(request)
    values = dict(
        fuel=lookup.fuel, port="Singapore", price_usd_per_mt=D("1445"),
        source=PriceSource.LIVE_ORDER, executable=True, availability_window="2026-11",
    )
    values.update(overrides)
    return {lookup: PricePoint(**values)}


def strategy(result, strategy_id):
    return next(s for s in result.strategies if s.strategy_id == strategy_id)


def close(actual, expected, tolerance="0.05"):
    return abs(actual - D(expected)) <= D(tolerance)


def plan(request=None, **price_overrides):
    request = request or golden_request()
    return build_voyage_plan(request, golden_prices(request, **price_overrides), today=TODAY)


def with_fuels(request, *fuels):
    return request.model_copy(update={
        "vessel": request.vessel.model_copy(update={"compatible_fuels": tuple(PlannerFuel(f) for f in fuels)})
    })


def test_required_price_lookups_ask_for_ucome_at_singapore_only():
    assert required_price_lookups(golden_request()) == (
        PriceLookup(fuel=PlannerFuel.UCOME_B100, port="Singapore", departure_date=date(2026, 11, 9)),
    )


def test_legs_have_scope_and_energy():
    first, second = plan().legs
    assert (first.eu_scope_share, first.berth_eu_scope_share) == (D("0"), D("0"))
    assert (second.eu_scope_share, second.berth_eu_scope_share) == (D("0.5"), D("1"))
    assert (first.sea_energy_mj, second.sea_energy_mj) == (D("10738095"), D("40511905"))
    assert (first.berth_energy_mj, second.berth_energy_mj) == (D("205000"), D("410000"))
    assert (first.sea_days, second.sea_days) == (D("6.5476"), D("24.7024"))
    assert first.arrival_date == date(2026, 11, 8)
    assert second.departure_date == date(2026, 11, 9)
    assert second.arrival_date == date(2026, 12, 4)
    assert first.distance_source == "CALLER"


@pytest.mark.parametrize(("strategy_id", "expected"), [
    ("CONVENTIONAL_PENALTY", ("1078760.48", "127060.32", "-37.68", "24194.29", "1245140.55")),
    ("CONVENTIONAL_POOL", ("1078760.48", "127060.32", "-37.68", "4106.92", "1223044.44")),
    ("UCOME_B100_MIN_BLEND", ("1100975.91", "122935.39", "0.00", "0.00", "1236204.84")),
    ("UCOME_B100_FULL", ("1763062.08", "0.00", "1122.92", "-122398.47", "1628423.77")),
])
def test_golden_strategies(strategy_id, expected):
    result = strategy(plan(), strategy_id)
    fields = ("fuel_cost_usd", "eu_ets_cost_eur", "fueleu_balance_tco2e", "fueleu_cost_eur", "total_cost_usd")
    for field, value in zip(fields, expected):
        assert close(getattr(result, field), value), field
    assert result.completeness == StrategyCompleteness.COMPLETE
    assert result.missing_inputs == ()
    assert result.in_scope_energy_mj == D("20665952")
    assert result.fueleu_target_gco2e_per_mj == D("89.3368")


def test_golden_fuel_tonnes_and_bunkering():
    result = plan()
    blend = strategy(result, "UCOME_B100_MIN_BLEND")
    full = strategy(result, "UCOME_B100_FULL")
    assert close(sum(u.tonnes for u in blend.fuel_uses if u.fuel == PlannerFuel.UCOME_B100), "35.71")
    assert close(sum(u.tonnes for u in blend.fuel_uses if u.fuel == PlannerFuel.VLSFO), "1232.60")
    assert close(sum(u.tonnes for u in full.fuel_uses if u.fuel == PlannerFuel.UCOME_B100), "1100.05")
    for use in full.fuel_uses:
        assert use.bunkering_port == result.legs[use.leg_index].from_port
        if use.fuel == PlannerFuel.VLSFO:
            assert use.price.source == PriceSource.CALLER
            assert use.price.executable is False


def test_recommends_cheapest_complete_strategy():
    result = plan()
    assert result.recommended_strategy_id == "CONVENTIONAL_POOL"
    assert "CONVENTIONAL_POOL" in result.recommendation_reason
    assert str(strategy(result, "CONVENTIONAL_POOL").total_cost_usd) in result.recommendation_reason


def test_missing_assumptions_make_strategies_partial():
    result = plan(golden_request(fueleu_pool_price_eur_per_tco2e=None, usd_per_eur=None))
    assert all(s.completeness == StrategyCompleteness.PARTIAL for s in result.strategies)
    assert all(s.total_cost_usd is None for s in result.strategies)
    assert result.recommended_strategy_id is None
    assert "usd_per_eur" in result.recommendation_reason
    assert "fueleu_pool_price_eur_per_tco2e" in result.recommendation_reason


def test_missing_vlsfo_price_names_the_port():
    result = plan(golden_request(vlsfo_price_usd_per_mt={"Shanghai": D("650")}))
    assert "vlsfo_price_usd_per_mt[Singapore]" in strategy(result, "CONVENTIONAL_POOL").missing_inputs
    assert strategy(result, "CONVENTIONAL_POOL").fuel_cost_usd is None
    assert strategy(result, "UCOME_B100_FULL").completeness == StrategyCompleteness.COMPLETE


def test_voyage_without_eu_scope_has_no_green_strategy():
    request = golden_request(eua_price_eur_per_tco2=None, fueleu_pool_price_eur_per_tco2e=None, usd_per_eur=None)
    request = request.model_copy(update={"legs": request.legs[:1]})
    assert required_price_lookups(request) == ()
    result = build_voyage_plan(request, {}, today=TODAY)
    assert [s.strategy_id for s in result.strategies] == ["CONVENTIONAL_PENALTY", "CONVENTIONAL_POOL"]
    for s in result.strategies:
        assert s.eu_ets_cost_eur == s.fueleu_cost_eur == D("0")
        assert s.fueleu_cost_basis == "NONE"
        assert s.completeness == StrategyCompleteness.COMPLETE
    assert result.recommended_strategy_id == "CONVENTIONAL_PENALTY"


def test_fuel_without_default_intensity_is_partial():
    request = with_fuels(golden_request(), "VLSFO", "BIO_ETHANOL")
    result = plan(request)
    for name in ("BIO_ETHANOL_MIN_BLEND", "BIO_ETHANOL_FULL"):
        s = strategy(result, name)
        assert s.completeness == StrategyCompleteness.PARTIAL
        assert "ghg_intensity_gco2e_per_mj[BIO_ETHANOL]" in s.missing_inputs
        assert s.fueleu_cost_eur is None
        assert s.total_cost_usd is None


@pytest.mark.parametrize("intensity", [D("95"), D("89.3368")])
def test_green_fuel_at_or_above_target_skips_min_blend(intensity):
    result = plan(golden_request(ghg_intensity_gco2e_per_mj={PlannerFuel.UCOME_B100: intensity}))
    assert "UCOME_B100_MIN_BLEND" not in [s.strategy_id for s in result.strategies]
    assert any("MIN_BLEND" in warning for warning in result.warnings)


def test_demo_price_adds_warning():
    result = plan(source=PriceSource.DEMO_ORDER, executable=False)
    assert any("demo" in warning.lower() for warning in result.warnings)
    assert all(not u.price.executable for s in result.strategies for u in s.fuel_uses if u.price and u.price.source == PriceSource.DEMO_ORDER)


def test_vessel_without_vlsfo_has_no_conventional_strategy():
    result = plan(with_fuels(golden_request(), "UCOME_B100"))
    assert [s.strategy_id for s in result.strategies] == ["UCOME_B100_FULL"]


def test_order_declared_intensity_is_used():
    baseline = strategy(plan(), "UCOME_B100_MIN_BLEND")
    result = strategy(plan(carbon_intensity_gco2e_per_mj=D("14")), "UCOME_B100_MIN_BLEND")
    assert result.intensity_sources[PlannerFuel.UCOME_B100] == IntensitySource.ORDER_DECLARED
    assert sum(u.tonnes for u in result.fuel_uses if u.fuel == PlannerFuel.UCOME_B100) < sum(u.tonnes for u in baseline.fuel_uses if u.fuel == PlannerFuel.UCOME_B100)


def test_start_date_too_far_ahead_is_rejected():
    request = golden_request().model_copy(update={"start_date": date(2029, 1, 1)})
    with pytest.raises(VoyagePlanError):
        build_voyage_plan(request, {}, today=TODAY)


def test_table_distance_used_when_caller_omits_distance():
    if sea_distance("Shanghai", "Singapore") is None:
        pytest.skip("Task 1 distance table is not yet populated")
    request = golden_request()
    first = request.legs[0].model_copy(update={"distance_nm": None})
    request = request.model_copy(update={"legs": (first,)})
    result = build_voyage_plan(request, {}, today=TODAY)
    assert result.legs[0].distance_source == "VERDAXIS_TABLE"


@pytest.mark.parametrize(("offset", "valid"), [(-31, False), (-30, True), (730, True), (731, False)])
def test_start_date_bounds(offset, valid):
    request = golden_request().model_copy(update={"start_date": TODAY + timedelta(days=offset)})
    if valid:
        build_voyage_plan(request, {}, today=TODAY)
    else:
        with pytest.raises(VoyagePlanError):
            build_voyage_plan(request, {}, today=TODAY)


def test_missing_table_distance_is_rejected(monkeypatch):
    monkeypatch.setattr("app.services.voyage_fuel_plan.sea_distance", lambda *args: None)
    request = golden_request()
    request = request.model_copy(update={"legs": (request.legs[0].model_copy(update={"distance_nm": None}),)})
    with pytest.raises(VoyagePlanError):
        build_voyage_plan(request, {}, today=TODAY)


def test_cape_route_without_table_entry_is_rejected_even_with_caller_distance():
    request = golden_request()
    request = request.model_copy(update={"legs": (request.legs[0].model_copy(update={"route": SeaRoute.CAPE_OF_GOOD_HOPE}),)})
    with pytest.raises(VoyagePlanError):
        build_voyage_plan(request, {}, today=TODAY)


def test_missing_market_price_names_fuel_and_port():
    result = build_voyage_plan(golden_request(), {}, today=TODAY)
    for name in ("UCOME_B100_MIN_BLEND", "UCOME_B100_FULL"):
        s = strategy(result, name)
        assert "market_price[UCOME_B100@Singapore]" in s.missing_inputs
        assert s.fuel_cost_usd is None


def test_missing_eua_price_only_affects_fossil_scope():
    result = plan(golden_request(eua_price_eur_per_tco2=None))
    assert "eua_price_eur_per_tco2" in strategy(result, "CONVENTIONAL_POOL").missing_inputs
    assert strategy(result, "UCOME_B100_FULL").eu_ets_cost_eur == D("0")


def test_unvalued_surplus_is_not_a_missing_input():
    result = strategy(plan(golden_request(fueleu_pool_price_eur_per_tco2e=None)), "UCOME_B100_FULL")
    assert "fueleu_pool_price_eur_per_tco2e" not in result.missing_inputs
    assert result.fueleu_cost_eur is None
    assert result.completeness == StrategyCompleteness.PARTIAL


def test_caller_intensity_overrides_order():
    request = golden_request(ghg_intensity_gco2e_per_mj={PlannerFuel.UCOME_B100: D("20")})
    result = strategy(plan(request, carbon_intensity_gco2e_per_mj=D("14")), "UCOME_B100_FULL")
    assert result.ghg_intensity_gco2e_per_mj == D("20.0000")
    assert result.intensity_sources[PlannerFuel.UCOME_B100] == IntensitySource.CALLER


def test_order_energy_density_changes_tonnes_not_energy():
    result = strategy(plan(energy_density_mj_per_kg=D("40")), "UCOME_B100_FULL")
    assert result.in_scope_energy_mj == D("20665952")
    assert close(sum(u.tonnes for u in result.fuel_uses if u.fuel == PlannerFuel.UCOME_B100), "1023.05")


def test_compliant_vlsfo_skips_identical_blend():
    result = plan(golden_request(ghg_intensity_gco2e_per_mj={PlannerFuel.VLSFO: D("80")}))
    assert "UCOME_B100_MIN_BLEND" not in [s.strategy_id for s in result.strategies]


def test_benchmark_and_sustainability_warnings_and_assumptions():
    result = plan(source=PriceSource.BENCHMARK, executable=False)
    warnings = " ".join(result.warnings).lower()
    assert "benchmark" in warnings
    assert "proof of sustainability" in warnings
    assumptions = " ".join(result.assumptions).lower()
    for text in ("departure", "41,000", "annual", "voyage", "outermost", "transshipment", "methane", "n2o"):
        assert text in assumptions


def test_reporting_year_override_controls_target_and_ets_phase_in():
    result = plan(golden_request(reporting_year=2025))
    assert result.reporting_year == 2025
    assert strategy(result, "CONVENTIONAL_POOL").eu_ets_cost_eur == D("88942.23")


def test_minimum_blend_has_exact_zero_balance_without_pool_price():
    request = golden_request(fueleu_pool_price_eur_per_tco2e=None)
    request = request.model_copy(update={"legs": (
        VoyageLegInput(from_port="Singapore", to_port="Rotterdam", distance_nm=D("1")),
    )})
    result = strategy(plan(request, carbon_intensity_gco2e_per_mj=D("14")), "UCOME_B100_MIN_BLEND")
    assert result.fueleu_cost_basis == "NONE"
    assert result.fueleu_cost_eur == D("0.00")
    assert result.completeness == StrategyCompleteness.COMPLETE


def test_nested_price_output_is_rounded_without_rounding_calculations():
    result = strategy(plan(price_usd_per_mt=D("1445.12345"), carbon_intensity_gco2e_per_mj=D("14.123456")), "UCOME_B100_FULL")
    uses = [u for u in result.fuel_uses if u.fuel == PlannerFuel.UCOME_B100]
    assert all(u.price.price_usd_per_mt == D("1445.12") for u in uses)
    assert all(u.price.carbon_intensity_gco2e_per_mj == D("14.1235") for u in uses)
    sea_energy = D("8300") / (D("14") * D("24")) * D("40") * D("41000")
    from decimal import ROUND_HALF_UP
    expected = (sea_energy / D("37200") * D("1445.12345")).quantize(D("0.01"), rounding=ROUND_HALF_UP)
    assert next(u.cost_usd for u in uses if u.segment == "SEA") == expected


def test_lookup_order_and_port_availability():
    request = with_fuels(golden_request(), "BIO_METHANOL", "UCOME_B100", "E_METHANOL")
    assert [lookup.fuel for lookup in required_price_lookups(request)] == [PlannerFuel.BIO_METHANOL, PlannerFuel.UCOME_B100, PlannerFuel.E_METHANOL]
    request = request.model_copy(update={"legs": (VoyageLegInput(from_port="Rotterdam", to_port="Singapore", distance_nm=D("8300")),)})
    assert [lookup.fuel for lookup in required_price_lookups(request)] == [PlannerFuel.BIO_METHANOL, PlannerFuel.E_METHANOL]

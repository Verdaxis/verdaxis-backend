"""Deterministic voyage contributions to fuel, ETS and annual FuelEU costs."""
from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from datetime import date, timedelta
from decimal import Decimal, ROUND_CEILING, ROUND_HALF_UP
from types import MappingProxyType
from typing import Literal

from app.market_catalog import DELIVERY_POINT_IDS, PRODUCTS_BY_CODE
from app.schemas.voyage import (
    FuelUse, IntensitySource, LegPlan, PlannerFuel, PriceLookup, PricePoint,
    PriceSource, SeaRoute, StrategyCompleteness, StrategyResult, VoyagePlanRequest,
    VoyagePlanResult,
)
from app.services.compliance_scoring import FUELEU_PENALTY_PER_TONNE, calculate_ets_score
from app.services.fueleu import fueleu_year_target
from app.services.sea_distances import sea_distance

EU_PORTS: frozenset[str] = frozenset({"Rotterdam"})
ZERO = Decimal("0")
ONE = Decimal("1")
# Consumption inputs use VLSFO-equivalent tonnes: 41 MJ/kg × 1,000 kg/t.
VLSFO_ENERGY_MJ_PER_TONNE = Decimal("41000")
KG_PER_TONNE = Decimal("1000")
GRAMS_PER_TONNE = Decimal("1000000")


@dataclass(frozen=True)
class FuelProperties:
    lcv_mj_per_kg: Decimal
    ets_co2_t_per_t: Decimal
    default_intensity_gco2e_per_mj: Decimal | None


FUEL_PROPERTIES: Mapping[PlannerFuel, FuelProperties] = MappingProxyType({
    # 3.151 tonnes fossil CO2 per tonne of VLSFO is the planning ETS factor.
    PlannerFuel.VLSFO: FuelProperties(Decimal("41.0"), Decimal("3.151"), Decimal("91.16")),
    PlannerFuel.UCOME_B100: FuelProperties(Decimal("37.2"), ZERO, Decimal("35.00")),
    PlannerFuel.BIO_METHANOL: FuelProperties(Decimal("19.9"), ZERO, Decimal("31.00")),
    PlannerFuel.E_METHANOL: FuelProperties(Decimal("19.9"), ZERO, Decimal("8.00")),
    PlannerFuel.BIO_ETHANOL: FuelProperties(Decimal("26.8"), ZERO, None),
    PlannerFuel.SYNTHETIC_ETHANOL: FuelProperties(Decimal("26.8"), ZERO, None),
})


class VoyagePlanError(ValueError):
    """The request cannot be planned from the supplied route or date."""


@dataclass(frozen=True)
class _Segment:
    leg_index: int
    kind: Literal["SEA", "BERTH"]
    port: str
    departure: date
    energy: Decimal
    scope: Decimal


@dataclass(frozen=True)
class _Burn:
    segment: _Segment
    fuel: PlannerFuel
    energy: Decimal
    intensity: Decimal | None
    source: IntensitySource | None
    price: PricePoint | None
    lcv: Decimal


def _rounded(value: Decimal | None, precision: str = "0.01") -> Decimal | None:
    return None if value is None else value.quantize(Decimal(precision), rounding=ROUND_HALF_UP)


def _ceil_days(days: Decimal) -> timedelta:
    return timedelta(days=int(days.to_integral_value(rounding=ROUND_CEILING)))


def _legs(request: VoyagePlanRequest) -> tuple[LegPlan, ...]:
    """Keep exact energies and days internally; round only the returned output."""
    departure = request.start_date
    legs = []
    for leg in request.legs:
        table = sea_distance(leg.from_port, leg.to_port, leg.route)
        if leg.route == SeaRoute.CAPE_OF_GOOD_HOPE and table is None:
            raise VoyagePlanError(f"No Cape route for {leg.from_port} to {leg.to_port}")
        if leg.distance_nm is None and table is None:
            raise VoyagePlanError(f"No distance for {leg.from_port} to {leg.to_port}")
        distance = leg.distance_nm if leg.distance_nm is not None else table.distance_nm
        sea_days = distance / (request.vessel.speed_knots * Decimal("24"))
        arrival = departure + _ceil_days(sea_days)
        scope = Decimal(int(leg.from_port in EU_PORTS) + int(leg.to_port in EU_PORTS)) / Decimal("2")
        legs.append(LegPlan(
            from_port=leg.from_port, to_port=leg.to_port, route=leg.route,
            via=table.via if table else "Caller distance",
            distance_nm=distance,
            distance_source="CALLER" if leg.distance_nm is not None else "VERDAXIS_TABLE",
            departure_date=departure, arrival_date=arrival, sea_days=sea_days,
            eu_scope_share=scope,
            sea_energy_mj=sea_days * request.vessel.sea_consumption_mt_per_day * VLSFO_ENERGY_MJ_PER_TONNE,
            berth_days=leg.berth_days_at_destination,
            berth_eu_scope_share=ONE if leg.to_port in EU_PORTS else ZERO,
            berth_energy_mj=leg.berth_days_at_destination * request.vessel.berth_consumption_mt_per_day * VLSFO_ENERGY_MJ_PER_TONNE,
        ))
        departure = arrival + _ceil_days(leg.berth_days_at_destination)
    return tuple(legs)


def _segments(legs: tuple[LegPlan, ...]) -> tuple[_Segment, ...]:
    return tuple(
        _Segment(index, kind, leg.from_port, leg.departure_date, energy, scope)
        for index, leg in enumerate(legs)
        for kind, energy, scope in (
            ("SEA", leg.sea_energy_mj, leg.eu_scope_share),
            ("BERTH", leg.berth_energy_mj, leg.berth_eu_scope_share),
        )
    )


def _available(fuel: PlannerFuel, port: str) -> bool:
    allowed = PRODUCTS_BY_CODE[fuel.value].available_delivery_point_ids
    return allowed is None or DELIVERY_POINT_IDS[port] in allowed


def required_price_lookups(request: VoyagePlanRequest) -> tuple[PriceLookup, ...]:
    lookups = []
    for leg in _legs(request):
        if not (leg.eu_scope_share or leg.berth_eu_scope_share):
            continue
        for fuel in request.vessel.compatible_fuels:
            if fuel != PlannerFuel.VLSFO and _available(fuel, leg.from_port):
                lookups.append(PriceLookup(fuel=fuel, port=leg.from_port, departure_date=leg.departure_date))
    return tuple(dict.fromkeys(lookups))


def _fuel_details(request, prices, segment, fuel):
    properties = FUEL_PROPERTIES[fuel]
    if fuel == PlannerFuel.VLSFO:
        amount = request.assumptions.vlsfo_price_usd_per_mt.get(segment.port)
        price = None if amount is None else PricePoint(
            fuel=fuel, port=segment.port, price_usd_per_mt=amount,
            source=PriceSource.CALLER, executable=False,
        )
    else:
        lookup = PriceLookup(fuel=fuel, port=segment.port, departure_date=segment.departure)
        price = prices.get(lookup) if _available(fuel, segment.port) else None
    overrides = request.assumptions.ghg_intensity_gco2e_per_mj
    if fuel in overrides:
        intensity, source = overrides[fuel], IntensitySource.CALLER
    elif price is not None and price.carbon_intensity_gco2e_per_mj is not None:
        intensity, source = price.carbon_intensity_gco2e_per_mj, IntensitySource.ORDER_DECLARED
    else:
        intensity = properties.default_intensity_gco2e_per_mj
        source = IntensitySource.DEFAULT_ILLUSTRATIVE if intensity is not None else None
    lcv = properties.lcv_mj_per_kg
    if price is not None and price.energy_density_mj_per_kg is not None:
        lcv = price.energy_density_mj_per_kg
    return intensity, source, price, lcv


def _burns(request, prices, segments, green, minimum, target, missing):
    burns = []
    unknown_blend = False
    for segment in segments:
        if segment.energy == ZERO:
            continue
        green_share = ZERO
        if green is not None and segment.scope > ZERO:
            details = _fuel_details(request, prices, segment, green)
            intensity = details[0]
            green_share = ONE
            if minimum:
                conventional = _fuel_details(request, prices, segment, PlannerFuel.VLSFO)[0]
                if intensity is None:
                    missing.append(f"ghg_intensity_gco2e_per_mj[{green.value}]")
                    unknown_blend = True
                    # No invented blend: leave this segment's fuel and costs unpriced.
                    continue
                green_share = (conventional - target) / (conventional - intensity)
            burns.append(_Burn(segment, green, segment.energy * green_share, *details))
        if green_share < ONE:
            details = _fuel_details(request, prices, segment, PlannerFuel.VLSFO)
            burns.append(_Burn(segment, PlannerFuel.VLSFO, segment.energy * (ONE - green_share), *details))
    return burns, unknown_blend


def _price_fuel_uses(burns, missing, warnings):
    uses = []
    total = ZERO
    unpriced = False
    co2 = ZERO
    for burn in burns:
        tonnes = burn.energy / (burn.lcv * KG_PER_TONNE)
        co2 += burn.segment.scope * tonnes * FUEL_PROPERTIES[burn.fuel].ets_co2_t_per_t
        cost = None
        if burn.price is None:
            name = (f"vlsfo_price_usd_per_mt[{burn.segment.port}]" if burn.fuel == PlannerFuel.VLSFO
                    else f"market_price[{burn.fuel.value}@{burn.segment.port}]")
            missing.append(name)
            unpriced = True
        else:
            cost = tonnes * burn.price.price_usd_per_mt
            total += cost
            if burn.price.source == PriceSource.DEMO_ORDER:
                warnings.append("Used price is demo preview liquidity, not executable liquidity.")
            elif burn.price.source == PriceSource.BENCHMARK:
                warnings.append("Used benchmark price is an indicative reference, not an executable quote.")
        uses.append(FuelUse(
            leg_index=burn.segment.leg_index, segment=burn.segment.kind, fuel=burn.fuel,
            tonnes=_rounded(tonnes), bunkering_port=burn.segment.port,
            price=_output_price(burn.price), cost_usd=_rounded(cost),
        ))
    return tuple(uses), None if unpriced else total, co2


def _fueleu_cost(balance, intensity, basis, pool_price, missing):
    if balance is None:
        return None, "NONE"
    if balance == ZERO:
        return ZERO, "NONE"
    if balance > ZERO:
        return (-balance * pool_price if pool_price is not None else None), "SURPLUS_AT_POOL_PRICE"
    penalty = -balance * GRAMS_PER_TONNE / (intensity * VLSFO_ENERGY_MJ_PER_TONNE) * FUELEU_PENALTY_PER_TONNE
    pool = -balance * pool_price if pool_price is not None else None
    if basis == "PENALTY":
        return penalty, "PENALTY"
    if basis == "POOL":
        if pool is None:
            missing.append("fueleu_pool_price_eur_per_tco2e")
        return pool, "POOL"
    if pool is not None and pool < penalty:
        return pool, "POOL"
    return penalty, "PENALTY"


def _strategy(request, prices, segments, strategy_id, target, year, warnings, *, green=None, minimum=False, basis=None):
    missing = []
    burns, unknown_blend = _burns(request, prices, segments, green, minimum, target, missing)
    uses, fuel_cost, co2 = _price_fuel_uses(burns, missing, warnings)
    energy = sum((segment.scope * segment.energy for segment in segments), ZERO)
    sources = {}
    weighted = ZERO
    unknown_intensity = unknown_blend
    for burn in burns:
        if burn.source is not None:
            sources[burn.fuel] = burn.source
        if burn.segment.scope > ZERO:
            if burn.intensity is None:
                unknown_intensity = True
                missing.append(f"ghg_intensity_gco2e_per_mj[{burn.fuel.value}]")
            else:
                weighted += burn.segment.scope * burn.energy * burn.intensity
    intensity = None if unknown_intensity or energy == ZERO else weighted / energy
    if minimum and not unknown_intensity and energy > ZERO:
        # Each exact minimum blend meets target by construction. Avoid treating
        # finite-context division residue as a real surplus or deficit.
        intensity = target
    balance = None if unknown_intensity else ((target - intensity) * energy / GRAMS_PER_TONNE if energy else ZERO)
    fueleu_cost, cost_basis = _fueleu_cost(balance, intensity, basis, request.assumptions.fueleu_pool_price_eur_per_tco2e, missing)
    eua = request.assumptions.eua_price_eur_per_tco2
    if unknown_blend:
        fuel_cost = None
        ets_cost = None
    elif co2 == ZERO:
        ets_cost = ZERO
    elif eua is None:
        ets_cost = None
        missing.append("eua_price_eur_per_tco2")
    else:
        ets_cost = calculate_ets_score(co2, eua, year).estimated_cost_eur
    rate = request.assumptions.usd_per_eur
    needs_rate = any(cost is not None and cost != ZERO for cost in (ets_cost, fueleu_cost))
    if needs_rate and rate is None:
        missing.append("usd_per_eur")
    total = None
    if fuel_cost is not None and ets_cost is not None and fueleu_cost is not None and (not needs_rate or rate is not None):
        total = fuel_cost + ((ets_cost + fueleu_cost) * rate if needs_rate else ZERO)
    return StrategyResult(
        strategy_id=strategy_id, label=strategy_id.replace("_", " ").title(),
        completeness=StrategyCompleteness.COMPLETE if total is not None else StrategyCompleteness.PARTIAL,
        fuel_uses=uses, in_scope_energy_mj=_rounded(energy, "1"),
        ghg_intensity_gco2e_per_mj=_rounded(intensity, "0.0001"),
        fueleu_target_gco2e_per_mj=_rounded(target, "0.0001"),
        fueleu_balance_tco2e=_rounded(balance), intensity_sources=sources,
        fuel_cost_usd=_rounded(fuel_cost), eu_ets_cost_eur=_rounded(ets_cost),
        fueleu_cost_eur=_rounded(fueleu_cost), fueleu_cost_basis=cost_basis,
        total_cost_usd=_rounded(total), missing_inputs=tuple(dict.fromkeys(missing)),
    )


def _recommend(strategies):
    complete = [s for s in strategies if s.completeness == StrategyCompleteness.COMPLETE]
    if complete:
        best = min(complete, key=lambda s: s.total_cost_usd)
        return best.strategy_id, f"{best.strategy_id} has the lowest complete total: USD {best.total_cost_usd}."
    missing = tuple(dict.fromkeys(name for s in strategies for name in s.missing_inputs))
    reason = "No complete strategy."
    if missing:
        reason += " Supply: " + ", ".join(missing) + "."
    if any(s.fueleu_cost_basis == "SURPLUS_AT_POOL_PRICE" and s.fueleu_cost_eur is None for s in strategies):
        reason += " FuelEU surplus is unvalued without a pool price."
    return None, reason


def _output_price(price: PricePoint | None) -> PricePoint | None:
    if price is None:
        return None
    return price.model_copy(update={
        "price_usd_per_mt": _rounded(price.price_usd_per_mt),
        "carbon_intensity_gco2e_per_mj": _rounded(price.carbon_intensity_gco2e_per_mj, "0.0001"),
    })


def _output_leg(leg: LegPlan) -> LegPlan:
    return leg.model_copy(update={
        "sea_days": _rounded(leg.sea_days, "0.0001"),
        "berth_days": _rounded(leg.berth_days, "0.0001"),
        "eu_scope_share": _rounded(leg.eu_scope_share, "0.0001"),
        "berth_eu_scope_share": _rounded(leg.berth_eu_scope_share, "0.0001"),
        "sea_energy_mj": _rounded(leg.sea_energy_mj, "1"),
        "berth_energy_mj": _rounded(leg.berth_energy_mj, "1"),
    })


def build_voyage_plan(
    request: VoyagePlanRequest, prices: Mapping[PriceLookup, PricePoint], *, today: date,
) -> VoyagePlanResult:
    # Permit a 30-day retrospective window and a 730-day planning horizon.
    if not -30 <= (request.start_date - today).days <= 730:
        raise VoyagePlanError("start_date must be within 30 days before and 730 days after today")
    legs = _legs(request)
    segments = _segments(legs)
    year = request.assumptions.reporting_year or request.start_date.year
    target = fueleu_year_target(year)
    warnings = []
    strategies = []
    has_vlsfo = PlannerFuel.VLSFO in request.vessel.compatible_fuels
    if has_vlsfo:
        for basis in ("PENALTY", "POOL"):
            strategies.append(_strategy(request, prices, segments, f"CONVENTIONAL_{basis}", target, year, warnings, basis=basis))
    in_scope = [segment for segment in segments if segment.scope > ZERO]
    for fuel in request.vessel.compatible_fuels:
        if fuel == PlannerFuel.VLSFO or not in_scope:
            continue
        warnings.append("Green fuels need a proof of sustainability for zero ETS CO2 and FuelEU credit.")
        minimum = has_vlsfo
        for segment in in_scope:
            conventional = _fuel_details(request, prices, segment, PlannerFuel.VLSFO)[0]
            green = _fuel_details(request, prices, segment, fuel)[0]
            if conventional <= target or (green is not None and green >= target):
                minimum = False
                warnings.append(f"{fuel.value}_MIN_BLEND skipped: VLSFO is already compliant or green intensity is at or above the target.")
                break
        if minimum:
            strategies.append(_strategy(request, prices, segments, f"{fuel.value}_MIN_BLEND", target, year, warnings, green=fuel, minimum=True))
        strategies.append(_strategy(request, prices, segments, f"{fuel.value}_FULL", target, year, warnings, green=fuel))
    recommendation, reason = _recommend(strategies)
    return VoyagePlanResult(
        reporting_year=year, legs=tuple(_output_leg(leg) for leg in legs),
        strategies=tuple(strategies), recommended_strategy_id=recommendation,
        recommendation_reason=reason,
        assumptions=(
            "Fuel for a leg and its destination berth stay is bought at the departure port on the departure date.",
            "Consumption uses a 41,000 MJ/t VLSFO-equivalent energy basis.",
            "FuelEU is an annual per-ship balance; this is the voyage contribution, not the annual penalty after banking, borrowing and pooling.",
            "EU scope ignores outermost-region and transshipment rules.",
            "ETS methane and N2O are not included.",
        ),
        warnings=tuple(dict.fromkeys(warnings)),
    )

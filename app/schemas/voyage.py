"""Typed contract for the voyage fuel planner and the read-only MCP tools.

Every planner result is an indicative planning estimate. It is not a quote,
an offer, or advice to trade. Design: docs/plans/2026-10-14-voyage-mcp-design.md.
"""
from __future__ import annotations

from datetime import date, datetime
from decimal import Decimal
from enum import Enum
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator

from app.market_catalog import DELIVERY_POINTS_BY_NAME
from app.schemas.market_activity import MarketDemoStatus

MAX_VOYAGE_LEGS = 10
SUPPORTED_PORTS: tuple[str, ...] = tuple(DELIVERY_POINTS_BY_NAME)


class ContractModel(BaseModel):
    """Frozen and strict, so that requests are hashable and unknown fields fail."""

    model_config = ConfigDict(extra="forbid", frozen=True)


class PlannerFuel(str, Enum):
    """VLSFO plus the catalog market products, using the catalog codes."""

    VLSFO = "VLSFO"
    UCOME_B100 = "UCOME_B100"
    BIO_METHANOL = "BIO_METHANOL"
    E_METHANOL = "E_METHANOL"
    BIO_ETHANOL = "BIO_ETHANOL"
    SYNTHETIC_ETHANOL = "SYNTHETIC_ETHANOL"


class SeaRoute(str, Enum):
    DEFAULT = "DEFAULT"
    # Only for pairs whose default route transits the Suez Canal.
    CAPE_OF_GOOD_HOPE = "CAPE_OF_GOOD_HOPE"


class PriceSource(str, Enum):
    LIVE_ORDER = "LIVE_ORDER"  # open public ask from a real organization
    DEMO_ORDER = "DEMO_ORDER"  # seeded preview liquidity; never executable
    BENCHMARK = "BENCHMARK"  # indicative reference price
    CALLER = "CALLER"  # supplied in the request


class IntensitySource(str, Enum):
    ORDER_DECLARED = "ORDER_DECLARED"
    CALLER = "CALLER"
    DEFAULT_ILLUSTRATIVE = "DEFAULT_ILLUSTRATIVE"


class StrategyCompleteness(str, Enum):
    COMPLETE = "COMPLETE"
    PARTIAL = "PARTIAL"


def canonical_port_name(value: str) -> str:
    """Accept any letter case for a catalog delivery point and return its catalog name."""
    for name in SUPPORTED_PORTS:
        if name.casefold() == value.strip().casefold():
            return name
    raise ValueError(f"Unsupported port {value!r}. Supported ports: {', '.join(SUPPORTED_PORTS)}")


# ---------------------------------------------------------------------------
# Requests
# ---------------------------------------------------------------------------


class VesselInput(ContractModel):
    name: str | None = Field(default=None, max_length=120)
    imo_number: str | None = Field(default=None, pattern=r"^\d{7}$")
    speed_knots: Decimal = Field(ge=1, le=30)
    sea_consumption_mt_per_day: Decimal = Field(
        gt=0, le=400, description="Tonnes per sea day, VLSFO-equivalent energy"
    )
    berth_consumption_mt_per_day: Decimal = Field(
        default=Decimal("0"), ge=0, le=100, description="Tonnes per berth day, VLSFO-equivalent energy"
    )
    compatible_fuels: tuple[PlannerFuel, ...] = Field(min_length=1, max_length=len(PlannerFuel))

    @field_validator("compatible_fuels")
    @classmethod
    def fuels_are_unique(cls, value: tuple[PlannerFuel, ...]) -> tuple[PlannerFuel, ...]:
        if len(set(value)) != len(value):
            raise ValueError("compatible_fuels must not repeat a fuel")
        return value


class VoyageLegInput(ContractModel):
    from_port: str
    to_port: str
    route: SeaRoute = SeaRoute.DEFAULT
    distance_nm: Decimal | None = Field(default=None, gt=0, le=30000)
    berth_days_at_destination: Decimal = Field(default=Decimal("0"), ge=0, le=60)

    @field_validator("from_port", "to_port")
    @classmethod
    def port_is_supported(cls, value: str) -> str:
        return canonical_port_name(value)

    @model_validator(mode="after")
    def ports_differ(self) -> "VoyageLegInput":
        if self.from_port == self.to_port:
            raise ValueError("A leg must connect two different ports")
        return self


class MarketAssumptions(ContractModel):
    """Inputs Verdaxis has no market for. A missing value leaves its cost unpriced."""

    vlsfo_price_usd_per_mt: dict[str, Decimal] = Field(default_factory=dict)
    eua_price_eur_per_tco2: Decimal | None = Field(default=None, gt=0, le=1000)
    fueleu_pool_price_eur_per_tco2e: Decimal | None = Field(default=None, gt=0, le=5000)
    usd_per_eur: Decimal | None = Field(default=None, gt=0, le=10)
    ghg_intensity_gco2e_per_mj: dict[PlannerFuel, Decimal] = Field(default_factory=dict)
    reporting_year: int | None = Field(default=None, ge=2025, le=2050)

    @field_validator("vlsfo_price_usd_per_mt")
    @classmethod
    def vlsfo_prices_are_valid(cls, value: dict[str, Decimal]) -> dict[str, Decimal]:
        prices: dict[str, Decimal] = {}
        for port, price in value.items():
            if not Decimal("0") < price <= Decimal("10000"):
                raise ValueError(f"VLSFO price for {port} must be above 0 and at most 10000 USD/t")
            prices[canonical_port_name(port)] = price
        return prices

    @field_validator("ghg_intensity_gco2e_per_mj")
    @classmethod
    def intensities_are_valid(cls, value: dict[PlannerFuel, Decimal]) -> dict[PlannerFuel, Decimal]:
        for fuel, intensity in value.items():
            if not Decimal("0") <= intensity <= Decimal("200"):
                raise ValueError(f"GHG intensity for {fuel.value} must be between 0 and 200 gCO2e/MJ")
        return value


class VoyagePlanRequest(ContractModel):
    vessel: VesselInput
    legs: tuple[VoyageLegInput, ...] = Field(min_length=1, max_length=MAX_VOYAGE_LEGS)
    start_date: date
    assumptions: MarketAssumptions = Field(default_factory=MarketAssumptions)

    @model_validator(mode="after")
    def legs_connect(self) -> "VoyagePlanRequest":
        for previous, following in zip(self.legs, self.legs[1:]):
            if previous.to_port != following.from_port:
                raise ValueError(
                    f"Legs must connect: a leg ends at {previous.to_port} "
                    f"but the next leg starts at {following.from_port}"
                )
        return self


# ---------------------------------------------------------------------------
# Prices: the boundary between the pure engine and the database resolver
# ---------------------------------------------------------------------------


class PriceLookup(ContractModel):
    """One green-fuel price the engine needs: fuel, bunkering port and departure date."""

    fuel: PlannerFuel
    port: str
    departure_date: date


class PricePoint(ContractModel):
    fuel: PlannerFuel
    port: str
    price_usd_per_mt: Decimal
    source: PriceSource
    executable: bool
    availability_window: str | None = None
    observed_at: datetime | None = None
    # Declared on the chosen ask, when present.
    carbon_intensity_gco2e_per_mj: Decimal | None = None
    energy_density_mj_per_kg: Decimal | None = None


# ---------------------------------------------------------------------------
# Results
# ---------------------------------------------------------------------------

PLANNER_DISCLAIMER = (
    "Indicative planning estimate from Verdaxis market data and the caller's assumptions. "
    "It is not a quote, an offer, or advice to trade. Only an order on the Verdaxis orderbook is executable."
)


class LegPlan(ContractModel):
    from_port: str
    to_port: str
    route: SeaRoute
    via: str
    distance_nm: Decimal
    distance_source: Literal["CALLER", "VERDAXIS_TABLE"]
    departure_date: date
    arrival_date: date
    sea_days: Decimal
    eu_scope_share: Decimal
    sea_energy_mj: Decimal
    berth_days: Decimal
    berth_eu_scope_share: Decimal
    berth_energy_mj: Decimal


class FuelUse(ContractModel):
    leg_index: int
    segment: Literal["SEA", "BERTH"]
    fuel: PlannerFuel
    tonnes: Decimal
    bunkering_port: str
    price: PricePoint | None
    cost_usd: Decimal | None


class StrategyResult(ContractModel):
    strategy_id: str
    label: str
    completeness: StrategyCompleteness
    fuel_uses: tuple[FuelUse, ...]
    in_scope_energy_mj: Decimal
    ghg_intensity_gco2e_per_mj: Decimal | None
    fueleu_target_gco2e_per_mj: Decimal
    fueleu_balance_tco2e: Decimal | None  # positive is surplus, negative is deficit
    intensity_sources: dict[PlannerFuel, IntensitySource]
    fuel_cost_usd: Decimal | None
    eu_ets_cost_eur: Decimal | None
    fueleu_cost_eur: Decimal | None  # deficit cost, or a negative value for surplus
    fueleu_cost_basis: Literal["PENALTY", "POOL", "SURPLUS_AT_POOL_PRICE", "NONE"]
    total_cost_usd: Decimal | None
    missing_inputs: tuple[str, ...]


class VoyagePlanResult(ContractModel):
    disclaimer: str = PLANNER_DISCLAIMER
    reporting_year: int
    legs: tuple[LegPlan, ...]
    strategies: tuple[StrategyResult, ...]
    recommended_strategy_id: str | None
    recommendation_reason: str
    assumptions: tuple[str, ...]
    warnings: tuple[str, ...]


# ---------------------------------------------------------------------------
# Market tools
# ---------------------------------------------------------------------------


class MarketProductInfo(ContractModel):
    code: str
    name: str
    fuel_type: str
    unit: str
    min_lot_size: Decimal
    delivery_points: tuple[str, ...]


class DeliveryPointInfo(ContractModel):
    name: str
    region: str
    is_eu: bool


class MarketCatalog(ContractModel):
    products: tuple[MarketProductInfo, ...]
    delivery_points: tuple[DeliveryPointInfo, ...]


class MarketSliceSnapshot(ContractModel):
    """Anonymous market evidence for one slice. It never names an organization or user."""

    market_product: str
    delivery_point: str
    availability_window: str
    best_bid_usd_per_mt: Decimal | None
    best_ask_usd_per_mt: Decimal | None
    bid_order_count: int
    ask_order_count: int
    bid_quantity_mt: Decimal
    ask_quantity_mt: Decimal
    demo_status: MarketDemoStatus
    benchmark_usd_per_mt: Decimal | None
    observed_at: datetime
    note: str

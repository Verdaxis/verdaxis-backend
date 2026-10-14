# Voyage Fuel MCP Server Implementation Plan

> **For agentic workers:** Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A read-only MCP endpoint on the staging backend (`/mcp`) with three tools (`list_markets`, `get_market_slice`, `plan_voyage_fuel`). An OpenClaw agent can use it to price a voyage's fuel strategy from deterministic code and Verdaxis market data.

**Architecture:** `app/schemas/voyage.py` (shared types) → `app/services/sea_distances.py` (static table) → `app/services/voyage_fuel_plan.py` (pure cost engine) ← `app/services/voyage_prices.py` (database reads) → `app/mcp_server.py` (official MCP SDK, stateless JSON, mounted in `app/main.py` outside production).

**Tech stack:** Python 3.12, FastAPI 0.135.3, Starlette 1.3.1, Pydantic v2, SQLAlchemy 2 async, `mcp==2.3.0` (`mcp.server.mcpserver.MCPServer`), pytest with SQLite in memory.

**Spec:** `docs/plans/2026-10-14-voyage-mcp-design.md`. Read it before any task.

## Global constraints

- Branch base: `origin/staging`. Feature branch: `feat/voyage-mcp-staging-20261014`. Deliver to `staging` by PR. `staging` requires the `test` and `postgres-analytics` checks.
- Read-only: no tool may insert, update or delete any row, or call any mutating service.
- No tool output contains organization, user, order owner, order id or support identifiers.
- Decimal arithmetic only, with the default 28-digit context. Quantize output fields only, with `ROUND_HALF_UP`:
  - money and tonnes: 0.01
  - energy (MJ): 1
  - intensity: 0.0001
  - days and shares: 0.0001
- Reuse `fueleu_year_target` (`app/services/fueleu.py`), `FUELEU_PENALTY_PER_TONNE` and `calculate_ets_score` (`app/services/compliance_scoring.py`), and the availability-window helpers (`app/services/availability_windows.py`). Do not redefine them.
- Never invent a price, rate or distance. A missing input leaves the cost unpriced and named in `missing_inputs`.
- Run unit tests with:
  `ENVIRONMENT=test RELEASE_SHA=test JWT_SECRET=test-secret-key-for-testing-minimum-32-chars DATABASE_URL="sqlite+aiosqlite:///:memory:" /Users/jonathanjie/Documents/workspace/Verdaxis/work/voyage-mcp-backend/venv/bin/python -m pytest <paths> -q`
- Do not add, remove or upgrade dependencies, except in Task 4 (`mcp==2.3.0` and its pins).
- Commit messages end with: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`

## Review focus

Most likely failures first:

1. **Missing assumptions** (no VLSFO price, EUA, pool price or USD/EUR rate): the affected strategies are `PARTIAL`, each missing input is named, there is no exception, and the reason says what to supply. (Task 2)
2. **No EU-scope legs** (Shanghai to Singapore only): conventional strategies have zero ETS and FuelEU cost. No green strategy is generated, because there is no in-scope segment. The recommendation is conventional. (Task 2)
3. **A green fuel with no default intensity** (bio-ethanol), or with an intensity at or above the target: no division by zero. The strategy is `PARTIAL` with `ghg_intensity_gco2e_per_mj[BIO_ETHANOL]` missing, or the minimum blend is skipped with a warning. (Task 2)
4. **Demo-only liquidity:** the price source is `DEMO_ORDER`, `executable` is false, and the result warns that the price is demo preview liquidity. It is never labeled `LIVE_ORDER`. (Task 3 and Task 2)
5. **A vessel without VLSFO** (methanol only): no conventional strategy, and no crash. (Task 2)

---

### Task 0: Contract, stub, design and plan (main thread, done)

**Files:** `app/schemas/voyage.py`, `app/services/sea_distances.py` (interface stub), `docs/plans/2026-10-14-voyage-mcp-design.md`, this plan.

All later tasks import these names exactly:

- From `app/schemas/voyage.py`:
  - enums: `PlannerFuel`, `SeaRoute`, `PriceSource`, `IntensitySource`, `StrategyCompleteness`
  - helper: `canonical_port_name`
  - requests: `VesselInput`, `VoyageLegInput`, `MarketAssumptions`, `VoyagePlanRequest`
  - prices: `PriceLookup`, `PricePoint`
  - results: `LegPlan`, `FuelUse`, `StrategyResult`, `VoyagePlanResult`, `PLANNER_DISCLAIMER`
  - market tools: `MarketProductInfo`, `DeliveryPointInfo`, `MarketCatalog`, `MarketSliceSnapshot`
  - constants: `MAX_VOYAGE_LEGS`, `SUPPORTED_PORTS`
- From `app/services/sea_distances.py`: `SeaDistance(distance_nm: Decimal, via: str)`, `SEA_DISTANCE_METHOD: str`, `sea_distance(from_port, to_port, route=SeaRoute.DEFAULT) -> SeaDistance | None`.

---

### Task 1: Sea distance table (DeepSeek V4.1 Flash)

**Files:**
- Modify: `app/services/sea_distances.py`. Fill `_DISTANCES` and `SEA_DISTANCE_METHOD`. Keep the interface unchanged.
- Create: `tests/unit/test_sea_distances.py`

**Interfaces:**
- Consumes: `SeaRoute` from `app/schemas/voyage.py`, `CANONICAL_DELIVERY_POINTS` from `app/market_catalog.py`.
- Produces: the unchanged `sea_distance()` interface, now with data.

**Data:**
- All 28 unordered pairs of the 8 catalog delivery points (Dalian, Busan, Shanghai, Singapore, Rotterdam, Houston, Los Angeles, Santos) need a `DEFAULT` entry.
- Every pair whose default route transits the Suez Canal also needs a `CAPE_OF_GOOD_HOPE` entry.
- Produce the numbers with the open-source `searoute` Python package, in a scratch virtual environment outside the repository: `python3 -m venv /tmp/searoute-venv && /tmp/searoute-venv/bin/pip install searoute`. Never add it to `requirements.txt`.
- Use the default network for `DEFAULT`. Use the package's passage restrictions to block Suez and Bab-el-Mandeb for `CAPE_OF_GOOD_HOPE`.
- Convert to nautical miles and round to the nearest 10. Set `via` from the passage used: "Suez Canal", "Panama Canal", "Cape of Good Hope", "Strait of Magellan" or "Direct".
- Write the table as a literal Python dict, sorted by key, with `Decimal("...")` values.
- Set `SEA_DISTANCE_METHOD` to a sentence naming the `searoute` version, the date (2026-10-14), the rounding rule, and that the values are planning estimates, not navigational distances.

- [ ] **Step 1: Write the failing tests** in `tests/unit/test_sea_distances.py`:

```python
from decimal import Decimal
from itertools import combinations

import pytest

from app.market_catalog import CANONICAL_DELIVERY_POINTS
from app.schemas.voyage import SeaRoute
from app.services.sea_distances import SEA_DISTANCE_METHOD, sea_distance

PORTS = [spec.name for spec in CANONICAL_DELIVERY_POINTS]


def test_every_pair_has_a_default_route():
    for first, second in combinations(PORTS, 2):
        assert sea_distance(first, second) is not None, (first, second)


def test_lookup_is_symmetric():
    for first, second in combinations(PORTS, 2):
        assert sea_distance(first, second) == sea_distance(second, first)


def test_cape_route_exists_exactly_where_default_uses_suez():
    for first, second in combinations(PORTS, 2):
        default = sea_distance(first, second)
        cape = sea_distance(first, second, SeaRoute.CAPE_OF_GOOD_HOPE)
        if default.via == "Suez Canal":
            assert cape is not None and cape.distance_nm > default.distance_nm
        else:
            assert cape is None


@pytest.mark.parametrize(
    ("first", "second", "route", "low", "high"),
    [
        ("Shanghai", "Singapore", SeaRoute.DEFAULT, 2100, 2400),
        ("Singapore", "Rotterdam", SeaRoute.DEFAULT, 8100, 8600),
        ("Singapore", "Rotterdam", SeaRoute.CAPE_OF_GOOD_HOPE, 11400, 12000),
        ("Shanghai", "Rotterdam", SeaRoute.DEFAULT, 10300, 10900),
        ("Busan", "Dalian", SeaRoute.DEFAULT, 400, 700),
        ("Houston", "Rotterdam", SeaRoute.DEFAULT, 4800, 5300),
    ],
)
def test_known_distances_are_plausible(first, second, route, low, high):
    distance = sea_distance(first, second, route)
    assert distance is not None
    assert Decimal(low) <= distance.distance_nm <= Decimal(high)


def test_unknown_pair_returns_none():
    assert sea_distance("Singapore", "Atlantis") is None


def test_method_is_recorded():
    assert "searoute" in SEA_DISTANCE_METHOD and "2026-10-14" in SEA_DISTANCE_METHOD
```

- [ ] **Step 2:** Run `pytest tests/unit/test_sea_distances.py -q`. Expected: FAIL, because the table is empty.
- [ ] **Step 3:** Generate the numbers with `searoute` and write the table.
- [ ] **Step 4:** Run the test file. Expected: all PASS. If a plausibility range fails, report the generated value. Do not edit the range.
- [ ] **Step 5:** Commit: `git add app/services/sea_distances.py tests/unit/test_sea_distances.py && git commit -m "feat: add static sea distances for catalog delivery points"`

---

### Task 2: Pure voyage cost engine (GPT-6 Astra)

**Files:**
- Create: `app/services/voyage_fuel_plan.py`
- Create: `tests/unit/test_voyage_fuel_plan.py`

**Interfaces:**
- Consumes:
  - the Task 0 types
  - `sea_distance` from Task 1. Tests pass `distance_nm` explicitly, except one test that is skipped when the table is empty.
  - `fueleu_year_target`, `FUELEU_PENALTY_PER_TONNE` and `calculate_ets_score`
  - `PRODUCTS_BY_CODE` and `DELIVERY_POINT_IDS` from `app/market_catalog.py`, for product availability at a port
- Produces:
  - `EU_PORTS: frozenset[str] = frozenset({"Rotterdam"})`
  - `FUEL_PROPERTIES: Mapping[PlannerFuel, FuelProperties]`, where `FuelProperties` is a frozen dataclass with `lcv_mj_per_kg: Decimal`, `ets_co2_t_per_t: Decimal` and `default_intensity_gco2e_per_mj: Decimal | None`. Values:
    - VLSFO: 41.0, 3.151, 91.16
    - UCOME_B100: 37.2, 0, 35.00
    - BIO_METHANOL: 19.9, 0, 31.00
    - E_METHANOL: 19.9, 0, 8.00
    - BIO_ETHANOL: 26.8, 0, None
    - SYNTHETIC_ETHANOL: 26.8, 0, None
  - `class VoyagePlanError(ValueError)`
  - `def required_price_lookups(request: VoyagePlanRequest) -> tuple[PriceLookup, ...]`
  - `def build_voyage_plan(request: VoyagePlanRequest, prices: Mapping[PriceLookup, PricePoint], *, today: date) -> VoyagePlanResult`

**Rules** (from the spec; follow exactly):

1. **Validation.** `build_voyage_plan` raises `VoyagePlanError` when:
   - `start_date` is more than 730 days after `today` or more than 30 days before it
   - a leg has no `distance_nm` and the table has no entry for its route
   - the CAPE route is requested for a pair that has none
2. **Legs.**
   - sea_days = distance / (speed × 24)
   - departure date of leg 0 = `start_date`
   - arrival = departure + ceil(sea_days) days
   - the next departure = arrival + ceil(berth days)
   - sea_energy = sea_days × sea_consumption × 41,000
   - berth_energy = berth_days × berth_consumption × 41,000
   - `distance_source` is `CALLER` when given, otherwise `VERDAXIS_TABLE`. `via` comes from the table, or is "Caller distance".
3. **Scope.**
   - sea share: 1 if both ports are in `EU_PORTS`, 0.5 if exactly one is, 0 if neither
   - berth share: 1 if the destination is in `EU_PORTS`, otherwise 0
4. **Bunkering.** A leg's SEA segment and its destination BERTH segment are both bought at the leg's `from_port`, on the leg's departure date.
5. **Price lookups.** `required_price_lookups` returns, deduplicated and in leg order, one `PriceLookup(fuel, leg.from_port, departure_date)` for each green fuel that meets all of these conditions:
   - it is in `compatible_fuels` and is not VLSFO
   - it is available at that port: the product's `available_delivery_point_ids` is None or contains the port's id
   - the leg has a SEA or BERTH segment with share > 0
6. **Intensity.** For each fuel, in order of precedence:
   - the caller's `ghg_intensity_gco2e_per_mj` override (`CALLER`)
   - otherwise the chosen PricePoint's `carbon_intensity_gco2e_per_mj` (`ORDER_DECLARED`)
   - otherwise the default (`DEFAULT_ILLUSTRATIVE`)
   - otherwise missing

   An `energy_density_mj_per_kg` on the PricePoint replaces the default LCV for that fuel use.
7. **Strategies.** Generate them in this order:
   - `CONVENTIONAL_PENALTY` and `CONVENTIONAL_POOL`, only if VLSFO is compatible
   - then, for each green fuel in `compatible_fuels` order that has at least one in-scope segment, `<FUEL>_MIN_BLEND` and `<FUEL>_FULL`

   Green fuel is burned only on segments with share > 0, covering the segment's whole energy for FULL and share s of it for MIN_BLEND. VLSFO covers the rest.
   - If the vessel lacks VLSFO, MIN_BLEND is skipped, and FULL is the only green strategy.
   - MIN_BLEND share: s = (I_vlsfo − target) / (I_vlsfo − I_green).
     - If I_green ≥ target, skip MIN_BLEND and add a warning.
     - If I_vlsfo ≤ target, s = 0, and MIN_BLEND is skipped as identical to conventional.
8. **Costs.**
   - Fuel cost = tonnes × the segment's price. VLSFO uses `assumptions.vlsfo_price_usd_per_mt[from_port]` (source `CALLER`, executable false). Green fuel uses `prices[lookup]`.
   - ETS:
     - in-scope fossil CO2 = Σ share × VLSFO tonnes × 3.151
     - `calculate_ets_score(co2, eua_price, year).estimated_cost_eur`
     - if no EUA price is given, ETS is unpriced unless the in-scope CO2 is 0, in which case it is 0
   - FuelEU:
     - E_in = Σ share × segment energy
     - I = energy-weighted intensity over in-scope energy
     - CB_g = (target − I) × E_in
     - balance_t = CB_g / 10^6
     - A deficit is priced by basis:
       - `PENALTY` = |CB_g| / (I × 41,000) × `FUELEU_PENALTY_PER_TONNE`
       - `POOL` = |balance_t| × pool price
     - Conventional strategies use their own basis. Green strategies with a deficit use the cheaper of the priced bases.
     - Surplus = −balance_t × pool price, basis `SURPLUS_AT_POOL_PRICE`. If there is no pool price, the surplus is unvalued, but this is not a missing input.
     - A zero balance gives basis `NONE` and cost 0.
     - If E_in = 0, the FuelEU cost is 0 and the basis is `NONE`.
   - total_usd = fuel_usd + (ets_eur + fueleu_eur) × usd_per_eur. It is None when any part is unpriced. The USD/EUR rate is needed only if an EUR part is nonzero.
9. **Completeness.**
   - `PARTIAL` if anything is unpriced.
   - `missing_inputs` uses stable names:
     - `vlsfo_price_usd_per_mt[<Port>]`
     - `eua_price_eur_per_tco2`
     - `fueleu_pool_price_eur_per_tco2e`
     - `usd_per_eur`
     - `ghg_intensity_gco2e_per_mj[<FUEL>]`
     - `market_price[<FUEL>@<Port>]`
10. **Recommendation.**
    - The cheapest `COMPLETE` strategy by `total_cost_usd`; ties go to the earlier strategy.
    - The reason names the strategy and its total.
    - If no strategy is complete, the recommendation is None, and the reason lists the union of `missing_inputs`.
11. **Warnings and assumptions.**
    - Always include these assumptions: the bunkering assumption, the 41,000 MJ/t VLSFO basis, and the FuelEU caveat (an annual per-ship balance, so this is the voyage contribution). Also: "EU scope ignores outermost-region and transshipment rules", and that ETS methane and N2O are not included.
    - Warn when a used price is `DEMO_ORDER`, or `BENCHMARK`.
    - Warn that green fuels need a proof of sustainability for zero ETS CO2 and FuelEU credit.
    - `reporting_year` = `assumptions.reporting_year` or `start_date.year`.

**Golden example.** All tests build from this. Expected values were computed independently with the formulas above.

```python
from datetime import date
from decimal import Decimal as D

from app.schemas.voyage import (
    MarketAssumptions, PlannerFuel, PriceLookup, PricePoint, PriceSource,
    StrategyCompleteness, VesselInput, VoyageLegInput, VoyagePlanRequest,
)
from app.services.voyage_fuel_plan import build_voyage_plan, required_price_lookups

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
            berth_consumption_mt_per_day=D("5"), compatible_fuels=("VLSFO", "UCOME_B100"),
        ),
        legs=(
            VoyageLegInput(from_port="Shanghai", to_port="Singapore", distance_nm=D("2200"), berth_days_at_destination=D("1")),
            VoyageLegInput(from_port="Singapore", to_port="Rotterdam", distance_nm=D("8300"), berth_days_at_destination=D("2")),
        ),
        start_date=date(2026, 11, 1),
        assumptions=MarketAssumptions(**assumptions),
    )


def golden_prices(request):
    (lookup,) = required_price_lookups(request)
    return {lookup: PricePoint(fuel=PlannerFuel.UCOME_B100, port="Singapore", price_usd_per_mt=D("1445"),
                               source=PriceSource.LIVE_ORDER, executable=True, availability_window="2026-11")}


def strategy(result, strategy_id):
    return next(s for s in result.strategies if s.strategy_id == strategy_id)


def close(actual, expected, tolerance="0.05"):
    return abs(actual - D(expected)) <= D(tolerance)
```

Expected numbers (target 2026 = 89.3368; sea days 6.5476 and 24.7024; E_in = 20,665,952 MJ):

| Strategy | fuel_cost_usd | eu_ets_cost_eur | fueleu_balance_tco2e | fueleu_cost_eur | total_cost_usd |
|---|---|---|---|---|---|
| CONVENTIONAL_PENALTY | 1078760.48 | 127060.32 | -37.68 | 24194.29 | 1245140.55 |
| CONVENTIONAL_POOL | 1078760.48 | 127060.32 | -37.68 | 4106.92 | 1223044.44 |
| UCOME_B100_MIN_BLEND | 1100975.91 | 122935.39 | 0.00 | 0.00 | 1236204.84 |
| UCOME_B100_FULL | 1763062.08 | 0.00 | 1122.92 | -122398.47 | 1628423.77 |

In UCOME_B100_MIN_BLEND, UCOME is 35.71 t and VLSFO 1232.60 t, with share s = 0.032464. In UCOME_B100_FULL, UCOME is 1100.05 t. The recommendation is `CONVENTIONAL_POOL`.

- [ ] **Step 1: Write the failing tests** in `tests/unit/test_voyage_fuel_plan.py`, with these test functions (use the helpers above):
  - `test_required_price_lookups_ask_for_ucome_at_singapore_only`: exactly one lookup, (UCOME_B100, Singapore, 2026-11-09). Per Rule 2, leg 0 arrives 2026-11-01 + ceil(6.5476) = 2026-11-08, and leg 1 departs after ceil(1) berth day, on 2026-11-09.
  - `test_legs_have_scope_and_energy`: shares (0, 0) for leg 0 and (0.5, 1) for leg 1. Sea energy is 10,738,095 and 40,511,905 MJ, berth energy 205,000 and 410,000.
  - `test_golden_strategies`: every value in the table, using `close()`.
  - `test_recommends_cheapest_complete_strategy`: `CONVENTIONAL_POOL`.
  - `test_missing_assumptions_make_strategies_partial` (Review focus 1): with `fueleu_pool_price_eur_per_tco2e=None` and `usd_per_eur=None`, every strategy is `PARTIAL`, the recommendation is None, and the reason names `usd_per_eur`.
  - `test_missing_vlsfo_price_names_the_port`: with Singapore missing from the VLSFO prices, `missing_inputs` contains `vlsfo_price_usd_per_mt[Singapore]`.
  - `test_voyage_without_eu_scope_has_no_green_strategy` (Review focus 2): Shanghai to Singapore only. Strategies are the two conventional ones, with ETS 0 and FuelEU 0.
  - `test_fuel_without_default_intensity_is_partial` (Review focus 3): BIO_METHANOL replaced by BIO_ETHANOL priced at Singapore. Its strategies are PARTIAL, with `ghg_intensity_gco2e_per_mj[BIO_ETHANOL]`, and there is no ZeroDivisionError.
  - `test_green_fuel_at_or_above_target_skips_min_blend`: a UCOME override of 95 means no MIN_BLEND, and a warning.
  - `test_demo_price_adds_warning` (Review focus 4): source DEMO_ORDER means a warning containing "demo".
  - `test_vessel_without_vlsfo_has_no_conventional_strategy` (Review focus 5): compatible only UCOME_B100 gives only `UCOME_B100_FULL`.
  - `test_order_declared_intensity_is_used`: a PricePoint with intensity 14 gives `intensity_sources[UCOME_B100] == ORDER_DECLARED` and a smaller MIN_BLEND share.
  - `test_start_date_too_far_ahead_is_rejected`: 2029-01-01 with today 2026-10-14 raises `VoyagePlanError`.
  - `test_table_distance_used_when_caller_omits_distance`: skip if `sea_distance("Shanghai", "Singapore") is None`. Otherwise `distance_source == "VERDAXIS_TABLE"`.
- [ ] **Step 2:** Run the tests. Expected: FAIL (the module does not exist).
- [ ] **Step 3:** Implement `app/services/voyage_fuel_plan.py` per the rules. Use small private functions: `_legs`, `_segments`, `_strategy`, `_price_fuel_uses`, `_fueleu_cost`, `_recommend`. Use no I/O and no clock.
- [ ] **Step 4:** Run the tests. Expected: all PASS.
- [ ] **Step 5:** Commit: `git add app/services/voyage_fuel_plan.py tests/unit/test_voyage_fuel_plan.py && git commit -m "feat: add deterministic voyage fuel cost engine"`

---

### Task 3: Market price resolver and slice snapshot (DeepSeek V4.1 Flash)

**Files:**
- Create: `app/services/voyage_prices.py`
- Create: `tests/unit/test_voyage_prices.py`
- Maybe create: `app/services/public_order_scope.py` (see Step 3)

**Interfaces:**
- Consumes:
  - `PriceLookup`, `PricePoint`, `PriceSource` and `MarketSliceSnapshot` (Task 0)
  - `normalize_availability_window` and `SPOT_WINDOW` (`app/services/availability_windows.py`)
  - `get_benchmark_quotes` (`app/services/benchmarks.py`)
  - `DELIVERY_POINT_IDS`, `PRODUCT_IDS` and `PRODUCTS_BY_CODE` (`app/market_catalog.py`)
  - `OrderBookOrder`, `OrderSide` and `OrderBookStatus` (`app/models/orderbook.py`)
  - `OrganizationProvenance` (`app/models/user.py`)
  - `MarketDemoStatus` (`app/schemas/market_activity.py`)
- Produces:
  - `def candidate_windows(departure_date: date, *, today: date) -> tuple[str, ...]`
  - `async def resolve_product_prices(db: AsyncSession, lookups: Sequence[PriceLookup], *, now: datetime) -> dict[PriceLookup, PricePoint]`
  - `async def market_slice_snapshot(db: AsyncSession, *, market_product: str, delivery_point: str, availability_window: str, now: datetime) -> MarketSliceSnapshot`
  - `class MarketDataUnavailable(RuntimeError)`, raised when a database error occurs, with the cause chained

**Rules:**

1. `candidate_windows`: return `"SPOT"` first if `departure_date <= today + 30 days`, then `"YYYY-MM"` of the departure, then `"YYYY-Qn"`.
2. Visibility: use exactly the public order visibility of `/api/orderbook/asks`, which is `_public_order_scope(side=OrderSide.ASK, as_of=now)` in `app/routers/orderbook.py`.
3. Batch: one SELECT of the visible ASKs for all (product id, delivery point id) pairs and all candidate windows. Use `.in_()` or a tuple filter, never a query per lookup. Then pick per lookup in memory: the first candidate window with any ask, and in it the lowest `price_per_mt_usd`, with ties broken by `acceptance_ordinal`.
4. Source:
   - `DEMO_ORDER` with `executable=False` if the order's `provenance` is `OrganizationProvenance.DEMO`
   - otherwise `LIVE_ORDER` with `executable=True` if the provenance is `REAL`
   - skip `TEST` and `UNKNOWN` orders
   - copy `carbon_intensity_gco2_mj` and `energy_density_mj_kg` from the order into the PricePoint
5. Fallback: if no window has an ask, call `get_benchmark_quotes` once for all unresolved lookups (month window). Use the quote's price with source `BENCHMARK` and `executable=False`. Otherwise leave the lookup out of the result.
6. `market_slice_snapshot` uses one visible-orders query for both sides of the slice:
   - best bid is the max BID price, best ask the min ASK price
   - counts and summed `remaining_quantity_mt` per side
   - `demo_status`: `REAL_ONLY`, `DEMO_ONLY`, `MIXED`, or `NOT_APPLICABLE` when there are no orders
   - the benchmark comes from one `get_benchmark_quotes` call
   - `note` explains the provenance in one sentence
   - unknown products or ports raise `ValueError` with the supported list
7. Never return or log organization, user or order identifiers.
8. Wrap `sqlalchemy.exc.SQLAlchemyError` in `MarketDataUnavailable("Market data is temporarily unavailable")`.

- [ ] **Step 1: Write the failing tests** in `tests/unit/test_voyage_prices.py`, using the SQLite fixtures pattern of `tests/unit/test_forward_curve_market_slices.py` (an in-memory engine, `Base.metadata.create_all`, and seeded `Product`, `DeliveryPoint` and `Organization` rows). Tests:
  - `test_candidate_windows_near_and_far`: departure today+10 gives ("SPOT", "2026-10", "2026-Q4"). Departure 2027-02-03 gives ("2027-02", "2027-Q1").
  - `test_lowest_real_ask_wins_and_is_executable`: two REAL asks at 1500 and 1445 in "2026-11", so 1445, `LIVE_ORDER`, executable.
  - `test_demo_ask_is_never_executable` (Review focus 4): only a DEMO ask gives `DEMO_ORDER` and executable False.
  - `test_earlier_window_preferred`: an ask in "2026-11" and a cheaper one in "2026-Q4", with departure 2026-11-09, picks the month window.
  - `test_declared_intensity_is_copied`: an ask with `carbon_intensity_gco2_mj=14` gives a PricePoint with 14.
  - `test_benchmark_fallback`: no asks, but a benchmark row gives source `BENCHMARK` and executable False.
  - `test_no_data_leaves_lookup_unresolved`: the result has no key.
  - `test_single_query_for_many_lookups`: use a SQLAlchemy `before_cursor_execute` event counter. Ten lookups cause at most 2 SELECTs of `orderbook_orders`.
  - `test_snapshot_has_no_identifiers`: for every UUID of the seeded orgs, users and orders, the string is absent from `snapshot.model_dump_json()`.
  - `test_snapshot_mixed_status`: one REAL and one DEMO ask give `MIXED`.
  - `test_database_error_is_wrapped`: a session whose execute raises `OperationalError` gives `MarketDataUnavailable`.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3:** Implement. On reusing `_public_order_scope`: if `_apply_public_marketplace_scope` and `_public_order_scope` in `app/routers/orderbook.py` use only module-level imports and no router state, move them to `app/services/public_order_scope.py` as `apply_public_marketplace_scope` and `public_order_scope`. Then import them back into the router under the old private names, so no other router line changes. Otherwise import `_public_order_scope` from the router, with a comment. Report which you chose and why.
- [ ] **Step 4:** Run the new tests plus `tests/unit/test_forward_curve_market_slices.py` and every `tests/unit/test_orderbook*.py`. Expected: all PASS.
- [ ] **Step 5:** Commit: `git add -A app/services tests/unit/test_voyage_prices.py app/routers/orderbook.py && git commit -m "feat: add batched market price resolver for voyage planning"`

---

### Task 4: MCP server, mount and dependency pins (GPT-6 Astra, after Tasks 1 to 3 merge)

**Files:**
- Create: `app/mcp_server.py`
- Modify: `app/main.py` (lifespan and mount only), `requirements.txt`, `constraints.txt`
- Create: `tests/unit/test_mcp_server.py`, `docs/agent-mcp.md`
- Modify: `ARCHITECTURE.md` (file map lines), `CLAUDE.md` (one line under Public routes)

**Interfaces:**
- Consumes: everything above.
- Produces:
  - `MCP_ENVIRONMENTS = frozenset({"development", "test", "staging"})`
  - `def mcp_enabled(environment: str) -> bool`
  - `def build_mcp_server() -> MCPServer`
  - `def market_catalog() -> MarketCatalog`
  - `MCP_PATH = "/mcp"`
  - `MCP_MAX_CONCURRENT_TOOL_CALLS = 1` per worker. Corrected in review (2026-10-14) from 4: each worker has 3 database checkouts and public market reads may hold 2, so MCP tools take at most 1. Comment the reason.

**Rules:**

1. Dependencies: add `mcp==2.3.0` to `requirements.txt`. Add exact pins to `constraints.txt` for `mcp` and every new transitive package. From the dry run these are attrs 26.1.0, httpcore2 2.13.1, httpx2 2.13.1, jsonschema 4.26.0, jsonschema-specifications 2025.9.1, mcp 2.3.0, mcp-types 2.3.0, opentelemetry-api 1.45.1, referencing 0.37.0, rpds-py 2026.9.1, sse-starlette 3.5.0 and truststore 0.10.5. Re-verify them with `pip install --dry-run -r requirements.txt -c constraints.txt` in a fresh venv.

   Run `pip-audit --strict -r requirements.txt` and `pip-audit --strict -r constraints.txt --no-deps`. Both must pass. If one fails, stop and report it.
2. **Server.** Create `MCPServer(name="verdaxis-market", instructions=...)`. The instructions say:
   - the tools are read-only
   - prices come from Verdaxis market data and caller assumptions
   - the agent must state the provenance of each price, and must never present a demo or benchmark price as executable
3. **Tools.** Register them with descriptions written for an LLM. Arguments and results are the Task 0 Pydantic models, so the SDK publishes input and output schemas.
   - `list_markets() -> MarketCatalog`. No database: built from `app/market_catalog.py` and `EU_PORTS`.
   - `get_market_slice(market_product, delivery_point, availability_window) -> MarketSliceSnapshot`. Normalize the window with `normalize_availability_window`.
   - `plan_voyage_fuel(request: VoyagePlanRequest) -> VoyagePlanResult`:
     - call `required_price_lookups`
     - then `resolve_product_prices(db, lookups, now=utcnow)`
     - then `build_voyage_plan(request, prices, today=utcnow.date())`

   Open each database session with `async with AsyncSessionLocal() as db` (find the session factory in `app/database.py`). Never commit.
4. **Errors.** `VoyagePlanError`, `ValueError` and Pydantic validation errors become tool errors with the message. `MarketDataUnavailable` becomes "Market data is temporarily unavailable." Any other exception is logged with structlog (no request body) and becomes "Internal error". Use the SDK's tool-error mechanism (for example, raising its `ToolError`, or whatever `mcp.server.mcpserver.exceptions` defines). Never return HTTP 500.
5. **Capacity.** One module-level `anyio.CapacityLimiter(MCP_MAX_CONCURRENT_TOOL_CALLS)` per worker, used with `acquire_nowait`. When it is full, return the tool error "Verdaxis market tools are busy; retry shortly."
6. **Transport.** `server.streamable_http_app(stateless_http=True, json_response=True, transport_security=TransportSecuritySettings(enable_dns_rebinding_protection=True, allowed_hosts=[...], allowed_origins=[]))`:
   - allowed hosts for staging: `api-staging.verdaxis.exchange`
   - for development and test: `127.0.0.1:*`, `localhost:*`, `testserver`

   Read the SDK source to confirm how wildcards and an empty origin list behave. Requests with no `Origin` header must be accepted. Requests with any `Origin` must be rejected.
7. **Mount** in `app/main.py`:
   - When `mcp_enabled(settings.ENVIRONMENT)`, `app.mount(MCP_PATH, mcp_app)`.
   - In the existing `lifespan`, wrap the `yield` in `async with mcp.session_manager.run():` only when enabled, inside the existing try/finally. The dispatcher start and stop must not change.
   - Check that `POST /mcp` works without a redirect. If Starlette mounting turns `/mcp` into `/mcp/`, mount the app so that the exact path `/mcp` serves it, and test that.
8. **Production:** not mounted. A test sets the environment to production, builds the gate decision with `mcp_enabled("production")`, and asserts False. A second test asserts that `/mcp` is absent from the routes of an app built with production settings, if that can be done without starting the database. Otherwise the first test is enough.

- [ ] **Step 1: Write the failing tests** in `tests/unit/test_mcp_server.py`. Use `httpx.AsyncClient(transport=ASGITransport(app=<the mounted starlette app inside a lifespan>))`, or the SDK's in-memory client if it has one. Override the session factory with the SQLite engine and seed data from the Task 3 tests (import that fixture module or copy its seeding helper). Tests:
  - `test_tools_are_listed`: the names are exactly {"list_markets", "get_market_slice", "plan_voyage_fuel"}, and each has an input schema.
  - `test_list_markets_has_eight_points_and_rotterdam_is_eu`
  - `test_plan_voyage_fuel_golden_through_mcp`: the Task 2 golden request with a seeded REAL UCOME ask at Singapore for 2026-11 at 1445, and the clock patched to 2026-10-14. The structured result's recommendation is `CONVENTIONAL_POOL`.
  - `test_invalid_port_is_tool_error_not_500`: from_port "Hamburg" gives `isError` true, with a message listing supported ports.
  - `test_no_identifiers_in_any_tool_output`: no seeded organization, user or order UUID string appears in any tool's JSON.
  - `test_busy_limiter_returns_retry_message`: with the limiter patched to 0 capacity, the message contains "retry shortly".
  - `test_browser_origin_rejected`: a request with `Origin: https://evil.example` is rejected (403 or 421 per the SDK).
  - `test_mcp_disabled_in_production`
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3:** Implement. Make the dependency pins first, install them into the shared venv, and confirm the full unit suite still collects.
- [ ] **Step 4:** Run the full unit suite with the global-constraints command on `tests/unit/`. Expected: all PASS except failures that also fail on `origin/staging`. Report the pass and fail counts, and the failures on `origin/staging` if any.
- [ ] **Step 5:** Write `docs/agent-mcp.md`:
  - what the endpoint is, where it runs, and that it is read-only
  - the three tools with an example request and response for the golden voyage
  - the limits
  - how to connect OpenClaw: `openclaw --profile <name> mcp add ...` with transport streamable HTTP and URL `https://api-staging.verdaxis.exchange/mcp`
  - that production is off

  Update the `ARCHITECTURE.md` file map (the four new modules, and the schema file) and one `CLAUDE.md` line under the public routes: `POST /mcp -- read-only MCP endpoint (development/test/staging only; see docs/agent-mcp.md)`.
- [ ] **Step 6:** Commit: `git add -A && git commit -m "feat: serve read-only voyage and market tools over MCP on staging"`

---

### Task 5: Integration and review (main thread)

- [ ] Merge the Task 1 to Task 3 branches into the feature branch. Resolve conflicts, then run Task 4.
- [ ] Run the full unit suite. Run `pip-audit` as in Task 4.
- [ ] Whole-branch review by GPT-6 Astra, read-only (`router-task --model astra --read-only`): correctness against the spec, Review focus items, security (read-only, no identifiers, DNS-rebinding protection), and test quality. Fix confirmed findings.
- [ ] Push, and open a PR into `staging`. Wait for `test` and `postgres-analytics`. Merge when green.

### Task 6: Staging deploy (main thread, operator-authorized 2026-10-14)

- [ ] On `194.233.68.86` as `jons-openclaw`, read `/home/verdaxis-prod/verdaxis/PRODUCTION_HOST.md` and the verdaxis section of `~/VPS_MAP.md`. Confirm that Caddy proxies `/mcp` for `api-staging.verdaxis.exchange`.
- [ ] Run `./scripts/deploy.sh --dry-run` per `README.md`. Record `APPROVED_RELEASE_SHA`. Read the current Alembic revision. Because this change has no migration, the target is the current revision. Confirm that the pair is allowed in `deploy/migration-checkpoints.tsv`. If it is not, stop and report.
- [ ] Deploy with the four variables. Check `/health/ready`: the status, database, environment and full SHA must match.
- [ ] `curl` the MCP endpoint: list the tools, and call `list_markets`.

### Task 7: OpenClaw acceptance (DeepSeek V4.1 Flash via OpenCode Go)

- [ ] On the staging host, create the profile with `openclaw --profile verdaxis-test`. Do not touch the existing profiles or the four running gateways.
- [ ] Configure the model provider `opencode-go/deepseek-v4.1-flash`, reusing the host's existing OpenCode Go credential method without copying the secret into chat or the repository. Then add the MCP server.
- [ ] `openclaw --profile verdaxis-test mcp probe` lists the three tools.
- [ ] `openclaw --profile verdaxis-test agent --local --model opencode-go/deepseek-v4.1-flash -m "<Shanghai -> Singapore -> Rotterdam request with ship details and assumptions>"`. Accept when:
  - the agent called `plan_voyage_fuel`
  - its answer names the recommended strategy and its total
  - it states the price provenance
  - it does not call a demo or benchmark price executable
- [ ] Save the transcript to `outputs/` in the workspace.

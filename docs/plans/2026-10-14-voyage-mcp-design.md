# Voyage Fuel MCP Server: Design

Date: 2026-10-14
Status: Approved by the founder for build on staging ("yes, work on this", 2026-10-14).
Vision: `outputs/Verdaxis Agentic Marketplace Vision - 2026-10-14.md` in the workspace, section 8, step 1.

## Goal

Let any MCP-capable agent (first OpenClaw) ask Verdaxis: "My ship sails Shanghai to Singapore to Rotterdam. What is the cheapest compliant fuel strategy?" Verdaxis answers with costed strategies built from deterministic code and Verdaxis market data. The model only talks. Code calculates.

Success on staging:

1. An OpenClaw agent connected to `https://api-staging.verdaxis.exchange/mcp` lists the Verdaxis tools and calls them.
2. For the example voyage, `plan_voyage_fuel` returns at least one complete, costed strategy. Each price carries its source, and the answer is reproducible.
3. Nothing in the server can create, change or cancel market data.

## Scope

In scope: one read-only MCP endpoint with three tools, a static sea-distance table, a pure voyage cost engine, a market price resolver, docs, and tests.

Out of scope for this step: write tools (orders), agent identity and mandates, OAuth, stored vessels or voyages, a chat UI, production enablement, and a forward-curve tool.

## Decisions

| Decision | Choice | Reason |
|---|---|---|
| Protocol library | Official `mcp` SDK 2.3.0 (`mcp.server.mcpserver.MCPServer`) | It supports the 2026-07-28 spec and older clients. The protocol changes quickly; the SDK absorbs that. Cost: about 12 pinned packages, which pass the `pip install` dry run against `constraints.txt`. |
| Transport | Streamable HTTP, `stateless_http=True`, `json_response=True`, mounted at `/mcp` | Staging runs 4 Uvicorn workers, so sessions held in memory would break. Read-only tools need no server push. |
| Enablement | Mounted when `ENVIRONMENT` is `development`, `test` or `staging`. Not mounted in `production`. | No new environment variable, so the deployed environment allowlist does not change. Production enablement is a later, deliberate change. |
| Authentication | None (public read-only data, like `/api/orderbook/asks`) | Write tools come later with OAuth and mandates. |
| Host protection | SDK `TransportSecuritySettings`: DNS-rebinding protection on, allowed hosts for the environment, no browser origins | Agents are not browsers. |
| Load control | A per-worker capacity limit on tool calls and the SDK's request body limit | The tools read the database. Public reads already use admission control. |
| Port set | The eight catalog delivery points only | Distances, EU scope and prices are known for these. An unknown port returns a clear error listing the supported ports. |
| Conventional fuel price | Caller supplies VLSFO price per port | Verdaxis has no VLSFO market. The engine never invents a price. |
| EUA price, FuelEU pool price, USD/EUR rate | Caller supplies them | Verdaxis has no feed. A missing input leaves that cost unpriced, as `calculate_ets_score` already does. |

## Components

Each unit has one job and a typed interface. The shared types live in `app/schemas/voyage.py`, written first, so that parallel work fits together.

1. `app/schemas/voyage.py`: request, result and price types. It contains no logic.
2. `app/services/sea_distances.py`: a static, symmetric table of sea distances (nautical miles) between the eight delivery points. Each pair has a default route, plus a Cape of Good Hope alternative where the default transits Suez. The module states how the numbers were produced.
3. `app/services/voyage_fuel_plan.py`: the pure cost engine. It has no database, network or clock access, and takes `today` as an argument. It also holds the fuel property table.
4. `app/services/voyage_prices.py`: database reads. It resolves the best price for a green product at a port and window, and builds a market slice snapshot. It only reads.
5. `app/mcp_server.py`: builds the `MCPServer`, registers the tools, applies the capacity limit, and mounts the app with its lifespan.

## Tools

| Tool | Input | Output |
|---|---|---|
| `list_markets` | none | Catalog products (code, name, fuel type, unit, minimum lot, delivery points) and the delivery points (name, region, EU flag). |
| `get_market_slice` | market product, delivery point, availability window | Best bid, best ask, order counts, benchmark, provenance (live, demo, benchmark or none) and the time of the snapshot. |
| `plan_voyage_fuel` | vessel, legs, start date, caller assumptions | Legs with distance, sea days, EU scope and energy; costed strategies; a recommendation; assumptions; warnings. |

Every result says that it is an indicative planning estimate, not a quote or an offer.

## Cost engine

**Legs.** Sea days = distance / (speed × 24). Distance comes from the caller, or from the table for the chosen route. Sea energy = sea days × sea consumption (tonnes VLSFO-equivalent per day) × 41,000 MJ/t. Berth energy at a port = berth days × berth consumption × 41,000 MJ/t.

**EU scope** (EU ETS and FuelEU):

- EU to EU: 100 percent.
- Between an EU port and a non-EU port, in either direction: 50 percent.
- Non-EU to non-EU: 0 percent.
- At berth in an EU port: 100 percent.

Rotterdam is the only EU delivery point today. Outermost regions and transshipment-port rules are ignored, and the result says so.

**Bunkering assumption.** Fuel for a leg, and for the berth stay at its destination, is bought at the leg's departure port. The departure date sets the availability window.

**Strategies.**

1. `CONVENTIONAL_PENALTY`: all VLSFO. The FuelEU deficit is priced at the penalty.
2. `CONVENTIONAL_POOL`: all VLSFO. The deficit is priced at the caller's pool price.
3. For each green product the vessel can burn that has a price at the departure port of an in-scope segment:
   - `<PRODUCT>_MIN_BLEND`: the smallest share of the product on in-scope segments that brings in-scope intensity to the FuelEU target. The share is s = (I_conventional − target) / (I_conventional − I_product), applied to the whole segment.
   - `<PRODUCT>_FULL`: the product covers all in-scope segments. The surplus is valued at the pool price.

**Cost lines per strategy.**

- Fuel: tonnes × price at the bunkering port.
- EU ETS: in-scope tonnes of fossil fuel × CO2 factor × EUA price × phase-in for the reporting year (100 percent from 2026).
- FuelEU: CB = (target − actual in-scope intensity) × in-scope energy.
  - A deficit is priced as the penalty |CB| / (actual intensity × 41,000) × 2,400 EUR (Annex IV, as in `compliance_scoring.py`), or as the pool purchase |CB| / 10^6 × pool price.
  - A surplus is valued at |CB| / 10^6 × pool price.
- Total in USD, using the caller's USD/EUR rate.

A strategy with any unpriced line is `PARTIAL`. Its total is null, and it lists the missing inputs. The recommendation is the cheapest `COMPLETE` strategy, or none with the list of missing inputs.

**Fuel properties.** These are defaults that the caller can override.

| Fuel | LCV (MJ/kg) | ETS CO2 factor (t/t) | Default well-to-wake intensity (gCO2e/MJ) |
|---|---|---|---|
| VLSFO | 41.0 | 3.151 | 91.16 |
| UCOME B100 | 37.2 | 0 | 35.00 |
| Bio-methanol | 19.9 | 0 | 31.00 |
| e-Methanol | 19.9 | 0 | 8.00 |
| Bio-ethanol | 26.8 | 0 | none |
| Synthetic ethanol | 26.8 | 0 | none |

- The intensity defaults are the existing illustrative values in `compliance_scoring.FUEL_GHG_INTENSITIES`.
- A green fuel's ETS factor is 0 only with certified sustainability. The result warns that a proof of sustainability is required.
- A product with no default intensity needs a caller value before its compliance can be priced.

**FuelEU caveat.** FuelEU compliance is annual per ship. The engine gives this voyage's contribution to the annual balance. The actual penalty depends on the whole year, banking, borrowing and pooling. Every result states this.

## Prices

`voyage_prices.resolve_product_prices` resolves each (product, port, departure date) as follows:

1. Candidate windows in order: `SPOT` if departure is within 30 days of today, then the departure month (`YYYY-MM`), then its quarter (`YYYY-Qn`).
2. In the first window that has one, take the lowest open ASK in the slice. The source is `LIVE_ORDER`, or `DEMO_ORDER` for demo liquidity, which is never presented as executable.
3. Otherwise use the benchmark quote for the month window. The source is `BENCHMARK`, and it is indicative.
4. Otherwise the price is unpriced.

VLSFO prices come from the caller, with source `CALLER`.

Refinements from plan review (2026-10-14):

- **One batched read.** Fetch the open public ASKs for all needed (product, delivery point) pairs and candidate windows in one query, then choose in memory. Use the existing plural benchmark lookup. Do not query one slice at a time.
- **Same visibility as the public API.** Apply the visibility rules of the public order endpoints (`_public_order_scope` in `app/routers/orderbook.py`): open, unexpired and publicly visible orders only. Classify demo liquidity by the order's `provenance`.
- **Certified values from the order.** When the chosen ask has `carbon_intensity_gco2_mj` or `energy_density_mj_kg`, the engine uses those values for that fuel and port, and marks the intensity source as `ORDER_DECLARED`. Otherwise it uses the defaults, marked `DEFAULT_ILLUSTRATIVE`, or the caller's override, marked `CALLER`.
- **No identities.** No tool output contains organization, user, order owner or support identifiers, only prices, quantities, windows and provenance. A test checks this.
- **Reuse, don't redefine.** Use `fueleu_year_target` (`app/services/fueleu.py`), `FUELEU_PENALTY_PER_TONNE` (`app/services/compliance_scoring.py`) and the availability-window helpers.
- **Lifespan.** The MCP session manager runs inside the existing `lifespan` in `app/main.py`, which already starts the market event dispatcher. Wrap it there; don't replace it.

## Error handling

- Validation errors (unknown port, unknown fuel, a non-positive speed or consumption, more than 10 legs, a start date more than 2 years ahead) return a tool error with a clear message. They never return a 500.
- A database failure returns a tool error: "Market data is temporarily unavailable." It never leaks internals.
- At capacity, the tool error says "busy, retry shortly".

## Testing

- Unit tests:
  - the distance table: symmetry, every pair present, plausibility ranges
  - the engine: golden numbers, scope rules, the minimum-blend formula, `PARTIAL` handling
  - the price resolver on SQLite: live, demo and benchmark fallbacks
  - MCP over an in-process ASGI client: tool listing, every tool, validation errors, and the production-off gate
- CI: the existing `test` and `postgres-analytics` checks. `pip-audit --strict` covers the new pins.
- Staging acceptance: `curl` a tool listing at `https://api-staging.verdaxis.exchange/mcp`, then an OpenClaw `--profile verdaxis-test` agent turn that plans Shanghai to Singapore to Rotterdam.

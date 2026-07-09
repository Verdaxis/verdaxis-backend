# Market Activity Contract v1 Plan

## Objective

Make Verdaxis market activity data explicit about source, scope, demo/reference status, and freshness before adding more monitoring UI. The goal is to stop the frontend from guessing whether a value came from a confirmed trade, live orderbook depth, demo seed data, benchmark reference data, or no usable data.

This is a backend-first staging slice. Frontend changes should consume the contract only after the API shape is reviewed and stable.

## Context

Recent Forward Curve and Market Intelligence feedback showed that users need clearer answers to basic market questions:

- Is this a live market signal or demo seed data?
- Is the value specific to this product, port, and window, or is it broader regional/reference data?
- Is the number from a confirmed trade, an orderbook indication, a benchmark seed, or a model/reference placeholder?
- Is the market quiet because there is no data, or because we are intentionally hiding incomplete analytics?

Current backend surfaces answer parts of this inconsistently:

- `/api/trade-tape` now exposes `scope` and `provenance_kind`.
- `/api/curves/forward/board` exposes `benchmark_source` and `is_demo_benchmark`, but not a general source/scope contract for each cell.
- `/api/prices` returns confirmed-trade aggregates but does not label source kind, scope, or freshness explicitly.
- Watchlist events store useful payloads, but event responses do not expose canonical source metadata at the top level.
- SSE activity events publish order and crossing messages with no consistent provenance fields.

## Non-Goals

- Do not build the full Braemar/Bloomberg-style custom dashboard in this slice.
- Do not invent synthetic fair-value, physical stem, or indication values unless the backend has real inputs for them.
- Do not change database schema unless a reviewer finds it necessary.
- Do not change production behavior until staging has been reviewed visually and functionally.
- Do not add cosmetic tests that only assert icon or copy implementation details.

## Contract Terms

Add reusable enum-like schema values, preferably in a small shared schema module.

### Source Kind

- `CONFIRMED_TRADE`: value came from confirmed/delivered/paid trade records.
- `LIVE_ORDER`: value came from active executable orderbook orders.
- `DEMO_SEED`: value came from seeded demo organizations or seeded demo benchmarks.
- `BENCHMARK_REFERENCE`: value came from a configured benchmark/reference quote that is not demo seed.
- `MIXED_SOURCE`: value is blended across live and demo/reference sources and must be displayed with caution.
- `NO_DATA`: no value exists for the requested slice.
- `UNKNOWN`: legacy or insufficiently classified source.

Do not include `MODEL_INDICATION` in v1. It can be added only when a real model feed exists.

### Scope

- `DELIVERY_POINT`: exact product + delivery point + window.
- `REGION`: regional fallback or broad aggregation.
- `PRODUCT`: product-level fallback across delivery points.
- `UNKNOWN`: legacy/unclassified scope.

### Demo Status

Avoid boolean `is_demo` for aggregate data. A false boolean reads as "real" even when the aggregate is mixed or unknown.

- `REAL_ONLY`: all contributing source records are non-demo.
- `DEMO_ONLY`: all contributing source records are demo seed/demo activity.
- `MIXED`: contributing records include both real and demo sources.
- `UNKNOWN`: legacy or insufficient information.
- `NOT_APPLICABLE`: no contributing source records.

Event-grain payloads may keep append-only `is_demo` only when the event maps to exactly one classified source record. Aggregates must use `demo_status` plus counts.

Aggregate precedence:

- If no contributing records exist, use `NOT_APPLICABLE`.
- If any contributing record is unknown, use `demo_status=UNKNOWN` and `source_kind=UNKNOWN`, then expose the real/demo/unknown counts. Do not collapse unknown contributors into real, demo, or mixed.
- If no unknown contributors exist and both real and demo contributors exist, use `MIXED`.
- If only demo contributors exist, use `DEMO_ONLY`.
- If only real contributors exist, use `REAL_ONLY`.

### Freshness

Separate response generation time from source observation time.

- `generated_at`: when the API response was generated.
- `observed_at`: when the underlying source changed or was observed.
- Active orderbook data uses max contributing order `created_at` or `updated_at` if available.
- Confirmed trade data uses the latest contributing `confirmed_at`/`created_at` used by that endpoint.
- Persisted benchmark overrides use persisted benchmark update/creation time if available.
- Seed-matrix benchmarks must not use request time as `observed_at`; use `null` or a documented seed-version timestamp.

### Display Vocabulary

Frontend must not show raw enum names. Add a shared frontend formatter before any UI rollout.

Required initial labels:

- `LIVE_ORDER` + `REAL_ONLY`: "Live orderbook" / "Open user-posted liquidity for this product, port, and delivery window."
- `DEMO_SEED` + `DEMO_ONLY`: "Demo activity" / "Seeded demo liquidity used to preview platform behavior."
- `MIXED_SOURCE` + `MIXED`: "Mixed live/demo" / "This view blends user-posted and demo liquidity. Treat as directional only."
- `CONFIRMED_TRADE` + `REAL_ONLY`: "Confirmed trade" / "Confirmed transaction history from the platform."
- `BENCHMARK_REFERENCE`: "Reference benchmark" / "Reference price used for context, not executable liquidity."
- `NO_DATA`: "No market data" / "No usable market signal exists for this product, port, and delivery window yet."
- `UNKNOWN`: "Source pending" / "This older activity does not carry enough source information to classify safely."
- `NOT_APPLICABLE`: "Not applicable" / "No contributing market records are present for this view."
- `REGION`: display as "Regional view" when scope must be shown.
- `PRODUCT`: display as "Product-wide view" when scope must be shown.
- `DELIVERY_POINT`: display as the selected port name where possible, or "Port-specific view" as a fallback.

The formatter should accept source kind, demo status, scope, and freshness fields so Forward Curve, Trade Tape, Watchlist, and Activity Feed do not drift.

## Backend Work

### 1. Shared Market Activity Schemas

Create a shared schema module for source and scope values.

Candidate file:

- `app/schemas/market_activity.py`

Expected contents:

- `MarketSourceKind`
- `MarketScope`
- `MarketDemoStatus`
- `MarketDataProvenance`

`MarketDataProvenance` should include:

- `source_kind`
- `scope`
- `demo_status`
- `is_reference`
- `observed_at`
- `real_count`
- `demo_count`
- `unknown_count`
- `detail`

Keep this as Pydantic/API schema only unless a database use case emerges.

### 2. Price Summary Provenance

Extend `PriceSummary` to expose:

- `source_kind`
- `scope`
- `demo_status`
- `is_reference`
- `observed_at`
- `real_trade_count_24h`
- `demo_trade_count_24h`
- `unknown_trade_count_24h`

Rules:

- Existing `/api/prices` aggregates confirmed+ trades. Non-empty real-only summaries should be `CONFIRMED_TRADE`.
- Detect demo status using `app.services.demo_market.is_demo_market_organization`, aligned with `/api/trade-tape`.
- A trade is demo-only when both buyer and seller organizations are demo market organizations.
- A trade is real-only when neither buyer nor seller is a demo market organization.
- A trade with exactly one demo organization should count as `unknown_trade_count_24h` unless business rules later prove it can occur safely.
- If any bucket has unknown trades, use `source_kind=UNKNOWN`, `demo_status=UNKNOWN`, and keep source-specific counts visible. Do not collapse unknown trades into `REAL_ONLY`, `DEMO_ONLY`, or `MIXED`.
- All-demo buckets should use `source_kind=DEMO_SEED` and `demo_status=DEMO_ONLY`.
- Mixed buckets should use `source_kind=MIXED_SOURCE` and `demo_status=MIXED`.
- Real-only buckets should use `source_kind=CONFIRMED_TRADE` and `demo_status=REAL_ONLY`.
- `scope` is `DELIVERY_POINT` when the emitted row has a `delivery_point_id`. Do not infer `REGION` or `PRODUCT` from request filters unless the endpoint actually emits region/product-level aggregate rows.
- `observed_at` should match `last_trade_at`.
- Empty responses stay empty. Do not emit fake `NO_DATA` rows from `/api/prices` v1.

Tests:

- Real-only confirmed-trade summary includes `source_kind=CONFIRMED_TRADE`, `demo_status=REAL_ONLY`, and real trade count.
- Demo-only confirmed-trade summary includes `source_kind=DEMO_SEED`, `demo_status=DEMO_ONLY`, and demo trade count.
- Mixed confirmed-trade summary includes `source_kind=MIXED_SOURCE`, `demo_status=MIXED`, and both real/demo counts.
- `observed_at` matches the latest trade timestamp.
- Empty responses stay empty instead of emitting misleading `NO_DATA` pseudo rows.

### 3. Forward Curve Board Provenance

Extend `ForwardCurveBoardCell` with:

- `order_source_kind`
- `benchmark_source_kind`
- `scope`
- `demo_status`
- `real_order_count`
- `demo_order_count`
- `unknown_order_count`
- `real_best_bid`
- `real_best_ask`
- `demo_best_bid`
- `demo_best_ask`
- `best_bid_source_kind`
- `best_ask_source_kind`
- `order_observed_at`
- `benchmark_observed_at`

Rules:

- Separate real and demo depth before choosing provenance. Do not label a blended best bid/ask as purely live when a demo order supplies the visible best price.
- If a cell has only active real orders, `order_source_kind=LIVE_ORDER` and `demo_status=REAL_ONLY`.
- If a cell has only demo-seed orders, `order_source_kind=DEMO_SEED` and `demo_status=DEMO_ONLY`.
- If a cell has both real and demo orders, `order_source_kind=MIXED_SOURCE` and `demo_status=MIXED`.
- If a cell has no active orders, `order_source_kind=NO_DATA`.
- `best_bid_source_kind` and `best_ask_source_kind` classify the source of the displayed best price for that side.
- Existing `benchmark_source` maps to:
  - `DEMO_SEED` when `is_demo_benchmark` is true.
  - `BENCHMARK_REFERENCE` when present and not demo.
  - `NO_DATA` when absent.
- `scope` for board cells is `DELIVERY_POINT`.
- Preserve the existing behavior that hides demo benchmarks when real orders exist.
- Focus curve cells must carry the same provenance fields as matrix cells. Do not reconstruct focus buckets through `compute_forward_curve()` if that drops demo/real metadata.

Implementation note:

- `_aggregate_orderbook_window` should compute real/demo best bid, real/demo best ask, volume, counts, and max source timestamp in grouped SQL.
- Add a grouped focus-window aggregation for all focus windows rather than rebuilding provenance from `compute_forward_curve()`.
- `_build_board_cell` should receive enough bucket data to set the provenance fields without re-querying.
- Batch benchmark reads for all board and focus cells. Do not call `get_benchmark_quote` once per cell in the board path.
- Add a query-budget regression test or instrumentation-backed unit test showing the board does not perform per-cell benchmark queries.

Tests:

- Cell with real active orders only reports `LIVE_ORDER`, `demo_status=REAL_ONLY`, and real order count.
- Cell with only demo active orders reports `DEMO_SEED`, `demo_status=DEMO_ONLY`, and demo order count.
- Cell with mixed real/demo orders reports `MIXED_SOURCE`, `demo_status=MIXED`, separate real/demo best prices, and correct per-side source for displayed best bid/ask.
- Focus curve cells preserve the same mixed-source metadata as board matrix cells.
- Cell with no orders and seed benchmark reports `order_source_kind=NO_DATA`, `benchmark_source_kind=DEMO_SEED`.
- Cell with no benchmark reports `benchmark_source_kind=NO_DATA`.

### 4. Watchlist Event Provenance

Add top-level provenance to `WatchlistEventResponse`:

- `source_kind`
- `scope`
- `demo_status`
- `observed_at`
- `market_product_code`
- `delivery_point_id`
- `delivery_point_name`
- `availability_window_code`
- `order_id`

Rules:

- Preserve existing `event_payload` for backward compatibility.
- New event emissions should write provenance fields into `event_payload` at emission time where needed. Do not rely on response-time N+1 order lookups for demo detection.
- Legacy events that lack enough payload data should return `source_kind=UNKNOWN` and `demo_status=UNKNOWN`, while still exposing target fields that can be joined cheaply.
- `SLICE_NEW_ORDER` should classify from the referenced order organization at emission time.
- `SLICE_BEST_PRICE_MOVED` should classify from the source bucket if known; otherwise `UNKNOWN`.
- `SLICE_BENCHMARK_MOVED` should classify as `BENCHMARK_REFERENCE`, `DEMO_SEED`, or `UNKNOWN` based on the benchmark source.
- Pin order events should classify from the pinned order at emission time.
- `list_watchlist_events` should use one batched query joining target and delivery point fields. Mark-read can use one bounded target lookup.

Tests:

- `list_watchlist_events` response carries target slice fields at the top level.
- Mark-read response carries the same fields.
- New order-created events include payload-level source/demo fields and expose them at the top level.
- Benchmark-moved events classify benchmark source.
- Legacy events without provenance remain compatible and return `UNKNOWN` rather than lying.
- Existing payload shape remains unchanged.

### 5. SSE Activity Provenance

Add compatible fields to public activity payloads:

- `source_kind`
- `scope`
- `demo_status`
- `observed_at`
- `availability_window`
- `market_product`

Rules:

- Inventory the actual producers before coding. `publish_new_listing` exists, but live order creation currently publishes directly to `orderbook`; trade lifecycle events publish directly to `trades`.
- v1 must cover the actual producer paths for `/stream/orderbook` and `/stream/trades`. `/stream/activity` helper coverage alone is insufficient.
- Add fields append-only: same event names, same envelope, and same existing value types.
- For public unauthenticated event streams, prefer `source_kind` and `demo_status` over counterparty-revealing booleans. Do not expose organization-derived details beyond whether the data is demo/live/mixed.
- `order_created` should be `LIVE_ORDER`/`DEMO_SEED` based on order organization.
- `trade_created`/`trade_confirmed` should use the same trade demo classification as trade tape.
- `price_crossing` should classify from the contributing book bucket when available; otherwise `UNKNOWN`.

Tests:

- `publish_new_listing` includes the new fields and preserves existing keys.
- Demo organization orders are marked `DEMO_SEED`.
- Router-level orderbook event tests verify the real `order_created` producer emits append-only provenance fields.
- Router-level trade event tests verify the real trade producer emits append-only provenance fields.
- Existing SSE consumers would still see the same event names and existing payload keys.

### 6. OpenAPI and Documentation

Update:

- `ARCHITECTURE.md`
- `README.md` if endpoint response descriptions change there
- OpenAPI output used by frontend, after backend implementation is stable

Document the contract rules, including the confirmed-trade demo classification rule shared with trade tape.
Document `provenance_kind` as a legacy trade-tape field. New clients should prefer `source_kind`, `demo_status`, and `scope` once those fields exist. Keep `provenance_kind` append-only for compatibility until a separate deprecation is planned.

## Frontend Follow-Up Slice

Only after backend review passes:

- Regenerate `openapi.json`.
- Update TypeScript types.
- Add a shared frontend market-provenance formatter with the display vocabulary above. Do not render raw enum values.
- Render provenance labels/tooltips in Forward Curve board cells, Trade Tape, Watchlist activity, and any Market Intelligence usage.
- Use `demo_status=MIXED` to show "Mixed live/demo" instead of collapsing to either live or demo.
- Use `NO_DATA` to hide half-baked sections or show explicit empty states rather than stale placeholders.
- For empty `/api/prices` responses, use endpoint/page context to choose a clear empty state such as "No confirmed trades for this selection yet." Do not infer unsupported market slices from an empty price summary alone.
- Keep `MODEL_INDICATION`, fair-value, and physical stem copy out of the UI until real sources exist.
- Visually dogfood affected pages at desktop and constrained laptop viewports.

## Review Gates

### Plan Review

Use at least two adversarial reviewers:

- API/database reviewer: checks contract stability, query cost, schema compatibility, and no unnecessary migrations.
- Product/design reviewer: checks whether source/scope labels are understandable to non-traders and whether the frontend follow-up can be visually verified.

### Implementation Review

Use at least two adversarial reviewers:

- Backend reviewer: inspect code, tests, OpenAPI, performance, and compatibility.
- Frontend reviewer, if UI changes are included: inspect code and visually dogfood in a browser with screenshots or browser notes.

For any UI-facing work, reviewers must use the browser against the affected page and relevant viewport sizes. Code review alone does not pass the gate.

Minimum visual review scope for frontend implementation:

- Pages/surfaces: Forward Curve workspace, Marketplace Trade Tape, Watchlist/Market Radar activity if changed, and Market Intelligence if it consumes the contract.
- Viewports: `1440x900`, `1280x800`, and `1024x768`.
- Evidence: screenshot paths or browser notes identifying the selected product, port, window, and the provenance label seen.
- If mobile is intentionally gated, verify the mobile gate rather than the full workspace.

## Verification

Backend:

- Focused unit tests for touched schemas/services/routers.
- Full backend unit suite when the implementation is complete.
- OpenAPI smoke to verify new fields are present.

Frontend follow-up:

- `npm run test -- --run`
- `npm run build`
- `npm run i18n:check`
- Browser dogfood on staging or local preview for affected pages.

Deployment:

- Deploy to staging only.
- Do not promote to production until the user reviews the staging UI.

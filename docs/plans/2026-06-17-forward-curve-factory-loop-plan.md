# Forward Curve Factory Loop Plan — 2026-06-17

## Objective

Turn the current Forward Curve page from a broad demo board into a canonical market-monitoring surface:

- Backend-owned canonical slice contract keyed by `market_product + delivery_point_id + availability_window`.
- Public read models that aggregate approved products, approved ports, executable orderbook context, confirmed trade context, indications, fair bands, physical stems, and persisted benchmarks without leaking counterparties or raw feed IDs.
- Frontend table and drilldown UI that behaves like a monitoring workspace, not a trading screen.

This plan is staging-first. Production rollout is a later explicit decision after browser review.

## Factory Loop

1. Backend wave:
   - Add a shared slice service for table/slice/board projections.
   - Add `/curves/forward/table` and `/curves/forward/slice`.
   - Keep existing `/curves/forward/board` compatible while moving its identity and labels toward the shared contract.
2. Backend review gate:
   - Check redaction allowlist, window validation, empty valid slices, canonical product aggregation, approved delivery points, and no broad cross-product/window fanout.
3. Frontend wave:
   - Replace the pre-click global curve chart with latest-signal strip, matrix, and direct cell drilldown.
   - Render the drilldown as a selected-period evidence graph.
4. Frontend review gate:
   - Browser dogfood staging across common desktop viewports with console/network observations.
5. Staging deploy:
   - Deploy only staging until the user approves promotion.

## Non-Goals

- Do not change `MarketTerminal`; it remains the trading/execution surface.
- Do not add hit/lift/place-order actions to Forward Curve.
- Do not create new demo seed routes or production demo fixtures.
- Do not expose raw source IDs, organization IDs, emails, account names, vessel IDs, ingestion run IDs, raw model names, or raw source record/event IDs in public responses.
- Do not call synthetic benchmark fallbacks "market" data.

## Acceptance Checklist

- `GET /api/curves/forward/table` returns rows by approved product/port and columns by canonical windows.
- `GET /api/curves/forward/slice` returns one exact product/port/window context with bounded depth, trades, indications, fair band, benchmark, and physical stems.
- Invalid windows return 422; valid empty windows return no-data cells.
- Canonical products aggregate all active `Product` IDs under the same `market_product`.
- Delivery points are limited to Dalian, Busan, Shanghai, Singapore, Rotterdam, Houston, Los Angeles, and Santos.
- Primary mark hierarchy is explicit: confirmed trade, executable two-sided midpoint, indication, fair band, benchmark, no data.
- One-sided orderbook data never produces a midpoint.
- Demo/source/status/staleness fields are present and policy-labelled.
- UI opens populated cells directly into drilldown without a separate Expand button.
- The selected-period graph clearly distinguishes historical trades, executable orders, indications, fair band, benchmark, and aggregate stems.
- Browser dogfood confirms no viewport clipping, no TradingView branding, no stale old layout, and no console/network errors on staging.

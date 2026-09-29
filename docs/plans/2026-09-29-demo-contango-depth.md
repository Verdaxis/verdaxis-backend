# Demo Contango and Depth Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Give each demo fuel, port and delivery window ten bids and ten asks, with an illustrative rising forward curve across the full five-year horizon.

**Architecture:** Reuse the existing price helper, rolling coverage reconciliation and public curve read model. Price every demo window from its delivery dates with one valuation date; let managed demo coverage supply the resting book. Preserve real orders, historic trades, immutable ownership and customer references.

**Tech Stack:** Python, Decimal, SQLAlchemy, existing pytest suites; no new dependencies or database schema.

## Production hold

The user is running a live demo. Prepare code, tests and a pull request only. Do not merge to prod, deploy, run a live deploy dry run, or refresh production data until the user explicitly releases the hold. Thirty minutes passing is not approval.

## 1. Shared pricing and complete depth

Files: `app/seeds/market_seed.py`, `app/services/demo_activity.py`, `tests/unit/test_market_seed.py`, `tests/unit/test_demo_activity_windows.py`.

- Replace the six-quarter pricing lookup with canonical delivery-date arithmetic. Keep Spot bid and ask anchors unchanged.
- Use a documented 3% simple annual premium on the Spot midpoint: `premium = spot_midpoint * Decimal("0.03") * delivery_days / Decimal(365)`.
- Delivery time is the midpoint from the later of window start or valuation date to exclusive window end. This keeps the current monthly window above Spot even on its last day.
- Translate both ladders by the same premium. Keep ten distinct, positive, uncrossed levels per side in every canonical slice.
- Retain deterministic coverage keys and immutable fields. Refresh only the existing mutable-field allowlist.
- Generate only the existing synthetic matched pair per activity tick, at that window's current midpoint. Coverage supplies resting quotes; remove the redundant third visible order.
- Expire only unfilled legacy generated activity quotes with exact demo organization/provenance markers and no customer actor, support authority, idempotency marker, or trade/match/negotiation reference. Keep rows and watchlist pins.
- Verify full coverage at month, quarter and year rollover, refresh idempotence, expiry scope, references and matched-pair isolation.

## 2. Display the current managed demo curve

Files: `app/services/forward_curve_market_slices.py`, `tests/unit/test_forward_curve_market_slices.py`.

- Prefer a two-sided managed demo book midpoint over a historical demo print only when all selected demo orders have `idempotency_operation == "DEMO_COVERAGE"`.
- Keep real/mixed/unknown behavior and all raw history unchanged.
- Test table/slice consistency, old demo outliers, real-trade precedence and unmanaged/one-sided exclusions.

## 3. Verify and prepare review

- Run focused unit tests with explicit test settings and SQLite; no production credentials.
- Measure full 21,120–23,040-row coverage reconciliation against the existing 512 MiB / 600-second job limits in a disposable database.
- Inspect Bio Ethanol/Busan, B30 and B100 in a local browser using responses from the changed backend. Verify full horizon and ten depth levels per side.
- Run remote CI and independent review after source freeze. Keep the pull request unmerged while the production hold remains.

## Pricing evidence and limits

[CME](https://www.cmegroup.com/education/courses/introduction-to-base-metals/what-is-contango-and-backwardation) describes contango and carrying costs. [ICE](https://www.ice.com/insights/exchanges/why-financial-participants-matter-to-the-commodity-markets/exposing-speculation-myths) explains the relation between Spot, carrying costs and delivery time. Neither establishes an actual five-year biofuel forward rate. The 3% rate is an illustrative demo convention (about 15% at five years), not a forecast or an observed forward quote. Existing dated Spot reference prices and demo provenance remain in place.

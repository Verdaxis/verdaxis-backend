# Endpoint Work Reduction Implementation Plan

**Goal:** Reduce the measured forward-curve and map work without changing market eligibility, prices, provenance, depth, or response contracts.

**Architecture:** Keep FastAPI, SQLAlchemy, PostgreSQL, and the existing frontend read cache. Optimize the measured query shapes and response construction, then remove unnecessary frontend requests. Preserve separate production and staging catalog contracts; do not deploy or change live database settings in this task.

**Tech Stack:** Python 3.12, SQLAlchemy 2, PostgreSQL 17, Pydantic, React 19, TypeScript, Vitest.

## 1. Forward table and slice service

Files: `app/services/forward_curve_market_slices.py`, `app/services/forward_monitoring.py`, a small shared signal-key predicate helper if both callers need it, and focused unit/PostgreSQL tests.

- Trace all callers before changing shared helpers.
- Add a focused behavior test proving rectangular and sparse requested slice sets select exactly the requested tuples, including duplicates and empty sets.
- For a complete Cartesian grid, represent the predicate with product/port/window IN clauses; retain exact tuple semantics for sparse keys. Use one shared implementation where useful.
- Remove eager fallback-model construction and redundant full-model dump/validation in the table path where a clear shared pricing/provenance implementation permits it.
- Preserve exact response models and latest-signal selection. Do not bypass Pydantic validation or remove eligibility checks for speed.
- Check further repeated slice lookups only when equivalence and measured benefit are clear.
- Run existing curve/monitoring/eligibility tests and focused new tests.

## 2. Map query work

Files: `app/routers/orderbook.py` and focused map/orderbook unit and PostgreSQL tests.

- Trace aggregate and recent-ask callers, including combined filters.
- Test mixed REAL/DEMO, invalid/expired/off-spec/private rows, multiple windows, empty ports, and equal-timestamp ordering.
- Compare simpler recent-ask selection and aggregate query shapes on a disposable PostgreSQL fixture matching current demo depth.
- Keep the current shared API fields, all-window rollups, product totals, exact sorting, and deterministic newest-ask tie-break.
- Prefer a measured query change over new caching, indexes, schema, or live ANALYZE changes.

## 3. Frontend request work

Files in frontend worktree: `src/components/ForwardCurveWorkspace.tsx`, `src/components/IntelligencePanel.tsx`, and focused rendered tests; shared read-cache code only if a demonstrated overlap issue warrants it.

- Test that hidden forward-reference UI makes no curve batch and that opening the relevant tab loads it.
- Start a saved valid selected slice without waiting for the table where safe. Preserve stale-result protection, invalid-selection recovery, and forced-refresh semantics.
- Stop periodic work while the document is hidden and resume correctly. Avoid a duplicate slice request when the same table response establishes a selection already loading.
- Keep the 30-second visible refresh behavior and existing cache scoping. Do not change screen design, user data, freshness promises, or API payload contracts.
- Batch the map ticker's multiple selected products through the existing all-products SPOT endpoint; prove equality with individual product reads and retain exact client product/port/window matching.
- Guard the shared NewsFeed against late success/error responses across panel close/reopen and category changes exposed by the new visibility gate.
- Verify navigation, rapid selection changes, visibility changes, manual refresh, errors, and React effect cleanup.

## 4. Verification and integration

- Use isolated worktrees; table and map writers own disjoint source/test paths. Freeze edits before full-suite acceptance.
- Use only a capped disposable PostgreSQL instance for mutating tests and performance comparisons, with synthetic fixtures rather than customer data.
- Compare baseline and candidate outputs under a fixed clock, including 10 bids/10 asks per slice and REAL precedence; time both revisions on the same fixture with several interleaved samples.
- Run complete backend unit/PostgreSQL checks and frontend verify commands required by each repository. Review performance-sensitive changes independently for spec and code quality.
- Port the reviewed changes into dedicated branches based on current staging, preserving staging's distinct FAME catalog. Run relevant checks there.
- Prepare concrete PRs and a report with measured gains, limits, test evidence, and remaining work. No deployment or live database mutation is included.

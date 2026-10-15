# Task 10b — canonical curve and board projections

Base: `439399c`, lane `feat/rust-phase-1-lane-10b`. Local disposable services only; no push or live access.

## Test-first checkpoint

- Read the required plan, shared-relation review, architecture and integration notes.
- Added the half-even midpoint test. Initial Cargo run failed because the new projection module did not exist (`work/rust-phase1/task10b-red-unit.txt`). Implemented the pure helper; **1 test passed**.
- The plan's `--rust-target` argument is no longer supported. Used `venv/bin/python scripts/rust_contract/run_disposable.py --group curves_canonical --seed-mode corpus` instead. The populated gate failed with **18 differences** across the six existing cases because the route module is empty (`work/rust-phase1/task10b-red-corpus.txt`).
- New modules are registered from the task-owned route module with explicit paths. Root module registration and Cargo manifests remain integration-owned.

## Implementation checkpoint

- Implemented R09–R11 with separate canonical and older-board projections. All six existing corpus cases match exact bytes. Another **107 HTTP probes matched** (all approved lanes, three windows, duplicate windows, invalid enum/window, unknown point and board fallback).
- Added a metadata-only UNKNOWN relation using the existing current-order SQL fragment. No existing shared SQL changed. Economic inputs use Candidates and the shared binder; immutable trades use the shared trade relation. No typed benchmark-only qualification was added.
- Added shared signal loaders, external/manual benchmark reads, and projection helpers. Signal fields retain stored NUMERIC text. Curve policy JSON holds source-derived seed quotes, labels, enum order and board defaults; executable Rust contains no product identity literals. No synthetic price was added for products without Python seed quotes.
- New modules use task-local path registration. The contract target and oracle probe are task-owned test additions; Cargo manifests and shared harness code are unchanged.
- Six projection tests pass. Workspace: **107 passed**. Clippy passed. Python contract suite: **95 passed**. Targeted Python curves/monitoring/horizon/ingestion: **115 passed**. PostgreSQL suite passed (count to be recorded in final check).
- Logs: `work/rust-phase1/task10b-{probes-green,workspace,clippy-final,python-contract,python-curves,postgres}.txt`.

## Resume point

The implementation is not accepted yet. Final ranked-query corpus and PostgreSQL reruns are in progress. Finish query-count proof and golden publication before claiming full acceptance.

## Acceptance repairs

- Explicit UNKNOWN metadata and privacy assertions now run on corpus rows. Separate `curve10b:` IDs add 12 price levels (10 returned), REAL/DEMO/UNKNOWN signals and 12 REAL stems (6 returned). REAL is older than DEMO, so the probe proves evidence selection precedes focus limits. Stored signal scale and fair-band prices are checked.
- UTC at `2026-10-15T15:59:59Z` and `16:00:00Z`: six corpus cases plus 107 probes passed in each run. Singapore before midnight: same result. Singapore at midnight: six cases plus 108 probes passed, including the 10-level fixture. One intermediate Singapore run failed while an import was being added; the rerun passed. Logs use `task10b-{utc-before,utc-after,sg-before,sg-after-final}.txt`.
- Signed-zero regression failed (`task10b-signed-zero-red.txt`), then passed after preserving the negative sign when cent rounding reaches zero.
- Signal queries now rank obsolete observations in PostgreSQL. Complete grids use compact axes; sparse keys use exact tuple existence. A pure test rejects sparse cross-product expansion.
- Query parsing ignores unknown names, including a caller-supplied `shape`. The route owns its projection type.
- Latest offline workspace: **108 passed**, clippy passed. Final PostgreSQL/Python reruns and ranked-query HTTP comparison remain pending.

## Architecture integration note

The integration owner should add these task-owned shared modules to the architecture map: `curve_projection`/`curve_cells` (mark policy), `curve_unknown_metadata` (count-only relation), `signals`/`curve_keys` (trusted ranked evidence and exact key selection), and `benchmarks` plus source-derived `curve_policy.json`. They are path-registered under `routes::curves_canonical` to avoid changing shared module registration in this lane. No application Python, existing shared SQL, deployment file, root module or manifest changed.

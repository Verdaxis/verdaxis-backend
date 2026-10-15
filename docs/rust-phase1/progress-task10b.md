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

See the repair checkpoint below for current evidence. Golden publication remains integration-owned; this lane must not re-record it.

## Acceptance repairs

- Explicit UNKNOWN metadata and privacy assertions now run on corpus rows. Separate `curve10b:` IDs add 12 price levels (10 returned), REAL/DEMO/UNKNOWN signals and 12 REAL stems (6 returned). REAL is older than DEMO, so the probe proves evidence selection precedes focus limits. Stored signal scale and fair-band prices are checked.
- UTC at `2026-10-15T15:59:59Z` and `16:00:00Z`: six corpus cases plus 107 probes passed in each run. Singapore before midnight: same result. Singapore at midnight: six cases plus 108 probes passed, including the 10-level fixture. One intermediate Singapore run failed while an import was being added; the rerun passed. Logs use `task10b-{utc-before,utc-after,sg-before,sg-after-final}.txt`.
- Signed-zero regression failed (`task10b-signed-zero-red.txt`), then passed after preserving the negative sign when cent rounding reaches zero.
- Signal queries now rank obsolete observations in PostgreSQL. Complete grids use compact axes; sparse keys use exact tuple existence. A pure test rejects sparse cross-product expansion.
- Query parsing ignores unknown names, including a caller-supplied `shape`. The route owns its projection type.
- Final offline workspace: **108 passed**, clippy and fmt passed. PostgreSQL runner: **116 passed**, two matrix tests filtered for their dedicated runner. Python contract tests: **95 passed**. Targeted Python curve tests: **115 passed**. Ranked-query HTTP gate: **6/6 corpus cases and 112/112 probes matched** (`task10b-ranked-final.txt`). The oracle validates source-derived benchmark values and enum order as well as response parity.
- Checks were repeated after the code checkpoint with the required full HEAD release SHA. No live service was contacted. Disposable runners removed their own containers and volumes.

## Original acceptance limits (superseded by the repair checkpoint below)

- The six accepted golden cases were not replaced. Extra route cases run in the task-local oracle (112 probes), not `cases.json` or a new golden recording.
- No measured SQL query-count test was added. Source has eight data statements for a table and nine for a slice/board, independent of cell count; this is source inspection, not a measured query-count proof.
- Focus projections are bounded before serialization. The trade tail now ranks REAL first and limits each slice to eight rows in SQL; its final corpus rerun passed six cases plus 112 probes (`task10b-bounded-trades.txt`). Signal histories rank in SQL; selected focus stems are bounded in Rust.
- Advisory KYC mode also passed **6/6 corpus cases and 112/112 probes** (`task10b-kyc-off.txt`).
- The source-derived curve policy has oracle-SHA equality and Python value comparisons in the oracle test, but no separate digest manifest. The task does not change the integration-owned catalog export.
- These limits mean Task 10b is **implemented but not fully accepted against every plan gate**. Do not mark query-count or expanded-golden acceptance complete. Resume with those tests; retain the exact comparator and privacy checks. Final code checks: fmt/clippy passed, workspace 108, PostgreSQL 116, Python contract 95, Python curves 115. Logs use `task10b-last-*`.

## Acceptance repair checkpoint — 2026-10-16

- **M1 fixed:** board focus compares parsed UUID values. Uppercase, compact,
  braced and URN spellings pass; Python rejects wrapped compact and malformed
  spellings. The original implementation failed the new uppercase test.
- **M2 fixed:** latest signals sort stable timestamp keys, including microseconds.
  The original implementation failed the same-second regression. The publication
  fixture uses two-sided books so both signals have actual midpoint marks.
- **M3 fixed:** SQL groups counts, remaining volume, best prices, observed times
  and managed-demo counts over `eligible_orders`. Depth groups before REAL-first
  class selection and ten levels per side. Tables retain no raw order rows;
  boards do not read unused trade evidence. UNKNOWN stays count-only.
- **N1 fixed:** invalid board windows return Python's unhandled plain-text 500,
  through the existing middleware marker. Its new test failed before the repair.
- **N2 lane work complete:** measured SQLx statement/row tests and **125 cases**
  are committed in `cases.json`. Shared attested fixtures include 10,000
  duplicate-price orders, 24 depth orders, four subsecond orders and 42 signals.
  Golden files were **not** recorded or changed. Task 7b and the PM own the
  coordinated recordings after integration (decision 23).
- Red evidence: `task10b-repair-red.txt`, `task10b-board-error-red.txt`, and
  `task10b-aggregate-red-final.txt`. The measured red table returned **10,012
  rows in eight data statements**; the row-bound assertion failed as intended.
  Two earlier instrumentation attempts captured no SQLx events; those were
  harness failures, not aggregation evidence.
- Large-corpus runs exposed expensive admission planning. The repair scopes
  `jit=off` to a read transaction, as Python does for canonical aggregation.
  Exact tuple membership replaces the correlated wildcard-key scan; single
  slices also bind the shared scalar filters. No predicate or raw-row cap was
  added. Two intermediate runs timed out before these query-cost repairs.
- Required checks at code commit `e786858b`: fmt and clippy passed; offline
  workspace **111 passed**; PostgreSQL **119 passed** (two dedicated matrix
  tests filtered by the existing runner); Python contract tests **96 passed**.
  Logs: `task10b-head-{workspace,postgres,python}.txt`.
  Final disposable HTTP runs passed **125/125 corpus cases and 112/112 probes
  in each KYC state**, plus the measured workload and explicit subsecond check.
  Logs: `task10b-final-corpus{,-disabled}.txt`; no golden recording was run.
- Measured enabled-state requests at the code checkpoint:

  | Request | Data statements | Returned data rows |
  | --- | ---: | ---: |
  | 10,000-order single table cell | 8 | 13 |
  | Same slice with depth | 10 | 14 |
  | Full default matrix | 8 | 24 |
  | Board with populated focus | 9 | 29 |

  SQLx additionally recorded two control statements (`SET LOCAL`, `COMMIT`)
  per successful request. SQLx does not emit a query event for protocol BEGIN
  or ping; these are not claimed as measured data statements. Counts include
  both catalog reads (12 rows) and are asserted for each measured request.
  Eight simultaneous large-cell requests all returned 200. Sampled process RSS
  was **15,376 KiB before, 17,264 KiB peak, 17,264 KiB after** (127 samples).
  This is a local debug-build observation, not a production memory guarantee.
  Evidence: `task10b-query-cost-enabled.json` and `task10b-final-corpus.txt`.
  Advisory KYC returned the same statement/row counts; its eight-request RSS
  was **19,536 KiB before, 21,184 KiB peak, 16,896 KiB after** (116 samples).
  Both are sampled debug-process observations, not allocator or host totals.
- Integration resume: merge the lane, retain the shared fixture hook and all
  125 curve cases, then publish the whole golden once on the build branch.
  The added orders are outside the default curve horizon but can change other
  public order/legacy aggregates; recording only the curve group is insufficient.
  Do not treat the old golden as current after merging this fixture addition.
- Source and architecture are updated. No application Python, deployment,
  schema, ACL, or golden file changed. No push or live access occurred.

## Commits

- `43cd35c9`: M1/M2 failing regressions and parsed UUID/timestamp fixes.
- `433b4714`: N1 unhandled board-window response.
- `53613f61`: SQL aggregation, bounded depth, exact-key pruning and scoped JIT.
- `e786858b`: measured costs, shared fixtures and publication-ready cases.
- `42334cb1`: original test-first midpoint and red corpus checkpoint.
- `19512eab`: canonical and older-board implementation.
- `70bc7f0a`: boundary fixtures, trusted ranking, sparse keys and signed-zero repair.
- Final checkpoint: SQL trade bound and these verification notes. No push.

## Architecture integration note

The architecture map now lists these task-owned shared modules: `curve_projection`/`curve_cells` (mark policy), `curve_unknown_metadata` (count-only relation), `signals`/`curve_keys` (trusted ranked evidence and exact key selection), and `benchmarks` plus source-derived `curve_policy.json`. They are path-registered under `routes::curves_canonical` to avoid changing shared module registration in this lane. The original implementation changed no application Python, existing shared SQL, deployment file, root module or manifest. The repair checkpoint adds test-only instrumentation and shared corpus fixture wiring; application Python and deployment remain unchanged.

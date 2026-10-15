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

The implementation is not accepted yet. Finish explicit UNKNOWN fixture assertions, time-zone/midnight matrix, signed-zero tests, query-count/trusted-evidence coverage and final checks. The time-zone matrix is running in disposable containers. Expanded golden publication and sparse-grid optimization still require review.

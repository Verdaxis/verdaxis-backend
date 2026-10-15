# Task 10b — canonical curve and board projections

Base: `439399c`, lane `feat/rust-phase-1-lane-10b`. Local disposable services only; no push or live access.

## Test-first checkpoint

- Read the required plan, shared-relation review, architecture and integration notes.
- Added the half-even midpoint test. Initial Cargo run failed because the new projection module did not exist (`work/rust-phase1/task10b-red-unit.txt`). Implemented the pure helper; **1 test passed**.
- The plan's `--rust-target` argument is no longer supported. Used `venv/bin/python scripts/rust_contract/run_disposable.py --group curves_canonical --seed-mode corpus` instead. The populated gate failed with **18 differences** across the six existing cases because the route module is empty (`work/rust-phase1/task10b-red-corpus.txt`).
- New modules are registered from the task-owned route module with explicit paths. Root module registration and Cargo manifests remain integration-owned.

## Resume point

Implement the named UNKNOWN metadata relation, canonical projections, signal loaders and separate legacy board projection. Preserve current shallow curve admission; do not add typed benchmark qualification. Keep stored numeric text for signal fields. Required final checks and expanded corpus coverage are **not yet run**.

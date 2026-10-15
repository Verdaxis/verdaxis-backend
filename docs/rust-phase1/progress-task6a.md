# Rust Phase 1 — Task 6a

## Scope and resume point

Lane: `feat/rust-phase-1-lane-6a`, base `f1d53a9`. Read-only live-slice
benchmarks only. No live host, push, rebuild, persistence, or application Python
change. Final acceptance waits for Task 4b's populated corpus (decision 15).

Implementation is in `rs/read/src/market/live_benchmarks.rs` and
`rs/read/sql/live_benchmarks.sql`. It composes the shared Candidates relation,
then applies positive remaining quantity, off-spec, typed qualification using
the captured Singapore date, and scalar ASK declarations before aggregation.
The key remains side/product/point/window: REAL and DEMO are intentionally
blended. The request-local cache also records missing keys.

## Wiring deviations

The PM approved minimum wiring after the initial stop:

- Add one module registration in `rs/read/src/market/mod.rs`.
- Add `live_benchmarks` to the disposable runner's group map.
- Follow Task 8: the HTTP group target is
  `rs/contract/tests/live_benchmarks.rs`, gated by `contract-http`; offline
  tests remain in `rs/read/tests/live_benchmarks.rs`. No manifest change.
- Replace the nonexistent `--rust-target` checkpoint flag with
  `venv/bin/python scripts/rust_contract/run_disposable.py --group live_benchmarks --seed-mode ordinary`.
- The minimum test-only projection needs dispatch in
  `tests/rust_contract/server.py` and `rs/read/examples/contract_server.rs`.
  Implementations live in task-local `tests/rust_contract/live_benchmarks.py`
  and `rs/read/tests/support/live_benchmarks_http.rs`. No serving route added.
- Add the group to the inventory meta-test's explicit known-group set.
- Add `&Config` to the planned helper signature. The current shared binder
  requires it for the runtime KYC switch; defaulting the setting would be wrong.
  Consumers call `live_benchmarks(tx, policy, config, keys, now)`.
- Architecture integration is described here rather than editing the shared
  `ARCHITECTURE.md` outside this lane's scope. The PM can fold this paragraph
  into that file when merging.

## Test development

- Arithmetic tests first failed to compile because the helper module was absent.
- The adapted HTTP group failed with 12 differences before Rust's projection
  was mounted (`work/rust-phase1/task6a-red.txt`).
- An unquoted SQL `window` record field caused the first implementation's
  data-bearing calls to fail. Quoted it; kept failure evidence.
- A correlated sparse-key EXISTS timed out under the ordinary seeded book.
  The task-local tail now materializes distinct requested tuples and joins them;
  it does not change shared SQL or broaden sparse selections.
- Offline regressions cover weighted REAL/DEMO arithmetic, two-place half-even
  rounding, zero/negative quantity, off-spec, scalar declaration requirements,
  custom certification schemes, invalid FAME/LNG declarations and Singapore
  midnight.
- The migrated fixture probe reuses Task 6's fixture builder without editing it.
  Current migrations refuse OPEN zero-remaining rows; the probe asserts that
  refusal, and the pure helper test covers rejection of a historical zero row.
- The HTTP projection tests missing-key caching by aborting its transaction
  after the first load. A second load must succeed without any database query.

## Checks

Verification is in progress. Logs are under `work/rust-phase1/task6a-*`.
Do not mark acceptance complete until the final entries below are recorded.

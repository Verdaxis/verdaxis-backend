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
  Materializing requested tuples alone still timed out. The task-local tail
  now also materializes the shared candidate projection before the key join;
  it does not change shared SQL or broaden sparse selections. The four ordinary
  contract cases then passed with exact bytes (`task6a-green5.txt`). That run's
  additional fixture probe failed its explicit database-name attestation; the
  probe now forwards `SEED_TARGET_DATABASE` to the existing oracle guard.
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

Logs are under `work/rust-phase1/task6a-*`. Every Cargo command exports the
current full HEAD as `RELEASE_SHA`; PostgreSQL uses the absolute harness Python.

- Arithmetic/qualification: 7 passed.
- Offline workspace: 81 passed.
- Runtime PostgreSQL: 88 passed; two market-matrix tests remain explicitly
  filtered by that runner and belong to the separate Task 6 runner.
- Python contract suite: 42 passed, five existing warnings.
- Named Python sources (`test_public_owner_admission`,
  `test_market_evidence_policy`, `test_orderbook_live_benchmarks`): 34 passed.
- Formatting, workspace/all-target clippy, all-features clippy, assertion ledger
  and `git diff --check`: passed.
- Final ordinary helper group (`task6a-green8.txt`): four cases matched exact
  bytes, plus three migrated matrix cases matched; one Rust contract test passed.
  The matrix proves both typed-invalid rows are shallow-visible but benchmark-
  ineligible, mixed REAL/DEMO prices, sparse selection and captured as-of expiry.
  Python asserts one SELECT per nonempty batch; Rust has one fetch in the shared
  helper. The aborted-transaction replay proves cached hits and misses do not
  issue another query.
- One ordinary seed attempt timed out before any helper test; no seed code or
  database timeout was changed to hide that failure.
- The fixture probe initially attempted to move existing orders to an isolated
  slice. The database correctly refused the immutable slice update. Removed
  that update; the fixture retains mixed prices/quantities in its original slice.
- The migrated as-of comparison exposed the test wrapper's frozen clock
  overriding explicit SQL/KYC instants. The helper-only dispatch now closes
  the outer clock stack, binds the complete existing clock manifest for the
  request, then restores the corpus clock. No application clock changed.
- No golden bytes were relabelled under the older oracle SHA. Task 4b owns the
  populated recording and policy refresh; this group compares fresh paired
  Python/Rust responses through the same comparator in the meantime.

Code commits: `372a5aa` (helper/offline tests), `96c8ff2` (test wiring/query
shape), `ce43c3b` (immutable fixture correction), `415f3ba` (complete test clock).
All requested local commands ran. Final acceptance and frozen golden recording
remain pending the populated Task 4b corpus. No Task 7 or curve implementation
was added. No existing shared SQL changed. New shared files are only
`market/live_benchmarks.rs` and `sql/live_benchmarks.sql`.

Resume: Tasks 7a/7b may consume the helper with the explicit runtime `&Config`,
the admitted transaction and the request's captured UTC instant. Keep each cache
within that transaction/instant. The test-only HTTP endpoints must never be
mounted by the serving binary. All containers started by these runners were
removed with their volumes on exit; other lanes' containers were not touched.

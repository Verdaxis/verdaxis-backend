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

## Independent review repair — 2026-10-16

M1 is fixed in `43b4f67`. `market/whitespace.rs` contains one explicit Python
Unicode whitespace predicate, including U+001C–U+001F. Scalar benchmark text
and shared certification normalization use it. SQL `btrim` is unchanged.
The complete Unicode inventory is checked; the four control characters are
covered for origin, specification, feedstock and scheme, with a custom-scheme
positive control. The exact two-row regression failed at price 150 instead of
100 before the fix and passes with price 100, quantity 10 and count 1 afterward.
Red/green evidence is in `work/rust-phase1/task6a-{m1,two-row}-*.txt`.

The expanded helper fixtures cover joined Singapore midnight, independent
sampling/test-date transitions, expiry at -1/0/+1 microsecond, and KYC grace at
those boundaries in both switch states. Pending, submitted, foreign-bound
review with grace and rejected-with-grace cases have explicit aggregate checks.
Stored `1E-29` and `0E-100` FAME values pass through Pydantic storage dumping,
PostgreSQL and both helpers. Typed BID, null-CI LNG, legal half-cent weighted
pairs, maximum arithmetic values, partial remaining quantities and Q/CAL windows
are covered. Cache checks add expansion, duplicate replay and new-cache isolation.
The helper-only gate/window controls are not mounted in the serving binary.

Wrong-lane LNG insertion is refused by the current database constraint. The
suite asserts that refusal and separately tests the pure helper exclusion. It
does not disable a guard or claim a historical wrong-lane SQL comparison.

During test development, setup failures exposed an Arc clone mistake, a fixture
bind-name parser that omitted digits, and an unescaped `+00:00` query offset.
A typed BID fixture also needed its required `neat_fame` declaration and
Pydantic-populated storage defaults. These were corrected in test code. Three
ordinary seed attempts timed out before helper tests; the unchanged command was
retried without changing seed behavior or timeouts. Running three full helper
queries in every comparison exceeded the HTTP case deadline; cache expansion is now one separate
case. No timeout or production query policy was relaxed.

### PM integration handoff

Task 4b has landed on the build branch, with accepted golden oracle SHA
`a127672` and `--seed-mode corpus` (PM-provided status). It was not merged into
this lane. After the PM merges both lanes, move the new whitespace, midnight,
expiry/KYC, stored-decimal, typed-BID/null-CI-LNG, arithmetic and Q/CAL/partial
fixture families and their explicit assertions into the central corpus ledger.
Keep pure arithmetic/whitespace tests and cache-abort instrumentation local.
Run the populated helper group with `--seed-mode corpus`; do not relabel the
existing golden bytes as if these new cases had already been accepted.

Review item 8 remains owned by Tasks 7a and 14: concurrent-writer snapshot
coherence, one captured instant through depth and benchmark queries, full public
DTO scale/null behavior, and sparse/large-book cost at the declared workload.
Task 7 adapters still own canonical key normalization. This lane adds no route
or deployment activation. Keep the runtime `&Config` argument and do not copy
benchmark-only typed qualification into curve economics.

### Repair verification

- `cargo fmt --all --check`: passed.
- Workspace/all-target/all-feature clippy with warnings denied: passed.
- `cargo test --workspace`: 86 passed (12 task-local pure tests).
- `RUST_TEST_PYTHON=$PWD/venv/bin/python bash rs/read/tests/run_postgres.sh`:
  93 passed; the same two Task 6 matrix tests remain explicitly filtered.
- `venv/bin/python -m pytest tests/rust_contract -q`: 42 passed, five existing
  deprecation warnings.
- Python source benchmark/owner-admission/evidence tests: 34 passed.
- Task-local Python Ruff checks and `git diff --check`: passed.

Logs: `work/rust-phase1/task6a-final-{offline5,postgres3,python5}.txt`.


**Remaining verification gate:** the required ordinary helper runner was run,
but the final three attempts (`task6a-acceptance-green5.txt` through `green7.txt`)
failed during `scripts/seed.py`, before any HTTP acceptance cases. PostgreSQL
reported `QueryCanceledError: canceling statement due to statement timeout` on
the ordinary order bulk insert. Shared Colima load was high; no timeout, seed
size, container allocation or other lane's container was changed. Earlier runs
passed the four existing ordinary cases but exposed test setup errors described
above. The corrected seven additional acceptance cases have **not** completed
a green end-to-end run. Do not mark them accepted from the 93-test runtime suite,
which does not execute this helper HTTP group.

Resume with the exact required ordinary command when local capacity is available,
then merge the corpus families through the PM and run the populated gate. Code
commits: `43b4f67` (M1), `d3028dd` (acceptance tests), `a9e58e4` (typed BID fixture).

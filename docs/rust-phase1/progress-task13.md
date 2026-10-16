# Task 13 — Exact routing and unchanged integration

Status: **in progress, not accepted**. Base: `c37f8074`, lane
`feat/rust-phase-1-lane-13`. No live access, push, agents or skills.

## Build-branch merge — Task 11 and unified golden merged

Merged `feat/rust-phase-1-20261015` (HEAD `4053303b`) into this lane. Task 11
(`78af98c4`) and the unified golden (`bef75fca`, oracle `78af98c4`, 758 cases)
are present; the old `9880b714` golden directory is gone. Only one conflict
appeared, in `scripts/rust_contract/run_disposable.py`: two port allocators.
Resolution keeps **one** shared helper. `ephemeral_port()` now reads the same
kernel range authority as the disposable target guards through
`ephemeral_range()` (Task 11's `5ed6326b` helper); the fixed 49152..65535
allocator from `d4c6be3b` was removed. `tests/rust_contract/test_runner_ports.py`
was rewritten to assert the shared authority rather than the removed range.

A HARNESS bug surfaced at that point: `run_disposable.py` imported
`tests.ephemeral_ports` at function scope, but the documented direct invocation
`python scripts/rust_contract/run_disposable.py` does not put the repo root on
`sys.path`, so every direct run failed with `ModuleNotFoundError: No module
named 'tests'`. Fixed by adding the repo root to `sys.path` at import time.
Commit `8988bf62`.

Podman image normalization was its own commit (`0b711cc6`): the extended
snapshot and lists oracles now call the shared
`scripts/rust_contract/image_identity.py::matches_pinned_image`, which accepts
only `IMAGE` or the same repository and digest under `docker.io/`. The digest
must still match exactly; four wrong-name variants are rejected by test.
Merge commit: `ed312543`.

### Merged-tree results (native amd64 via `verdaxis-prod-test`)

Every corpus group matched Python at full count:

| Group | Case count | Result |
| --- | ---: | --- |
| health | 8 | pass (HEAD/`Allow` repair holds) |
| echo | 2 | pass |
| simple_reads | 29 | pass |
| live_benchmarks | 4 | pass |
| orderbook_snapshot | 3 | pass |
| orderbook_lists | 12 | pass |
| orderbook_map | 9 | pass |
| prices | 40 | pass |
| curves_legacy | 4 | pass |
| curves_canonical | 125 | pass |
| mcp | 143 | pass |

The MCP "stall" was harness-only: this lane lacked the Task 11 merge, so its
older harness opened a `GET /mcp` event stream that never closed. The PM stopped
that orphan run. After the merge, MCP matches. `run_market_contract.sh`
(2 passed) and `run_postgres.sh` both pass on the merged tree.

### Merged-tree quality gates

Run on the host through `verdaxis-prod-test`:

- `cargo fmt --all -- --check`: **pass**.
- `cargo clippy --workspace --all-targets -- -D warnings`: **pass**.
- `cargo test --workspace`: **146 passed**, 0 failed.
- `venv/bin/python -m pytest tests/rust_contract -q`: **445 passed**, 5 warnings.
- `cargo sqlx --version`: the Cargo SQLx subcommand is not installed on the test
  host, so `cargo sqlx prepare --check` is **unrun**, not passed.

## Paired p95 gate after the projection repair — **FAIL**

240 requests, 30 per cell, Python and Rust interleaved in one run on the same
ANALYZEd disposable database. `require_latency_parity` rejects the result.

| Route | Mode | Python p50 / p95 s | Rust p50 / p95 s | Rust p95 vs Python |
| --- | --- | --- | --- | --- |
| list | cold process | 1.509 / 1.906 | 2.148 / 2.825 | +48% FAIL |
| list | warm | 1.315 / 1.804 | 2.196 / 2.491 | +38% FAIL |
| bids | cold process | 1.763 / 2.133 | 2.499 / 2.946 | +38% FAIL |
| bids | warm | 1.725 / 2.676 | 2.668 / 3.875 | +45% FAIL |

The repair cut the gap from ~4x to ~1.4x but does not reach the go/no-go bar.
Rust is still slower in every cell. The remaining gap is database cost, not
transport: the earlier plans showed Rust's page and benchmark statements running
well above Python's on the same rows. No index or schema change was made; a
possible index is a PM decision, not taken here.

### Remaining gap — named cause with merged-tree plans

Plans captured on the merged tree (`task13-merged-plans.log`, JSON in
`task13-merged-plans.json`). Database time per statement (node total):

| Route | Statement | Python ms | Rust ms |
| --- | --- | ---: | ---: |
| list | page (50 rows) | 133.8 | 229.6 |
| list | benchmark (10000 rows) | 86.7 | 388.9 |
| bids | count (1 row) | 107.3 | 211.4 |
| bids | page (20 rows) | 146.5 | 222.2 |
| bids | benchmark (10000 rows) | 85.8 | 506.6 |

Two causes are visible in the actual plans:

1. **Wide internal projection.** The Rust benchmark join node reports
   `Plan Width = 3289` over 10,047 rows, although only 100 bytes leave the
   query. `eligible_orders` projects `o.*` plus the digest JSON object; Python's
   equivalent selects a narrow column set. Rust materializes roughly 3.3 KB per
   candidate row that the consumer discards.
2. **Per-row admission tree.** Rust evaluates the shared policy/lane/KYC
   `EXISTS` predicates and the current-order rule for every scanned row, so the
   planner chooses a sequential scan of `orderbook_orders` (10051 rows, ~114 ms
   in the list benchmark) where Python uses an index scan.

A trial forcing `MATERIALIZED` on `policy_products`/`policy_points` measured
219/406 ms against 230/389 ms — inside noise, so it was **reverted** and not
kept.

The width comes from one construct: `private_digest_input` is built inside the
shared relation as `to_jsonb(o) || jsonb_build_object(...)`. PostgreSQL must
therefore materialise the whole order row plus a JSON object for every one of
the ~10,047 candidate rows, and only 100 bytes leave the query. Python builds
its digest input only for rows that survive, which is why its benchmark
statement is ~4x faster on the same data.

**This fix needs a PM decision.** `private_digest_input` is part of the frozen
Task 6 shared interface ("order rows include catalog labels and
`private_digest_input`"). Removing it from the shared relation and constructing
the digest in only the consumers that keep rows — after the page limit for
lists, after key/qualification selection for benchmarks — would change that
interface for Tasks 6a/7a/7b/7c consumers and their tests. The digest contents
themselves must not change. No change was made here.

## Projection-cost repair checkpoint — not accepted

Exact emitted SQL and timed actual plans for both routes are recorded side by
side in `task13-query-plans.md`. Native diagnostic page query times:
list Rust 1578 -> 388 ms, bids Rust 1773 -> 338 ms; benchmark Rust
3465 -> 729 ms (list) and 2629 -> 838 ms (bids). Python after times remain
212/256 ms for pages and 182/181 ms for benchmarks. These are individual plan
runs, **not** after p50/p95 measurements. No no-regression claim.

Changes: narrow materialized page keys, then lateral eligible detail lookup;
`eligible_orders AS NOT MATERIALIZED` prevents full private JSON materialization
when the relation is referenced twice. Benchmark projects only qualification
fields instead of the whole private row. All eligibility stays in shared SQL;
no index, schema or policy change. Existing red 240-request p95 gate remains.

Consumer runs after this structural change: health 8, echo 2, simple 29,
benchmarks 4, lists 12, map 9, prices 40, legacy curves 4, canonical curves 125
all matched and their full group targets passed. Snapshot's three corpus cases
matched, but its old Docker-image/log-count probe failed. Updated that probe to
exact pinned Podman identity and approved pg_stat_statements counts; rerun pending.
Offline fmt/clippy passed and workspace 128 passed.

**Remote execution blocker:** MCP is still an empty seam in this lane. Its group
run stalled after the other groups completed; the queued matrix/runtime commands
never ran. TaskStop ended the local batch but did not produce the wrapper's
remote-unit-stop confirmation. Two subsequent wrapper calls waited on the lane
lock; both were interrupted through their own wrapper TERM traps and confirmed
remote-unit stop (exit 130). No direct SSH or host setup change was attempted.
The original remote run/container cleanup cannot be confirmed from this session.
Operator must release/inspect that test-lane run before further database tests.
Do not treat MCP, matrix, runtime or snapshot rerun as passed.

**Resume:** first restore the approved test lane and clean only this run's
resources; then rerun snapshot, matrix and runtime gates. Next remove remaining
policy-subplan cost if needed, rerunning all consumers after each structural
change. Finally run the 240-request paired gate and fill the after-p95 table.
The required after table is not available yet. Colima remained off.

## Paired latency baseline — 30 samples per cell

The new `scripts/rust_contract/measure_lists.py` uses the existing Runner and
one populated, ANALYZEd disposable database. It alternates runtime order and
measures identical default list/bid requests. Cold means fresh service process,
connections and prepared statements, **not cold OS/database buffers**; host
cache manipulation is prohibited. Warm has one warmup request. All 240 requests
completed with HTTP 200. Native amd64 via `verdaxis-prod-test`, no local Docker.

| Route | Mode | Python p50 / p95 seconds | Rust p50 / p95 seconds |
| --- | --- | --- | --- |
| list | cold process | 1.323 / 1.784 | 5.554 / 6.541 |
| list | warm | 1.511 / 1.862 | 7.245 / 8.512 |
| bids | cold process | 1.690 / 3.502 | 6.759 / 10.810 |
| bids | warm | 1.487 / 2.730 | 6.432 / 11.632 |

**FAIL: Rust p95 exceeds Python in every cell.** The earlier ten-second spot
checks were not sufficient acceptance. The new `require_latency_parity` gate
rejects this measured baseline and requires all eight cells with >=30 samples.
Its two offline tests passed after an initial missing-function collection failure.
No latency fix or no-regression claim is made at this checkpoint.

`pg_stat_statements` runs only in the disposable container, with its extension
created by the test administrator. Per-request reset captures calls, rows,
execution time, buffer hits/reads and JIT counters. Background Python outbox
polls also appear and are distinguishable by query text; they are not counted
as route queries. Representative warm list: Python list/benchmark 135.8/187.7 ms;
Rust 2112.2/2391.4 ms. Each issues one list and one benchmark query; both return
50 list rows and 10,000 benchmark rows. JIT functions=0. Sequential requests
have no admission competition; unaccounted wall time is not claimed to be
admission wait without further instrumentation.

`explain_lists.py` records actual executed plans using auto_explain ANALYZE,
BUFFERS and JSON inside the disposable container (no host logging changes).
Native plans show similar shared buffer hits (Python list 30,529, benchmark
30,280; Rust list 30,526, benchmark 30,473). Python benchmark uses a selective
index scan returning 10,000 rows; Rust walks 10,047 eligible candidates and
repeats policy recordset/point/lane subplans. Rust list also projects private
JSON for 10,045 rows beneath its sort/limit. Next investigation: defer that
projection and use a narrow benchmark qualification input without copying
eligibility or changing typed qualification. Capture explicit EXPLAIN command
and admission timings if needed; auto_explain captures the actual bound plans.

Evidence (ignored, retained locally): `task13-list-measurement-run.log`,
`task13-list-baseline.json`, `task13-native-plans.log`, `task13-native-plans.json`.
The full measurement JSON includes all samples and statement statistics.

Query-count blocker resolved: the ASK-page probe now resets and reads scoped
`pg_stat_statements`, and observes **exactly one benchmark statement**
(`task13-stat-count.log`). This replaces unavailable Podman log reads without
relaxing the count. Full list target still needs rerun after the latency repair.

**Resume here:** implement and verify the query-cost repair against the red p95
gate, then rerun all 240 paired requests and full differential targets. OpenAPI,
method/slash corpus and publication remain lower priority and unfinished.
All measurement and diagnostic runs finished and their containers were removed.

## Decision 25 implemented — eligibility sets; p95 improved, gate still FAIL

PM decision 25 approved replacing the per-row correlated `EXISTS` inside
`CASE`/`OR` with sets computed once per statement. Implemented, code commit
`07622f5b`.

One definition: `rs/read/sql/eligibility_sets.sql` holds `eligible_owners`
keyed `(user_id, organization_id)` and `eligible_assisted` keyed
`(owner_user_id, actor_user_id, organization_id)`, both `MATERIALIZED` and both
using the same captured `$2` for grace expiry.
`market::visibility::eligibility_sets_sql(gate)` renders it for a consumer's gate
bind, and `expand_shared_markers` substitutes `/* eligibility_sets */` in every
relation that composes `current_order_rule.sql` — the public relation, the legacy
curve relation (`$13`) and the curve-unknown metadata relation (`$5`). No
consumer copies the sets. `current_order_rule.sql` now tests membership and
keeps the `CASE` on `creation_method` and every NULL semantic.

Two equivalence proofs run over the populated corpus and compare the **old** and
**new** visible id sets:
`eligibility_sets_match_legacy_predicate_for_every_order` (public relation, both
gate states, both off-spec states) and
`eligibility_sets_match_legacy_for_the_curve_relation` (legacy curve relation).
The old rule is frozen verbatim in
`rs/read/tests/support/current_order_rule_legacy_owner.sql`, test-only. Both pass.

A real bug was found and fixed by these proofs: the first attempt inserted an
extra comma before the sets fragment in the two curve relations, so
`curves_legacy`/`curves_canonical` returned 500 (PostgreSQL `42601`, syntax error
at position 1017). The equivalence test surfaced it; the corpus later confirmed
the fix.

Verification (native amd64 via `verdaxis-prod-test`):

- All 11 corpus groups match: health 8, echo 2, simple_reads 29,
  live_benchmarks 4, orderbook_snapshot 3, orderbook_lists 12, orderbook_map 9,
  prices 40, curves_legacy 4, curves_canonical 125, mcp 143.
- `run_market_contract.sh`: 4 passed (both new equivalence tests, the visibility
  matrix and the trade matrix). `run_postgres.sh` and `tests/rust_contract`
  (445) pass; fmt and clippy clean; offline workspace 34 targets green.

### p95 table, Python and Rust interleaved (240 requests, 30 per cell)

| Route | Mode | Python p95 s | Rust p95 s (baseline -> D24 -> D25) |
| --- | --- | ---: | --- |
| list | cold process | 1.602 | 2.825 -> 2.218 -> **1.982** |
| list | warm | 1.473 | 2.491 -> 2.116 -> **1.843** |
| bids | cold process | 1.641 | 2.946 -> 2.332 -> **2.164** |
| bids | warm | 1.714 | 3.875 -> 2.193 -> **2.131** |

Rust p95 is now +24% to +32% over Python, down from +38% to +45%. The gate still
**FAILS**. Page queries are now level (Rust 199.9/186.3 ms against Python
192.2/148.8 ms); the remaining gap is entirely the benchmark statement.

### Next cause, named with plans

Plans: `task13-d25-plans.log`, `task13-d25-plans.json`.

| Statement | Python ms | Rust ms |
| --- | ---: | ---: |
| list page | 192.2 | 199.9 |
| list benchmark | 88.5 | 399.7 |
| bids page | 148.8 | 186.3 |
| bids benchmark | 93.1 | 303.9 |

Python's benchmark is an index scan over exactly the requested keys (10,000
rows, ~30,280 shared hits) and qualifies them outside SQL. Rust's benchmark
nests `eligible_orders` against the requested keys, so PostgreSQL evaluates the
whole shared admissibility predicate for all ~10,047 candidate rows. Rust's
buffer count is only 277 against Python's 30,280 — so the extra time is
**row-by-row CPU in the SQL predicate** (policy JSON parsing per row, lane-array
scans, window regexes, scalar declarations), not I/O.

**This needs a PM decision.** The next step is a decision-25-style change for the
product/lane identity and scalar-declaration checks (precompute approved
`(product_id, delivery_point_id)` lane pairs and the product identity tuples once
as `MATERIALIZED` sets and test membership), or accepting that Rust's benchmark
legitimately costs more because eligibility runs in SQL by design (decision 16).
No change, no index and no schema change was made. Stopped, as instructed.

## Remaining items — both need a PM decision before I build them

### 1. Method / HEAD / trailing-slash differential corpus

The ported **data** groups carry only `GET` cases today: `simple_reads` 29 GET,
`orderbook_lists` 12 GET, `orderbook_map` 9 GET, `prices` 40 GET,
`curves_canonical` 125 GET, `orderbook_snapshot` 3 GET. Only `health` has the
method matrix (GET/HEAD/POST/OPTIONS + one slash case). `mcp` has one `GET` and
one `DELETE` case. So there is no differential coverage of HEAD, OPTIONS or
trailing-slash behaviour for the 27 ported data routes.

Adding those cases is **not** a local edit. The comparator compares a live
recording against the pinned golden, so new cases must be recorded, and the
golden oracle SHA is pinned in `deploy/rust/market-policy.manifest.json`
(`source_sha = 78af98c4…`). Re-recording the golden changes that SHA and forces
regeneration of the Task 5 policy and MCP-catalog artifacts. Corpus recording is
Task 4b/PM-owned and is shared with the build branch. **Requesting a PM decision
on when and where to record the expanded corpus.**

Note that Rust's current method behaviour is already asserted by offline tests —
`http_contract.rs` checks HEAD→405 with `Allow: GET` and the 307 slash redirect
for every `MOUNTED_GET_PATHS` entry, and `test_routing.py` checks the Caddy method
matrix (306 cases). What is missing is the corpus-level byte comparison.

### 2. Combined publication gate

The plan's step-4 command is
`run_disposable --group all --rust-target all --mixed-caddy --require-complete-ledger --report docs/rust-phase1/ci-contract.json`.
This lane's runner implements `--group all --mixed-caddy` (which runs the
unchanged integration suite) but has **no** `--rust-target`,
`--require-complete-ledger` or `--report`. `--group all` without `--mixed-caddy`
is refused by a guard.

`venv/bin/python -m scripts.rust_contract.check_assertions` reports
**`assertion ledger OK`**, and every group already matches its Rust target
individually (11/11 above), so the substance is proven piecewise. Building the
single combined command means adding the three flags and a JSON report writer to
`run_disposable.py`. The plan couples that job to artifact publication, which is
Task 12a's scope. **Requesting a PM decision on whether I extend the runner here
or defer to the Task 12a publication job.**

## Remaining gates — status after this turn

**`cargo sqlx prepare --check`: PASS.** sqlx-cli 0.9.0 is installed on the test
host. `scripts/rust_contract/sqlx_offline_check.py` runs
`cargo sqlx prepare --check --workspace` from `rs/` against its own disposable
migrated container (the subcommand takes no `--manifest-path`; it must run at
the workspace root). Exit 0 (`task13-sqlx5.log`). The project uses runtime
`sqlx::query(...)` calls only — `grep` finds no `query!`/`query_as!`/
`query_scalar!` macros — so there is no compile-time query metadata, and the
check confirms none is missing.

**OpenAPI subset comparison: not started; scope needs a PM steer.** Task 13
step 3 assumes typed DTOs with `utoipa::ToSchema` derivations. This
implementation builds responses as dynamic `json!` values: `grep` finds **no**
`utoipa`/`ToSchema`/`OpenApi` use anywhere in `rs/read/src`, and the 26 ported
routes contain about 148 `json!` construction sites across `prices` (42),
`orderbook_map` (29), `curves_canonical{,_views}` (41) and the rest. Matching
the plan's byte-exact semantic comparison therefore means hand-writing ToSchema
mirror types for every response and query parameter (names, required/default/
bounds, enum values, null-versus-omitted, content types, deprecated flags), then
a Python comparator against the recorded ported subset of `/openapi.json`.

That is a large new unit, and its method is a real choice: (a) hand-written
ToSchema mirrors that must be kept in step with the JSON by tests, or (b) a
different parity method that compares the recorded Python OpenAPI subset
per-operation without generating a Rust schema. The plan fixes (a) but its
"derive on each DTO" premise does not hold here. **Requesting a PM steer on the
method before building it.**

Still open after that: the expanded method/HEAD/trailing-slash differential
corpus, and the combined `--rust-target all --require-complete-ledger --report`
publication gate (the runner in this lane exposes `--group all` with
`--mixed-caddy`, not those flags).

**p95 go/no-go remains FAIL and awaits the eligibility-cost decision** (above).

## Decision 24 implemented — digest input built for kept rows only
### After decision 24 — p95 gate still FAIL, next cause named

240 requests, 30 per cell, interleaved, same disposable database, code
`8d5084a9` (`task13-d24-measurement.log`).

| Route | Mode | Python p50 / p95 s | Rust p50 / p95 s | Rust p95 vs Python |
| --- | --- | --- | --- | --- |
| list | cold process | 1.340 / 1.574 | 1.928 / 2.218 | +41% FAIL |
| list | warm | 1.195 / 1.487 | 1.810 / 2.116 | +42% FAIL |
| bids | cold process | 1.451 / 1.612 | 2.059 / 2.332 | +45% FAIL |
| bids | warm | 1.250 / 1.570 | 1.896 / 2.193 | +40% FAIL |

Decision 24 improved Rust p95 by ~0.7 s (list cold 2.825 -> 2.218, bids warm
3.875 -> 2.193) and the benchmark statement no longer carries the digest
(`Plan Width` 3289 -> 100). The remaining gap is a uniform ~0.6 s (~+40%), so it
is a fixed per-request database cost, not the digest projection.

Next cause, from the plans on this code (`task13-d24-plans.log`,
`task13-d24-plans.json`). Rust's shared eligibility predicate tree is evaluated
for every candidate row inside SQL:

| Statement | Python ms | Rust ms | Rust's dominant node |
| --- | ---: | ---: | --- |
| list page | 156.6 | 238.4 | `Subquery Scan rows=10045 t=218.8` (the eligible relation) |
| list benchmark | 104.9 | 458.4 | join over `rows=10047 t=279.6`, inner `rows=10049 t=151.2` |
| bids page | 139.5 | 226.7 | `Subquery Scan rows=10023 t=214.4` |
| bids benchmark | 89.4 | 426.4 | join over `rows=10047 t=264.0` |

Rust scans and strict-qualifies about 10,045 `orderbook_orders` rows to return a
50-row page or a 20-row page, because the shared relation applies the policy
identity/lane, owner/KYC and current-order rules as SQL predicates with no
index-supported early termination. Python's equivalent statements are narrower
and much cheaper. The digest is no longer involved: the Rust benchmark output
width is now 100 against Python's 1775. This is the SQL-side eligibility design,
not the projection.

**This needs a PM decision.** Removing it means either an index on the
eligibility columns (forbidden without a decision) or changing how the shared
relation evaluates eligibility. No index, no schema change and no predicate
change was made. Stopped here for the decision, as instructed.



PM decision 24 approved moving `private_digest_input` out of the shared
visibility relation. Implemented, code commit `8d5084a9`.

One definition: `rs/read/sql/private_digest_input.sql` holds
`to_jsonb(alias) || jsonb_build_object(...)` with a `/*row_alias*/` placeholder.
`market::visibility::private_digest_input_sql(alias)` expands it, and
`order_query_sql` replaces every `/* private_digest_input(alias) */` marker in a
tail. No consumer copies the expression; `live_benchmarks.sql` keeps its own
narrower qualification object. The relation no longer projects the digest.

Shape per consumer (all build the digest after their own filter/sort/limit):
snapshot ranks `eligible_orders` first, then joins `orderbook_orders` for the
`depth_rank <= 15` rows; lists/bids/with-ci/asks select narrow page keys with
LIMIT/OFFSET, then join the base row by primary key for the returned page; asks
keeps its `count(*) OVER ()` total; listings keeps `created_at DESC, id`.
`market_contract_postgres.rs` now selects the digest through the shared marker
for the rows it keeps, matching the new contract.

Verification (native amd64 via `verdaxis-prod-test`), all on the decision-24
code:

- All 11 corpus groups match Python at full count: health 8, echo 2,
  simple_reads 29, live_benchmarks 4, orderbook_snapshot 3, orderbook_lists 12,
  orderbook_map 9, prices 40, curves_legacy 4, curves_canonical 125, mcp 143.
  Digest bytes are unchanged, so the golden still matches byte for byte.
- `run_market_contract.sh`: 2 passed (14/46 orders, 4/6 trades).
- `run_postgres.sh`: 155 passed.
- `cargo fmt --all -- --check`: pass. `cargo clippy --workspace --all-targets
  -- -D warnings`: pass. `cargo test --workspace`: 146 passed.
- `venv/bin/python -m pytest tests/rust_contract -q`: 445 passed, 5 warnings.

## Status summary — after the merge

Done and green on the merged tree: Task 11 merge with one shared port helper;
Podman image normalization as its own commit; all 11 corpus groups matching at
full count (758-case unified golden); `run_market_contract.sh` and
`run_postgres.sh`; fmt, clippy, workspace tests (146) and the Python contract
suite (445); health HEAD/`Allow` fix; Caddy route-ownership corpus (306 cases).

Open and unaccepted: the paired p95 go/no-go gate (FAIL, table above); OpenAPI
subset comparison; expanded method/HEAD/trailing-slash differential corpus;
combined `--rust-target all --require-complete-ledger --report` publication
gate; `cargo sqlx prepare --check` (Cargo SQLx subcommand absent on the test
host — unrun, not passed).

Next: PM decision on moving `private_digest_input` out of the shared relation
(the concrete remaining latency fix), then the OpenAPI and publication gates.

## Native test-host checkpoint — 16 October

All database/Linux commands now use `verdaxis-prod-test "$PWD" 'COMMAND'`.
Colima stayed off. No direct SSH or test-host setup change was made.

- `d4c6be3b`: test-first Linux port allocation repair. Kernel port 38653 failed
  the existing 49152–65535 guard; the runner now probes within the guarded range.
  Offline regression: one passed. No collection guard was weakened.
- Native analyzed-corpus regression reproduced list and bid ten-second timeouts
  (`task13-native-latency.log`). This rules out emulation as the sole cause.
- `4cb01a1a`: paginate before the supplier-tier display join, keep explicit outer
  ordering, and remove the benchmark's full-candidate materialization barrier.
  Eligibility and typed qualification are unchanged. The existing shared
  `sql/live_benchmarks.sql` changed because its optimization fence forced full
  private-row projection before the requested-key join; no predicate was copied.
  Native list/bid timings improved to 6.274/6.226 seconds; all map timing checks
  were 0.023–0.414 seconds (`task13-native-sparse.log`).
- Unchanged mixed suite: **24 passed, 49 failed, six errors**, zero timeouts
  (`task13-native-mixed.log`). All six formerly failing public reads pass.
  Every remaining failure is classified in `task13-integration-failures.md`.
  These are observed fixture/auth failures, not proof that unreachable write
  assertions will pass after Phase 0. No Phase 0 fixes were copied.
- `77f30129`: benchmark corpus collision reproduced, then fixed by reusing the
  matrix and cloning its cancelled negative declaration into a probe namespace.
  Populated benchmark target passes: four corpus cases, three base comparisons,
  seven acceptance probes (`task13-native-benchmark-green.log`).
- Native list corpus: 12 cases match; 24 integer boundaries and 15 non-null CI
  projections pass. Full target remains blocked at query-count evidence:
  Podman normalizes Config.Image to docker.io/repository@digest (adapted without
  weakening repository/digest verification), but `docker logs` returns no new
  bytes after enabling PostgreSQL statement logs. Isolated failure reports
  `expected one benchmark statement, observed 0; new log bytes=0`.
  Logs: `task13-native-count.log`, `task13-native-log-config.log`.
  PostgreSQL reports logging_collector=off and log_destination=stderr. Do not
  change host logging setup or claim the statement-count assertion passed.
- Offline workspace 128 passed; fmt and strict clippy passed. Native runtime
  PostgreSQL runner 136 passed, two dedicated matrix tests filtered.

**Resume:** resolve the missing disposable container log evidence with the test
host owner, or approve a test-local SQLx statement counter. The current host
logging surface does not supply the query-count test's required evidence; no
host setup was changed. Then rerun the complete list target, finish OpenAPI,
method/slash differential cases and combined ledger/publication gates. All test
runs launched in this checkpoint finished. No acceptance claim for Task 13.

## Implementation checkpoint

- Added the route ownership test first. Its first run had six expected setup
  errors because `route_owner` did not exist.
- Added a real Caddy fixture with two instrumented loopback upstreams. Caddy
  adapts and validates the actual staging snippet, then serves requests. All
  306 parametrized tests pass, including all 27 GET paths, MCP method routing,
  HEAD/OPTIONS/slash variants, private and excluded paths, Host forwarding, and
  independent rollback of all five groups. Caddy 2.10.2 was built with Go into
  ignored `work/rust-phase1/bin/caddy`; no shared Python packages were installed.
- The staging snippet is inert configuration only. Import it inside the existing
  site's route block, before its existing fallback, with five independently
  approved upstream arguments. Revert one argument to Python for one-group
  rollback. This task does not install or activate it. HEAD and OPTIONS remain
  Python-owned. Health, identity, docs and unknown paths remain Python-owned.
- The health HEAD regression first failed (200 instead of 405). The dispatch
  boundary now overrides axum's automatic HEAD behavior with the recorded 405
  and `Allow: GET`. The eight-case populated health group passes. The assembled
  offline test checks HEAD, OPTIONS and slash redirects on every public GET.
- `public_routes` assembles all completed modules and the empty Task 11 MCP seam;
  `public_router` adds health and the shared middleware. The contract server's
  `all` group consumes the same assembly.
- `runtime/policy.rs` moves the contract server's digest/source-attested loader
  and three tests into shared runtime code. The real serving binary no longer
  uses an empty policy. No catalog policy, SQL or market predicate changed.
- The existing disposable runner now accepts `--group all --mixed-caddy` and
  launches the unchanged integration suite after attesting Caddy's Python-owned
  identity endpoint. It uses the same populated database for Python and Rust,
  the recorded Asia/Singapore zone, ephemeral loopback listeners and owned
  process/container cleanup. No second lifecycle runner was added.

## Checks completed at this checkpoint

Logs: ignored `work/rust-phase1/task13-*.log`.

- Rust offline workspace: 128 tests passed. Strict clippy and rustfmt pass.
- Runtime PostgreSQL runner: 136 tests passed; two market-matrix tests filtered
  by that existing runner for their separate migrated target.
- Python contract suite: 402 passed, five warnings (includes Caddy tests).
- Python unit suite: 2027 passed, 19 failed. All failures are in
  `test_deploy_health.py`, the recorded macOS Bash 3.2 limitation. No deploy
  implementation was changed.
- Mixed integration: 18 passed, 55 failed, six setup errors, zero skipped.
  Known fixture problems include tokens missing issuer/audience, invalid
  synthetic email domains, and shared login rate counters. Five setup errors
  explicitly report login 429; the lifecycle setup reports incorrect login
  credentials. Six public orderbook reads timed out at the suite's 10-second
  deadline. These timeouts are **not** classified as known fixture rot.
  No assertion or fixture in `tests/integration` was edited.
- Populated live-benchmark probe fails before comparison: it tries to insert
  existing corpus user `4e4b985d-a14e-5d26-8dbf-58997282248e` (`users_pkey`).
  This is a Task 6a/harness follow-up, not a reason to modify its fixture here.
- All launched runs finished and removed their disposable containers. Populated
  groups passing their complete targets: health 8 (both KYC states), echo 2,
  simple reads 29, snapshot 3, lists 12, prices 40, legacy curves 4, canonical
  curves 125. These cover 223 cases per enabled-state run across passing groups.
  Lists also passed 24 integer boundaries, 136 edge comparisons, 15 non-null CI
  projections and mixed-sort checks. Prices passed two sets of 17 probes;
  canonical curves passed 112 probes.
- Map's nine corpus cases match, but its extra probe failed on a Rust request
  timeout for `/api/orderbook/aggregated?region=Busan`. This is not acceptance.
  Together with live benchmarks, two full group targets failed. No timeout or
  comparator rule was relaxed.
- `cargo sqlx --version` failed: the Cargo SQLx subcommand is not installed.
  Thus `cargo sqlx prepare --check` remains unrun, not passed.
- Commits: `cdcb7a87` (Caddy ownership/rollback), `f231735a` (assembly,
  attested loader, mixed runner and checkpoint notes). No running child or
  container remains from these checks; `docker ps` was empty at final inspection.

## Timeout investigation and first repair

Sequential requests on the same pinned disposable amd64 PostgreSQL isolate the
fault from Caddy and admission concurrency. Initial direct Rust/Caddy/Python
seconds: list 18.144/17.169/4.506; bids 16.418/16.697/3.382;
aggregate 32.859/31.631/0.861; regions 33.082/30.365/0.855.
Rust map requests returned 503 + Retry-After after the 30-second database limit;
list requests returned 200 after the integration client's ten-second deadline.
This is not evidence of pool saturation: requests were sequential and direct
Rust reproduced the proxied result. No limit was increased in application or
integration tests. Diagnostic clients alone waited up to 65 seconds to observe
completion. Logs: `task13-diagnostic.log`, `task13-explain-run.log`.

`ANALYZE` did not repair the regression (`task13-analyzed.log`). Disposable-only
`jit=off` reduced aggregate to 1.557 seconds, regions to 1.117, fuel types to
1.223, and absent-region aggregate to 0.038. Lists still took 15–17 seconds
(`task13-jit-off.log`). Thus emulation may amplify cost, but it is not a sufficient
explanation: Python on the same database is much faster, and planner changes
remove map timeouts without changing hardware or query semantics.

Fresh, not-yet-analyzed corpus statistics can keep map plans below the JIT cost
threshold. A fresh-corpus probe alone therefore passed before the repair.
After ANALYZE, the same ten-second regression failed for all repeated map reads
without auto_explain/logging enabled (`task13-latency-analyzed-red.log`).
Transaction-local JIT-off in map/count/distinct handlers then passed the same
analyzed-corpus reads in 0.035–1.543 seconds while both list reads still failed
(`task13-map-green-list-red.log`). This also fixes the Busan map-probe timeout.
No shared eligibility predicate or SQL was changed. The setting rolls back or
commits with the admitted transaction; it cannot leak through the pool.

The map contract target now runs the ten-second latency probe. Final populated
map target: ten timing checks 0.495–1.370 seconds, nine corpus cases matched,
97 edge comparisons passed (`task13-map-final.log`). Strict clippy passed.

List diagnosis remains open. PostgreSQL duration logs attribute about 7 seconds
to the list query and 7–9 seconds to the benchmark query. The actual list plan
sorts 10,045 eligible rows before LIMIT 50; benchmark materialization retains
10,047 candidates and returns 10,000 matching rows. JIT is already off in both.
No list optimization has been claimed or committed. Full actual plans are in
ignored `work/rust-phase1/task13-plans.json`. Resume with this query-cost issue,
not fixture rot or OpenAPI. All diagnostic containers were removed.

## Remaining work / resume point

1. Resume with the incomplete OpenAPI gate and method/slash differential cases.
   Every launched test has finished; do not treat this checkpoint as full Task
   13 acceptance. Preserve the two group failures listed above for their owners.
2. Add the complete semantic OpenAPI export/comparison. Existing route DTOs have
   no utoipa derives; this requirement is not implemented or claimed complete.
3. Complete the combined `--rust-target all --require-complete-ledger --report`
   interface with the harness owner. Those flags are not present in this lane's
   base runner. Current `all` is deliberately valid only with `--mixed-caddy`.
4. Integrate Task 11 when provided; the MCP seam is mounted but still empty.
   Do not claim its protocol corpus or publication gate passes.
5. Investigate the six public-read timeouts separately from Phase 0 fixture
   repairs. Do not increase the unchanged suite's timeout to hide them.
6. Run `cargo sqlx prepare --check` on the disposable schema and the complete
   CI contract/publication dependency gate after schema/MCP integration.
7. Expand method/slash differential cases and golden recording, then rerun all
   required checks on the final assembled commit.

New shared file: `rs/read/src/runtime/policy.rs` (startup policy loading only).
No new shared market relation or SQL file. No architecture or global progress
file was edited; the task-specific file ownership rule takes precedence here.

## Lane 13 continuation — 16 October (regression fix and combined gate)

### 1. orderbook_lists regression at 3bf216ac — cause and fix

Bisect result: the regression is not a behaviour change between `fc63cf3a`
and `3bf216ac`; it entered with `f7b6c7b8`'s new corpus cases and was revealed
by the `3bf216ac` re-record. Exactly one case, `R26-orderbook-post-405`,
differs (4 schema paths). Python registers a real POST write route on
`/api/orderbook` (`app/routers/orderbook.py:1802`), so an unauthenticated POST
fully matches that route and the OAuth2 dependency answers **401**
`{"detail":"Not authenticated"}` with `WWW-Authenticate: Bearer`. The other 27
ported paths have no non-GET route, so they answer 405 — the assumption the
new cases were written under. The golden corpus is truthful (all other
754 re-recorded cases byte-identical), so the fix belongs in Rust.

Fix, commit `47d0da6f`: Rust registers a POST handler on `/api/orderbook`
that answers Python's unauthenticated boundary (a present `Bearer` scheme
answers Python's could-not-validate detail). A middleware-only answer kept
axum's method-not-allowed fallback in the chain, which appended an
`Allow: GET,HEAD` header Python never sends — the route-level handler avoids
that. Python's support-scope middleware classifies POST `/api/orderbook` as an
allowed market-support mutation (`app/services/request_party.py:273`), so the
dispatch boundary exempts that one combination from the generic 403; every
other context-bearing mutation still answers 403.

Proof: `orderbook_lists` passes alone — 28 corpus cases byte for byte, 24
integer boundaries, 15 non-null CI overlays, one benchmark statement,
mixed-value sort pages, 136 exact edge comparisons. The offline suite gains
`post_orderbook_answers_python_auth_boundary` (16 passed).
Log: `work/rust-phase1/task13-lane13-lists-fixed2.log`.

### 2. Combined gate failure — cause and fix

Baseline re-run at `47d0da6f` with one shared database
(`task13-lane13-combined-1.log`): health, echo, simple_reads,
live_benchmarks, prices and mcp pass; snapshot, lists, map, curves_legacy and
curves_canonical fail — every one from **persistent cross-group corpus
mutations**:

- `live_benchmarks`' probe sets `lng_terms.production_country='ZZ'` and
  rewrites corpus quantities/prices; snapshot and lists then read those rows
  and Python's `OrderResponse` validation raises 500 (`ZZ` is not an ISO
  3166-1 alpha-2 code), so both endpoints' live comparison sees status/keys
  differences.
- `orderbook_snapshot`'s probe copies rows with empty certificate/supplier
  strings and a PENDING quality sample into windows `2032-*`; the map
  target's listings probe reads all current ASK listings, Python 500s on
  `certificate_holder: string_too_short`.
- `curves_legacy`'s scale probe asserts an exact `2030-01` CSV row
  (`order_count` 2, volume 200.00); earlier mutations had changed that slice.
- `curves_canonical`'s cost probe read zero metrics events for the combined
  server (`"statements": 0`), failing the grouped-rows assertion.

Fix (uncommitted at this point): `--rust-target all` now starts **one fresh
disposable environment per group** (`start_environment`/`stop_environment` in
`scripts/rust_contract/run_disposable.py`) — new container, migrate, corpus
seed, fresh Python and Rust servers per group, then teardown. A `--mixed-caddy`
combined run keeps its own fresh database for the integration suite for the
same reason (its writes also mutate the corpus). Single-group invocations,
`--record-all` and the measurement scripts (which subclass `run_mixed` on one
shared database) keep their exact previous behaviour. No probe was weakened or
skipped. New unit test `test_combined_gate_gives_each_target_a_fresh_environment`
proves per-group start/stop ordering.

### 2a. First isolation attempt had its own sequencing bug (recorded for honesty)

The first per-group fix looped `start_environment(group)` outside
`run_targets`, but `run_targets` still iterated every group internally, so
all eleven targets ran against each per-group environment (121 target
executions; every data group failed 404 against servers that mounted only
one group's routes). Caught by reading the log's server-ready lines against
the target-result lines. `run_targets` now takes an explicit `groups` list
and the combined loop passes exactly the group its environment started
(`93f50591`). The wasted full run was left to finish; no call was
interrupted.

### 2b. Corrected gate run blocked by the paused test host

The corrected combined gate (`--group all --rust-target all
--require-complete-ledger --report docs/rust-phase1/ci-contract.json`,
commit `93f50591`) could not run: `verdaxis-prod-test` is paused by the PM
for a Phase 0 rehearsal on the prod host (16 Oct, ~20:30 SGT, 1-2 hours;
wrapper exit 75, "do not retry"). Strong indirect evidence the isolation
works came out of the wasted 121-execution run: against its OWN
environment, `curves_legacy: OK` (including `legacy price scale: 8
comparisons`) and `curves_canonical: OK` (including CURVE_QUERY_COST and
CURVE_MEMORY) — the two groups whose probes read cross-group state most
directly. The same run shows the sequencing bug's signature clearly
(every data group 404s against a foreign group's server), so the evidence
separates the two defects.

**Resume point:** rerun the corrected combined gate end to end when the
test host unpauses, capture its report into
`docs/rust-phase1/ci-contract.json`, then run the p95 profiling
(`work/rust-phase1/profile_lists.py`) and the disposable index experiment
(`work/rust-phase1/index_experiment.py`) with no other heavy run in
parallel.

### 2c. Isolation correctness proven per group by the 121-execution run

The wasted run doubles as per-group proof: every group that reached its
own environment passed its own target there — health 11x, echo,
simple_reads (line 5469), live_benchmarks (8122), orderbook_snapshot
(11454), orderbook_lists (14345), orderbook_map (17777), prices (20707),
curves_legacy (23452, including `legacy price scale: 8 comparisons`),
curves_canonical (26555, including CURVE_QUERY_COST/CURVE_MEMORY) and mcp
(29538, 143 cases plus privacy and SQL-cancellation proofs). The
corrected gate collapses this to eleven environments, one target each.

### Remaining sequence once the test host unpauses (resume point)

1. `verdaxis-prod-test "$PWD" 'venv/bin/python -m scripts.rust_contract.run_disposable
   --group all --rust-target all --require-complete-ledger
   --report docs/rust-phase1/ci-contract.json'` — the corrected combined
   gate. The report JSON is printed to stdout by `write_report`; capture it
   from the log into `docs/rust-phase1/ci-contract.json` (the remote writes
   its own copy that does not sync back).
2. `venv/bin/python work/rust-phase1/profile_lists.py` through the wrapper —
   one list and one bids request per runtime, wall time, per-statement
   pg_stat_statements, and outside-SQL time. Name where the ~0.5 s goes.
3. `venv/bin/python work/rust-phase1/index_experiment.py` through the
   wrapper — disposable-only partial index
   `(side, created_at DESC, id DESC) WHERE status IN ('OPEN','PARTIALLY_FILLED')`,
   warm p95 for both runtimes with/without, EXPLAIN plans for the walk and
   the real route statements, then DROP INDEX. Report and stop: the real
   index is a PM decision.

### 3. p95 breakdown — where the ~0.8 s goes

`scripts/rust_contract/profile_lists.py` (module form) on one ANALYZEd
disposable corpus, warm process and pool, five samples per cell, resetting
`pg_stat_statements` per request and excluding the Python outbox poll
(`task13-lane13-profile2.log`). Admission wait is ~0 here: sequential
requests, eight free slots, so `try_acquire_owned` succeeds on the first
try and no request queues.

| Cell | wall median | SQL total | outside SQL |
| --- | ---: | ---: | ---: |
| list Python | 1.022 s | 0.247 s | 0.776 s |
| list Rust | 1.865 s | 0.719 s | 1.146 s |
| bids Python | 1.214 s | 0.363 s | 0.851 s |
| bids Rust | 1.832 s | 0.815 s | 1.017 s |

Per statement (Rust list): the live-benchmark statement
`WITH requested_keys AS MATERIALIZED (...)` is **523 ms**; the page
statement `WITH eligible_orders AS NOT MATERIALIZED (...)` is 196 ms.
Rust bids: benchmark 523 ms, page 293 ms. Python list: product catalog
127 ms, its one benchmark/order statement 120 ms (Python batches the page
and benchmark work into fewer, narrower statements); bids adds a count
statement 88 ms.

So the ~0.84 s list gap splits into **+0.47 s of SQL, dominated by the
523 ms key-first benchmark statement**, and **+0.37 s outside SQL** —
Rust's pool acquire is ~0 after warmup, so that remainder is row → public
projection, `serde_json` serialization and HTTP framing. The page
statements are not the cause: Rust 196/293 ms against Python 120/148 ms.
Naming it plainly: the single Rust-only 0.5 s is the batched live-benchmark
statement, and a smaller part is Rust's per-row projection and JSON build.

### 4. Index experiment (disposable database only) — numbers and plans

`scripts/rust_contract/index_experiment.py` created the candidate partial
index **inside its own disposable container only** (never a migration file):

```sql
CREATE INDEX ix_lane13_open_page_experiment
  ON orderbook_orders (side, created_at DESC, id DESC)
  WHERE status IN ('OPEN', 'PARTIALLY_FILLED')
```

Warm p95, 30 alternating samples per cell, fresh service processes after
the index change (`task13-lane13-index.log`, CALL_EXIT=0; the index is
dropped before cleanup):

| Cell | p50 without | p50 with | p95 without | p95 with |
| --- | ---: | ---: | ---: | ---: |
| list Python | 1.115 s | 1.098 s | 1.397 s | **1.288 s** |
| list Rust | 1.753 s | 1.686 s | 2.025 s | **1.872 s** |
| bids Python | 1.260 s | 1.228 s | 1.593 s | 1.652 s |
| bids Rust | 1.888 s | 1.853 s | 2.292 s | **2.059 s** |

Plans: the page-walk shape (side + open status, `created_at DESC, id
DESC`, LIMIT 20) changes from a Seq Scan + top-N sort over all 10,052 open
orders (9.1 ms, 274 buffers, BID) or the existing slice-lookup index scan
(0.43 ms, ASK) into an **Index Only Scan reading 20 rows from 3-4 buffers
(0.19-0.33 ms)** on both sides. The real route statements' plans confirm
the effect where it applies: Python's bids page plan's top node becomes a
`Limit` (early termination in page order) with the index present.

Interpretation: the index removes the sort-and-scan of the open set for
page-ordered reads, worth about 0.11-0.23 s of p95 on the default page in
this corpus, and it slightly narrows the Rust-Python gap (list p95
2.025 -> 1.872 s against Python 1.397 -> 1.288 s). It does **not** close
the gap: the dominant costs from the breakdown above (the 523 ms key-first
benchmark statement and ~1.0 s outside SQL) are untouched by a page-order
index. **Stopped here as instructed — adding a real index is a PM
decision.** No migration file was created or modified.

### 5. Release-profile re-measurement (PM finding accepted)

PM finding: every earlier p95/breakdown run built the contract server
without `--release`, so the Rust numbers measured an unoptimized dev
build. `Runner` now has an explicit `release` flag (commit `aecb2f4c`):
`build_command()` appends `--release` and `server_path()` selects
`rs/target/release/examples/contract_server` only when set. Correctness
runs keep the dev profile (overflow panics); `measure_lists`,
`profile_lists` and `index_experiment` always pass `release=True`.
Unit test `test_build_profile_selection_keeps_correctness_on_dev` pins
both profiles (8 tests pass).

p95, release build, 240 requests / 30 per cell, interleaved
(`task13-lane13-measure-release.log`, CALL_EXIT=0 — **the
`require_latency_parity` gate passes: Rust p95 <= Python p95 in all
eight cells**):

| Cell | Python p50 / p95 s | Rust p50 / p95 s |
| --- | --- | --- |
| list cold process | 1.299 / 1.515 | **0.881 / 1.152** |
| list warm | 1.265 / 1.775 | **0.841 / 1.066** |
| bids cold process | 1.477 / 1.670 | **1.077 / 1.246** |
| bids warm | 1.273 / 1.707 | **0.952 / 1.159** |

The dev-profile deficit (Rust +24-32% over Python) inverted: the release
Rust service is now 24-40% faster than Python per cell.

Warm breakdown, release build, same method (`task13-lane13-profile-release.log`,
CALL_EXIT=0):

| Cell | wall median | SQL total | outside SQL |
| --- | ---: | ---: | ---: |
| list Python | 1.107 s | 0.245 s | 0.862 s |
| list Rust | **0.807 s** | 0.667 s | **0.140 s** |
| bids Python | 1.171 s | 0.415 s | 0.756 s |
| bids Rust | **0.975 s** | 0.864 s | **0.111 s** |

The PM's diagnosis is confirmed: the dev build's 1.15 s outside SQL is now
0.14 s. Rust is faster than Python on wall time in both cells. The largest
remaining single cost is the key-first benchmark statement: 493 ms (list)
and 542 ms (bids), server-side. Step 3 of the PM instruction targets it.

### 6. Benchmark statement repair — cause, fix, proof (commit `df5dd4a5`)

Cause, from auto_explain on the disposable corpus
(`task13-lane13-explain-bench.log`, `scripts/rust_contract/explain_benchmark.py`):
the decision-27 fences (`requested_slices AS MATERIALIZED`,
`keyed_orders AS MATERIALIZED`) hid the real `orderbook_orders` row counts
from the planner, so it walked the 10,000 keyed rows in a nested loop that
re-scanned `products` (loops=10000) and `delivery_points` (loops=10000)
and the policy CTEs once per row — 265 ms of per-row work — plus 77 ms
scanning the materialised wide `o.*` tuplestore: 417 ms total for 10,000
rows. Python's equivalent work is a narrow key-driven scan that qualifies
outside SQL.

Fix: `requested_slices` and `keyed_orders` are each referenced exactly
once, so they are now `NOT MATERIALIZED` and the planner inlines them and
hash-joins the small dimension tables. `requested_keys` stays MATERIALIZED
(two readers). No predicate, key column or eligibility rule changed.

Byte-exactness proof: `benchmark_key_first_matches_key_join_last` now
executes three shapes — the original key-join-last, the exact decision-27
fenced statement (test-only `benchmark_candidate_sql_fenced`), and the
unfenced statement — and requires the identical row multiset from all
three. `run_market_contract.sh`: 5 passed. Affected corpus groups green
alone (live_benchmarks 4, snapshot 7, lists 28 + all probes). fmt and
strict clippy clean; offline workspace tests pass.

Combined gate at `df5dd4a5`: all eleven targets OK, assertion ledger OK,
exit 0; `docs/rust-phase1/ci-contract.json` updated (no failed groups).
Log: `task13-lane13-fence-gate.log`.

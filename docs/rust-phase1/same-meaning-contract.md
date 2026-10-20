# Phase 1 same-meaning successor contract

## Authority and release boundary

This branch prepares Tasks 1–6 only. It does not authorize deployment or change
`integrate/rust-phase-1-20261018`, the first byte gate, Caddy activation, or its
artifact. Production activation requires Phase 1 live in production and the
founder's decision. Production Phase 1 identity is **not verified** by this lane.
No live host or health endpoint was inspected.

No load, latency, memory, capacity, or rate measurement is authorized in Tasks
1–6 (decision 96). Do not refresh Python's decision-87 baseline.

## Source checkpoints

| Source | Exact identity | Scope |
| --- | --- | --- |
| Integrated base | `34dacf327d56ec32ddaf74c80390986a4fe1061d` | Starting source, not deployment proof |
| Final shared SQL speed handoff | `738fe0e85d85d7667263e8950fe4de4c05831e38` | Already integrated; do not repeat the SQL edits |
| Integrated gate successor | `3990151f04c99232a8ea03592828240a682e280c` | Includes decision-96 rate-limited sample-floor formatting |
| Frozen-baseline gate handoff | `92a97587d72678a75921734b09d1d93727884059` | Already integrated; reuse baseline and gate implementation |
| Reviewed Phase 2 comparator | `64179cd15c85e1da76ec49d8756b6f7419e94c66` | Import by Git history, then extend the same engine |
| Observed Phase 2 remote tip | `a446c628c4dabe486b157217fc16b60570109242` | Source observation only; do not merge unrelated Phase 2 work |
| Frontend step 5b | merge `c3633f70`, reviewed tip `03248101` | Tracker records PR 93 and production promotion 39080087535; full frontend SHA needs reconciliation before release |
| Live Phase 1 identity | Not established | Blocks activation, not isolated preparation |

The frontend identity above is a historical tracker observation, not a new live
check. Do not turn a short SHA into a fabricated full identity.

## Inventory and default policy

`tests/rust_contract/phase1_meaning/routes.json` pins 27 measured GET routes,
three MCP tools, and all 487 requests across the 11 existing correctness groups.
It copies whole request definitions, not only positive request IDs. This retains
malformed inputs, protocol versions, methods, redirects, supplied headers, policy
fixtures, and negative cases. `test_phase1_meaning_inventory.py` checks completeness
against the existing corpus and tests missing and changed coverage.

Comparator IDs use `P1-` prefixes because Phase 2 already uses R01–R05 for referral
routes. These are distinct route namespaces in the same comparator, not a second
comparison engine. Task 2 must add an immutable Phase 1 ledger through the existing
loader. The byte path and `phase2-meaning-v1` remain unchanged.

Until Task 2 reviews a scalar path, comparison is strict. Empty scalar path lists
are not permission to normalize every numeric-looking string. Unclassified cases
are strict and uncached. CSV exports remain byte-exact without an approved codec.

### Accepted differences

- Decision 84: only declared MCP validation diagnostic presentation may change.
  Rejection, semantic error kind, location, input, envelope and `isError` remain
  strict. Ordinary messages remain exact.
- Decision 85: tape membership uses `(confirmed_at DESC, full trade UUID DESC)`
  **before** pagination. The expected page comes from the complete pinned Python
  filtered relation and public DTO projection. Never sort a Python page or compare
  only truncated public IDs. Canonical tape array order is strict.
- JSON syntax and exact mathematical JSON-number equivalence are permitted only
  through the shared reviewed profile. Decimal strings and timestamps need exact
  declared paths. Types, null/absence, values, privacy, status and meaningful
  headers remain strict. News permits only equal-`published_at` tie permutations;
  price summaries retain their reviewed collection rule.
- Decision 94: do not fix Python. The Phase 2 plan section 0 owns the known-defect
  list. Its PD1–PD7 concern later authenticated routes, not a blanket Phase 1
  exception. Any applicable defect must be an explicit approved difference, never
  a silent pass. New defects require PM approval.

## Dependency handoff for later cache work

Reference caching is allowed only for R01–R05: ports/intelligence, catalog products,
delivery points, and published news. Use complete typed query keys, environment
and policy identity, a monotonic 60-second expiry, and byte/entry bounds. Cache only
successful public payloads; rebuild request headers. Invalid input and errors do
not enter the cache. No personal, auth, health, or private data may be cached.

Trading routes remain uncached in Tasks 1–6. Their shared dependency families are
orders, formal trade snapshots and state, product/lane eligibility, delivery-point
activity, user and organization admission/provenance, assisted-member eligibility,
KYC reviews and grace, expiry and the captured clock, quarantine, trusted signals,
and live/manual benchmarks. Task 7 must derive the exact table/column trigger
matrix from source; this family inventory does not authorize infrastructure or
claim that economic events cover policy changes. Python remains the HTTP writer
and SSE sequencer.

## Frozen future sampling schedule

`phase1-meaning-sampling-v2` is a future qualification schedule, not permission to
run it now. Light IDs: R01–R05, R07, R08, `list_markets`, `plan_voyage_fuel`.
Heavy IDs: R06, R09–R27, `get_market_slice`. Use equal-weight rounds within each
class; keep baseline verdict weights and query identity unchanged.

| Size/class | Candidate RPS | Window per cache mode | Chunks |
| --- | ---: | ---: | --- |
| Small/heavy | 10 | 2,100 s | 2 × 1,050 s |
| Large/heavy | 2 | 10,500 s | 5 × 2,100 s |
| Small/light | 40 | 225 s | 2 × 112.5 s |
| Large/light | 40 | 225 s | 2 × 112.5 s |

Future qualification needs 1,000 successful samples per ID/size/cache mode, exact
identity and complete-round ranges, separate off/on cells, and Rust-only timing.
Candidate and 25%-higher guard probes follow 60 s warm-up and run for 120 s each.
Ladders are small heavy `[10,5,2,1,0.5,0.25]`, large heavy
`[2,1,0.5,0.25,0.1,0.05]`, and light `[40,20,10,5]`. Lower rates preserve counts
and extend durations; heavy subchunks are at most 4,200 measured seconds. No
Python performance cell may be collected. Task 11 needs separate PM authority.

## Shared-file ownership and resume point

Execute Tasks 1–6 serially on this branch, without subagents. Task 4 owns amount
call replacements in tape/prices/orderbook-map until its commit. Task 5 takes tape
after that handoff. Later SQL lanes and cache integration must use the committed
amount API; shared visibility patches apply serially, Task 9 then Task 10.
Tasks 7–12 are outside this brief.

### Task 1 checkpoint

- Inventory RED: pytest exit `1`, wrapper exit `1`; six expected failures because
  the successor inventory did not exist.
- Inventory GREEN plus existing inventory suite: pytest exit `0`, wrapper exit
  `0`; 10 passed on the staging test host with Python 3.12.
- `git diff --check`: exit `0`.
- No build, test, or project code ran on the Mac. No measurement ran anywhere.
- CI: not run; no push or PR creation while publication authority is unresolved.
- Remaining inventory detail: Task 2 must replace the strict scalar placeholders
  with source-derived decimal/timestamp paths in the shared ledger and reconcile
  those paths back into this inventory. This checkpoint does not claim typed-path
  normalization is implemented.

PM clarified publication authority: push only `feat/rust-same-meaning` and open
the draft PR. PR 90 targets staging, is draft, and says Do not merge. Task 1 CI
completed: offline, contract, test, postgres-phase0 and postgres-analytics passed;
publish skipped. No PR comments, edits, merge or close actions are authorized.

### Task 2 checkpoint

- Reused `64179cd1` through cherry-pick with source attribution (`6c44e31a`).
  Kept the already-integrated baseline architecture notes and omitted unrelated
  Phase 2 output documentation. No application Python code changed.
- Imported comparator regression suite: 94 passed, test exit `0`, wrapper exit `0`.
- RED: 23 failed, 64 passed; test exit `1`, wrapper exit `1` (missing successor
  ledger/profile). Source-derived ledger export: exit `0`, wrapper exit `0`.
- First GREEN: 1 failed, 122 passed, exit `1`; the negative `isError=false` mutation
  was correctly rejected as invalid inner JSON, while the test expected a normal
  difference. The test now requires invalid evidence for that malformed success.
- Final GREEN: 123 passed, test exit `0`, wrapper exit `0`. One existing Pydantic
  class-config warning. Includes unchanged Phase 2, byte, news and tape checks.
- Immutable Phase 1 ledger digest:
  `92578257e18b80120fbb87d27d3a4f3ae6e83bc84342ba33bd07aed20d38b80d`.
  Inventory scalar placeholders are replaced with the reviewed paths.
- The engine remains `phase2_meaning.py`; one profile dispatch, one JSON walker,
  typed-map traversal only at reviewed paths, and no new comparator or repr parser.
  Decision 84 validates structured error semantics against the request validator.
- CSV and non-JSON auxiliary bytes stay strict. Tape membership and order stay
  strict; Task 5 must supply the canonical complete-relation page oracle.
- No measurements ran. Next: Task 3 must reuse the existing decision-87 baseline
  and gate, not create another baseline file or refresh Python performance.

### Task 3 checkpoint

- Reused the accepted decision-87 baseline at its existing path. Its bytes remain
  SHA-256 `3c74f7960add211757198149241f1633ef90a165ec89a902ac40bf79c1f8ea5e`.
  No duplicate baseline file or fresh Python measurement was created. Historical
  source provenance remains PM-attested; this lane did not re-open its private raw
  source on the production test host.
- Successor policy is explicit: `frozen_verdict(..., successor=True)` and Rust
  `meaning_performance_gate`. Existing first-cutover defaults retain their policy.
  Successor error rates block at both sizes, exact cross-products allow unequal
  offered totals, positive lower rates are valid, transport failures block, and
  the full sample floor remains required. Large speed remains a fix list.
- Python RED: 9 failed, 30 passed; test/wrapper exit `1`. GREEN with measurement
  unit tests: 131 passed; test/wrapper exit `0`. No measurement process started.
- Rust RED: missing successor API, cargo/wrapper exit `101`. GREEN: 38 tests
  passed across meaning, frozen and legacy gate suites; cargo/wrapper exit `0`.
- Initial fmt command lacked `--all` (exit `1`). Corrected check found import
  ordering differences (exit `1`); host formatting and final check exit `0`.
- Successor bundle requires digest-checked both-size correctness artifacts and
  actual JUnit case coverage; caller booleans cannot qualify a release. Known
  failures take precedence over incomplete evidence. Task 6 must produce the
  declared correctness artifact fields and canonical tape proof.
- Task 2 CI completed: all five required jobs passed; publish skipped.
- Skipped measurements: all latency, memory, CPU, rate/capacity and probes under
  decision 96. No release verdict or production permission is claimed.

### Task 4 partial checkpoint — blocked, not accepted

Task 3 is committed as `5d3704ca`. Its CI completed with all five required jobs
passing and publication skipped. Task 4 edits remain uncommitted and unpushed.

- Added a shared arbitrary-precision `Amount` implementation and domain product/sum
  operations, moved voyage/validation consumers to it, and replaced map-specific
  digit arithmetic. Retained 28-digit intermediate economic rounding and separate
  digest canonicalization. This is not yet the complete Task 4 cleanup.
- Amount RED: cargo/wrapper `101` (missing amount module). First GREEN: `101`
  (test expected an inconsistent simple spelling). Second GREEN: `101` (old voyage
  string equality rejected value-equivalent distance spelling). After routing
  those assertions through the shared Python comparator, the amount, market,
  price and voyage suites passed 33 tests; cargo/wrapper `0`.
- Structured tool-validation diagnostics now preserve kind/location/message/input
  as JSON. Protocol diagnostics are still exact and have not been removed.
  Existing diagnostic spelling assertions failed as expected (`101`). A semantic
  diagnostic bridge passed rejected-input comparisons but still failed accepted
  normalized-string equality (`101`). The next edit moved positive normalization
  checks to the pinned Python model; that edit is **not verified**.
- Last call: `work/task4-diagnostic-final.log`, `CALL_EXIT=255`, no
  `DIAGNOSTIC_GREEN_EXIT` marker. Compilation finished; the test did not report a
  result before the connection ended. Do not call this a test pass or infer a
  test exit from the wrapper. No call was interrupted and no extra host call ran
  concurrently from this lane.
- A subsequent light read-only process check showed no lane systemd unit,
  `mcp_validation`, `compare_values`, or `heavy run` process remaining.
- No measurements or live application changes ran. No Python application source
  was changed. Do not push the partial Task 4 tree.

### Task 4 resumed checkpoint

PM confirmed the `255` was a lost SSH connection, not a test hang. The wrapper
now has keepalive. The required one-time rerun passed: `MCP_INPUT_EXIT=0`,
`CALL_EXIT=0`. No bridge diagnosis or time-limit increase was needed.

- Checkpoint `8962a36b` preserves the exact amount operations and semantic bridge.
- Merged comparator-owner branch at `e718c97d` as `51f495bf` before further
  comparator work. Both profiles and the unified byte/live API remain present.
  Merge regression: 182 passed; test/wrapper `0`. No `app/` or `alembic/` change.
- Removed the obsolete MCP decimal module (352 lines), product spelling helper
  (64 lines), map digit reducer duplication and envelope key-order builders.
  The new amount module retains significant-digit intermediate rounding because
  tests prove it changes economic values. Digest canonicalization remains separate.
- Kept protocol diagnostics and ordinary error repr helpers: decision 84 applies
  only to the reviewed tool argument paths, not every error string.
- Protocol/arithmetic GREEN: 46 tests passed; `TASK4_GATE_EXIT=0`, wrapper `0`.
  Additional map/arithmetic GREEN: 38 tests passed; `TASK4_FINAL_EXIT=0`, wrapper
  `0`. Host formatting `FORMAT_EXIT=0`. Earlier cleanup runs returned `101` for
  obsolete spelling assertions and one plain-text fixture parsed as JSON; semantic
  assertions and strict plain-text comparison now cover those cases.
- `preserve_order` remains: ordered schema/digest consumers are still present.
  No dependency was removed without proof that its semantic consumers were gone.
- All measurements remain skipped under decision 96. No tests ran on the Mac.

New PM scope authorizes Tasks 5–8, but forbids edits under `app/` and `alembic/`.
Task 7 explicitly requires an Alembic-owned migration. That conflict blocks Task 7
acceptance until the PM assigns or authorizes the migration. Task 8 also requires
Task 7 plus reviewed Task 9/10 route handoffs. Do not activate trading caches or
invent a second schema/deployment path to bypass those dependencies.
Tasks 5–6 have not started at this checkpoint.

### CI checkpoint after Task 4 cleanup

- Pushed cleanup `cec750dd`, format correction `b84837d1`, and test-lint
  correction `0bc77829` only to `feat/rust-same-meaning`.
- Latest CI: offline passed (workspace tests, fmt, clippy and build); backend test,
  postgres-phase0 and postgres-analytics passed. Contract failed, step exit `1`;
  publish skipped. CI watch exit `1`. All jobs have finished.
- The contract job still runs the historical byte runner. It rejects the intended
  decimal spelling and MCP presentation changes in orderbook_map, prices and MCP.
  Run `39151343120` records those failures. Do not weaken the historical byte gate
  or describe this CI as green. Task 6 owns explicit successor runner selection;
  it must land before this branch can pass that job without reverting the cleanup.
- Automated post-commit security review did not complete (API HTTP 404); its local
  details are in `/Users/jonathanjie/.claude/security/log.txt`. No independent
  security-review success is claimed.

PM accepted Task 4 and authorized draft contract CI to remain red until Task 6.
This lane owns Task 7's additive migration, literal checkpoint and exact ACL rows.
Python route/service changes still require a stop and PM review. Production remains
at `p0_20261016_union`; no live migration or deployment is authorized by this work.

### Task 5 checkpoint

Pushed `7b134ae1`: bounded public reference payload cache, canonical tape SQL tuple
order, and test-only complete-relation Python page oracle. Cache keys are typed;
validation runs before news lookups, errors are not inserted, headers are rebuilt,
and each AppState owns its policy/environment/schema-bound cache. Trading caching
remains absent.

- RED Rust exit `101` (missing cache API); Python exit `2` (missing oracle module).
- GREEN: 16 Rust cache/simple-read tests, `CACHE_GREEN_EXIT=0`; 26 Python
  oracle/comparator tests, `ORACLE_GREEN_EXIT=0`; wrapper `0`. Format exit `0`.
- CI on `7b134ae1`: offline, backend, postgres-phase0 and postgres-analytics passed.
  Byte contract failed, step/watch exit `1`, under the PM-approved temporary
  exception. Publication skipped. All jobs finished.
- Remaining acceptance evidence belongs to Task 6: populated canonical page
  comparisons and both-KYC meaning groups. Current unit tape tests cover 405 tied
  rows, nonzero offsets, repeated pages, full UUID prefix collisions and duplicate
  source refusal, but do not establish complete database filter/plan coverage.

### Task 6 partial checkpoint — not accepted

Uncommitted successor plumbing adds explicit profile/digest recording comparison
and a test-only canonical tape response builder. The historical default byte path
is unchanged. CI workflow selection and corpus replacement are not yet complete.

- RED runner tests: exit `1`. GREEN: 43 passed, `RUNNER_GREEN_EXIT=0`, wrapper `0`.
  The first GREEN failed because a synthetic test omitted the captured exchange
  body/digest; it now constructs the complete capture.
- First full KYC-on run: `MEANING_ON_EXIT=1`, wrapper `1`; empty HEAD and non-JSON
  method responses incorrectly reached JSON parsing. Successor transport bodies
  now remain exact bytes; Phase 2 rules stay unchanged.
- Second full KYC-on run: `MEANING_ON_EXIT=1`, wrapper `1`. Nine groups passed;
  `simple_reads` failed `duplicate-list:/body:value`, and `prices` failed
  `fixture-0:/body:value` and `fixture-1:/body:value`. Every group completed and
  the runner cleaned up. No KYC-off run was started after this failure.
- Price diagnosis: the declared collection tie path is `/prices`, but the source
  DTO uses `/summaries`. Correct the successor ledger through the shared owner
  boundary, update its pin and tests, and never broaden arbitrary arrays.
- Simple-read diagnosis is pending: check whether its test fixture mutates port
  intelligence after an earlier cache fill, and whether the test needs explicit
  cache clear/expiry evidence. Do not hide changed fields or relax port ordering.
- Task 7 has not started. No measurement ran. No Python app or Alembic source was
  changed. All host and CI runs have finished; no active run remains.

### Resumed Task 6 evidence

- `56567679` selects the explicit successor runner in CI and propagates the
  reviewed profile/digest. `bc244657` records the successor corpus and architecture.
- Simple-read mismatch was a test mutation after warming a legal 60-second
  reference cache. The duplicate fixture now exists before any read; all 49
  existing requests plus duplicate list/detail remain checked. Target exit `0`.
- Price collection path corrected to the actual DTO `/summaries`; populated target
  exit `0`. New ledger pin is
  `f68125dd74d59f0af49f120b839d8e3cdadef1deca8027f9d0f08fc8a0b99e0e`.
- Full KYC enabled: `MEANING_ON_EXIT=0`, wrapper `0`; full KYC disabled resumed
  after the session interruption: `MEANING_OFF_EXIT=0`, wrapper `0`. All 11 groups
  passed in each run. The interrupted earlier disabled run is not counted.
- Runner/comparator regressions: 114 passed, exit `0`; corpus coverage: 14 passed,
  exit `0`. Both corpus recordings: `TOTAL_UNSTABLE=0`, `RECORD_FAILURES=0`,
  recording/wrapper exit `0`. Each state retains all 487 requests.
- Both tape recording legs now use the canonical complete-relation page builder;
  ordinary unchanged-Python pages cannot establish tie membership. The first
  recording refused publication for three tape mismatches and one curve timeout;
  its failed evidence remains in `work/`.
- CI passed all five required jobs on both `56567679` and `bc244657`; publish skipped.
- Historical byte bodies are archived at `work/historical-byte-corpus.tgz`, SHA-256
  `dde43542e10d6cee94950b09c8baf774019002dcfb344786492179f3810a4fe7`.
  Exact deletion inventory has 1,948 bodies. They are not deleted yet: policy export
  and its tests still read them. Do not break those consumers or claim full cleanup.
- PM accepted Task 6 at `56567679` and `bc244657`. Task 11 owns these named
  mandatory cleanup items; Task 6 is not reopened for them:
  - **SM-11-BYTE-CONSUMERS:** migrate policy-export and test consumers, then remove
    only the reviewed obsolete byte-body inventory; preserve historical evidence.
  - **SM-11-SHADOW-PROFILE:** finish explicit successor profile/digest support for
    shadow and staging reads; absent canonical live snapshot proof is INCONCLUSIVE.
  - **SM-11-CANONICAL-REPLAY:** bind tape evidence to an immutable snapshot/source
    multiset and retain canonical proof in replay; never sort Python pages or
    suppress membership differences.

### Task 7 partial infrastructure checkpoint

Pushed `852d8760`: additive checkpoint `prc_20261018_cache` after
`p0_20261016_union`, three metadata tables, restricted transaction-owned generation
triggers, exact runtime SELECT ACL rows, declaration-only models and disabled
Rust authority/fence primitives. No Python handler/service changed. No live DDL.

- PostgreSQL RED: two failed, exit `1`. First GREEN migration failed, exit `1`,
  because asyncpg refuses two commands in one prepared statement. Split function
  creation and REVOKE. Corrected metadata/rollback/ACL plus Alembic checks: four
  passed, `CACHE_DB_GREEN_EXIT=0`, wrapper `0`.
- Rust authority/reference cache checks: five passed, `AUTHORITY_EXIT=0`, wrapper
  `0`; host formatting exit `0`.
- CI `39172005757`: backend unit and both PostgreSQL jobs failed (exit `1`);
  `39172005678`: offline and successor contract passed, publish skipped. All jobs
  finished. Unit/market migration tests still pin head to `p0_20261016_union`;
  Phase 0 builds a historical union fixture then runs `alembic check` against the
  newer source head, causing `Target database is not up to date` (exit `255`).
  Do not change historical Phase 0 identity to hide this; separate historical
  fixture checks from current-head drift checks.
- Task 7 is NOT complete: listener delivery, external authority activation,
  old-primary pool retirement, two-incarnation failover proof and budget tests
  remain. Trading caching stays disabled. See `cache-dependencies.md`.

Resume: repair migration-head test assumptions without altering frozen Phase 0
schema evidence, complete Task 6 residual consumers/proof, then complete Task 7's
canonical deployment/recovery fencing integration. No measurements ran and no
host or CI run remains active at this checkpoint.

### Task 7 checkpoint — listener, external authority, fencing, failover proof

Migration-head CI repair landed earlier (`2857a68d`, `6299cbfc`; frozen Phase 0
evidence untouched, drift checks not weakened); listener groundwork `68b3d564`.
This checkpoint completes Task 7's infrastructure. Trading caching stays disabled:
nothing starts the listener or the refresh loop and no deploy budgets a listener.

- **External authority.** The activation proof is an owner record outside every
  database (`.runtime-rust/cache-authority.json`), written only by
  `scripts/rust_cache_authority.py` (`revoke`, `activate`, `status`). It revokes,
  waits twice the Rust lease, binds the incarnation row to a fresh epoch, primary
  identity and higher revision through the migrator role, then publishes the
  active record; any failure leaves it revoked. Ordinary deploys neither create
  nor rotate it (pinned by test). `scripts/deploy.sh` and
  `scripts/prepare_rust_runtime.py` are deliberately unchanged: the release
  fixture copies a fixed script list, and authority rotation belongs to the
  recovery owner, so the runbook section in
  `docs/runbooks/integrated-migration-cutover.md` is the integration point.
- **Fencing.** `CacheAuthority` grants a hit or fill only under the record, a
  5 s lease re-read every second, the serving connection's own identity, a
  listener bound to that identity and an unchanged fence counter. A stuck process
  loses its hits by itself, so no acknowledgement or routing removal is needed.
  This replaces the plan's acknowledge-or-lose-route step; safety rests on the
  lease. Generation rollback inside one authority revokes everything.
- **Dependency ledger.** Closed-world test over Rust queries versus the migrated
  table list with reviewed exclusions; per-table, per-statement trigger tests;
  real Python writers (place, amend, cancel, publish, cross); concurrent writers
  get contiguous generations. Triggers are now `ENABLE ALWAYS` and the listener
  refuses to attach unless all sixteen keep that exact shape. The migration takes
  `SHARE ROW EXCLUSIVE ... NOWAIT` locks first.
- **ACL.** The deployed application role reads the three cache tables, has no
  INSERT, UPDATE, DELETE or TRUNCATE on any, cannot disable or drop a trigger or
  set `session_replication_role`, and its business statement and the generation
  append share one transaction.
- **Budget.** `budget.json` gains an optional `cache_listener` (0 or 1), absent
  and byte-identical when 0; Python and Rust both count it and require the whole
  total to fit. `CacheEvents::connect` refuses an unbudgeted listener before it
  connects. The compiled revision allowlist now accepts `prc_20261018_cache` and
  keeps both earlier checkpoints. A test starts against a schema without the
  checkpoint and finds nothing to serve.
- **Failover proof** on two migrated incarnations (`cache_failover_postgres.rs`):
  promotion with a reused generation, restore with copied metadata, old-primary
  connection and listener, stale fills with equal, ahead and rolled-back
  generations at both insertion and response, uncertain authority (lease lapse,
  missing, malformed and symlinked records, mismatched metadata) and missing
  generation infrastructure. Listener loss and reconnect catch-up are proved in
  `cache_events_postgres.rs`.
- **Not measured.** The single generation row serializes writers and can deadlock
  with a later lock; no measurement ran (decision 96). See
  `docs/rust-phase1/cache-dependencies.md`.
- **For Task 8.** Remaining there: start the refresh loop and listener under an
  explicit switch (passing `--cache-listener 1`), clear the reference cache when
  `current_fence()` changes, commit-safe hit checks in routes, time boundaries.
  Tasks 9 and 10 need measured dispositions and so PM measurement authority.

Resume: Task 8 starts only after Tasks 9 and 10 hand off reviewed uncached route
commits; their acceptance needs large-data measurements the brief has not
authorized.

### Task 7 verification of the listener, authority, fencing and budget commits

Gates ran on the staging test host against `54d98df8` (the four Task 7 commits
plus one lint correction). Trading caching stays disabled.

- Host formatting exit `0`; workspace clippy exit `0`; package clippy with
  `contract-tests` exit `0`; workspace tests exit `0`.
- Python unit suite: 2,447 passed. Two `test_assert_host_git_owner` cases fail
  only on this host because `/etc/systemd/system/verdaxis-backend.service` is a
  symlink to `/dev/null`; the suite passes with those host-artifact cases
  deselected, exit `0`. CI runs the same cases without that host file.
- Disposable PostgreSQL suite (`tests/postgres`, including the cache event and
  two-incarnation failover cases that start the Rust suites): 296 passed, one
  failure in `test_fee_downgrade_fails_fast_during_idempotent_trade_read`. That
  case upgrades only to `kycd_20261015_staging`, before this migration, and it
  passed alone in 27.85 s; the full run took 21:54 on a busy host and exceeded
  the case's five-second refusal window. Not a schema regression.
- No measurement ran. No deploy, live DDL or application change ran.

CI blocker: after the push, GitHub created no workflow run for `54d98df8`.
Pull request 90 is `CONFLICTING` with `staging`, and GitHub skips
`pull_request` workflows for a conflicting merge ref. The conflicts are a union
of staging's `p0_20261016_union` allowlist and atomic budget write with this
branch's cache checkpoint and optional listener count, in
`rs/read/src/runtime/artifact.rs`, `rs/read/tests/runtime.rs` and
`scripts/prepare_rust_budget.py`. Resolving needs a merge of `staging` into this
branch; that changes branch topology, so this lane stopped for a PM decision
instead of doing it unasked.

### Task 8–10 correctness gate record after the staging merge

Evidence tree `e2fe7dfe` (the branch after `origin/staging` was merged, which
brings PR 95 and the populated Task 4b corpus). Every run used the reviewed
successor profile `phase1-meaning-v1` with rules digest
`f68125dd74d59f0af49f120b839d8e3cdadef1deca8027f9d0f08fc8a0b99e0e`, the same
profile the CI contract job runs. No measurement, deploy, live DDL or route
change ran.

| Plan task | Contract group | Cases | Exit |
| --- | --- | ---: | ---: |
| 8 — ports, catalog, news | `simple_reads` | 49 | `0` |
| 9 — prices and trade tape | `prices` | 56 | `0` |
| 10a — legacy curves | `curves_legacy` | 12 | `0` |
| 10b — canonical curves and board | `curves_canonical` | 137 | `0` |

The combined gate (`--group all --rust-target all --mixed-caddy
--require-complete-ledger` with the same profile) exited `0`: health 8, echo 2,
simple_reads, live_benchmarks 4, orderbook_snapshot 7, orderbook_lists 28,
orderbook_map 41, prices 56, curves_legacy 12, curves_canonical 137, mcp 143,
and the assertion ledger reported OK.

Without the reviewed profile the strict legacy comparison reports the known
reviewed typed-path differences only: tape item bytes in `prices` ties and
quantity spellings in `orderbook_map`. Those are the differences the successor
profile declares; no route changed in this checkpoint. The merged tree therefore
carries Tasks 8–10 with their populated-corpus gates green. Task 13 route
registration and any phase-gate measurement remain outside this checkpoint.

# Task 13 — Exact routing and unchanged integration

Status: **in progress, not accepted**. Base: `c37f8074`, lane
`feat/rust-phase-1-lane-13`. No live access, push, agents or skills.

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

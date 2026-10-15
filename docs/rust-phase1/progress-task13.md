# Task 13 — Exact routing and unchanged integration

Status: **in progress, not accepted**. Base: `c37f8074`, lane
`feat/rust-phase-1-lane-13`. No live access, push, agents or skills.

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

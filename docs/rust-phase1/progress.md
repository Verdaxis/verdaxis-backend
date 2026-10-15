# Rust Phase 1 progress

## Task 1 — Workspace and compiled identity

- Status: complete; Tasks 2–14 were not started.
- Date: 2026-10-15.
- Base and compiled test identity: `b2ea81459fd50c4598b6bd87a4231a8522307ea7`.
- Implementation commit: the commit that adds this file, titled
  `build: establish Rust read workspace and release identity`.
  Resolve its full SHA with `git log --diff-filter=A --format=%H -- docs/rust-phase1/progress.md`.
  This reference avoids embedding a commit's own hash in its contents.
- Compiler: Homebrew Rust 1.95.0 (`59807616e`, LLVM 22.1.3).
- Cargo: Homebrew 1.95.0 (`f2d3ce0bd`).
- Target triple: `aarch64-apple-darwin` (native default; no Linux build claimed).
- `rustup` is absent. Local Cargo accepted the pinned toolchain file without
  attempting a toolchain download. No system packages were installed.

### Resolved direct dependencies

Exact resolutions are pinned in `rs/Cargo.lock`.

| Crate | Version |
| --- | --- |
| axum | 0.8.9 |
| sqlx | 0.9.0 |
| rmcp | 3.5.1 |
| tokio | 1.53.3 |
| utoipa | 6.0.0 |
| rust_decimal | 1.43.0 |
| serde | 1.0.229 |
| serde_json | 1.0.154 |
| tracing | 0.1.44 |
| tracing-subscriber | 0.3.23 |
| tower | 0.5.3 |
| tower-http | 0.7.1 |
| uuid | 1.28.0 |
| chrono | 0.4.45 |
| chrono-tz | 0.10.4 |
| sha2 | 0.11.1 |
| reqwest | 0.13.5 |

### Commands and results

All commands ran from this worktree, in the foreground. `cargo` below means
`/opt/homebrew/bin/cargo`. Build commands used
`PATH=/opt/homebrew/bin:$PATH`, `RUSTC=/opt/homebrew/bin/rustc`, and a fresh
`export RELEASE_SHA=$(git rev-parse HEAD)` before each invocation.
Temporary logs are in ignored `work/rust-phase1/`.

1. `cargo --version`, `/opt/homebrew/bin/rustc -vV`, and `command -v rustup`:
   versions and target recorded above; no rustup found.
2. Added the planned identity test before any workspace implementation.
   `cargo test --manifest-path rs/Cargo.toml -p verdaxis-read --test identity`:
   **expected failure**, exit 101:
   `error: manifest path \`rs/Cargo.toml\` does not exist`.
   Evidence: `work/rust-phase1/identity-red.txt`.
3. Ran every dependency command below in plan order. All returned exit 0:

   ```sh
   cargo add --manifest-path rs/read/Cargo.toml axum@0.8
   cargo add --manifest-path rs/read/Cargo.toml sqlx@0.9 --no-default-features --features runtime-tokio,postgres,rust_decimal,chrono,uuid,macros
   cargo add --manifest-path rs/read/Cargo.toml tokio --features full
   cargo add --manifest-path rs/read/Cargo.toml serde --features derive
   cargo add --manifest-path rs/read/Cargo.toml serde_json --features preserve_order
   cargo add --manifest-path rs/read/Cargo.toml tracing
   cargo add --manifest-path rs/read/Cargo.toml tracing-subscriber --features json,env-filter
   cargo add --manifest-path rs/read/Cargo.toml tower --features util
   cargo add --manifest-path rs/read/Cargo.toml tower-http --features cors
   cargo add --manifest-path rs/read/Cargo.toml uuid --features serde,v4
   cargo add --manifest-path rs/read/Cargo.toml chrono --features serde
   cargo add --manifest-path rs/read/Cargo.toml chrono-tz rust_decimal sha2 utoipa
   cargo add --manifest-path rs/read/Cargo.toml rmcp --features server,transport-streamable-http-server
   cargo add --manifest-path rs/contract/Cargo.toml serde --features derive
   cargo add --manifest-path rs/contract/Cargo.toml serde_json --features preserve_order
   cargo add --manifest-path rs/contract/Cargo.toml reqwest --features blocking,json
   ```

4. `cargo fetch --manifest-path rs/Cargo.toml --locked`: **pass**, exit 0.
   Inspected local source declarations for the bootstrap types.
5. `cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read --test identity`:
   **pass**, 1 test, 0 failures. SQLx and rmcp compiled on Rust 1.95.
6. `cargo check --manifest-path rs/Cargo.toml --locked --workspace`:
   **pass**, exit 0.
7. `cargo fmt --manifest-path rs/Cargo.toml --all`, then
   `cargo fmt --manifest-path rs/Cargo.toml --all -- --check`:
   **pass**, exit 0.
8. `cargo clippy --manifest-path rs/Cargo.toml --locked --workspace --all-targets -- -D warnings`:
   **pass**, exit 0.
9. `cargo test --manifest-path rs/Cargo.toml --locked --workspace`:
   **pass**, 1 identity test, 0 failures; other targets and doc tests contain no tests.
10. `cargo metadata --manifest-path rs/Cargo.toml --locked --no-deps --format-version 1`:
    **pass**, exit 0.
11. `git diff --check`: **pass**.

### Scope and deviations

- Added `/work/` as required and `/rs/target/` to exclude generated build output.
- Updated `ARCHITECTURE.md` as required by the worktree instructions.
- No dependency substitutions or implementation deviations.
- Python tests and HTTP/database contract tests were not run: Task 1 adds only
  the workspace, shared type declarations, identity code, and independent empty
  contract scaffold. No Python files changed and no server was started.
- Codesight refresh was deferred to avoid changing generated bootstrap files
  outside Task 1. Its summaries do not yet describe `rs/`.
- Tool hooks created untracked `.claude/` checkpoint files. They are not part
  of this task and are left untouched and uncommitted.
- No agents, background jobs, live hosts, databases, deployments, or pushes.

### Resume point

Task 2 starts from this implementation commit, after separate authorization.
Set `RELEASE_SHA` to the current full commit SHA and rerun the identity test
before runtime configuration work. The local build proves macOS dependency
compatibility only, not Linux release compatibility or deployment readiness.

## Task 2 — Initial stop (resolved by plan correction 1d52cd9)

- Date: 2026-10-15.
- Base: `8bd4d95` on `feat/rust-phase-1-20261015`.
- Resume check: with a fresh `export RELEASE_SHA=$(git rev-parse HEAD)`,
  `cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read --test identity`
  passed (1 test) on local Homebrew Rust 1.95.0.
- Blocker: Task 2 requires the Python oracle's recorded SQL session timezone
  (`docs/plans/2026-10-15-rust-phase-1-plan.md:412`), but no recorded value was
  found in the current Rust source, progress documents, design, or convergence
  analysis. Task 4 creates the oracle. The current `Config` has no session
  timezone field, and Python's `engine_options` does not set a timezone.
- Stopped before adding tests or implementation, as instructed for a plan gap.
  No timezone was assumed and no later-task work was started.
- The explicit Task 2 instruction takes precedence over stale prose: Rust pool
  maximum is 9; expected budget totals are 61 and 109, not 55 and 103.
- Not run: Task 2 tests, workspace tests, clippy, and rustfmt. No code changed.
  PostgreSQL authority, revision, and successful-readiness tests also remain
  unrun because no local PostgreSQL is running and starting one is prohibited.
- Resume point: obtain the approved recorded SQL timezone, or an approved plan
  correction for how Task 2 obtains it. Then begin Task 2 with failing tests.
  Task 3 remains blocked. No implementation commit, push, live access, agents,
  or background jobs were made.

## Task 2 — Implementation; PostgreSQL acceptance pending

- Resumed from Task 1 `3952534`, after correction `1d52cd9`.
- Implementation checkpoint uses the planned message:
  `feat: add bounded Rust runtime and checkpoint admission`.
- Rust inherits the server TimeZone; startup reads it once and logs
  `database_timezone`. Golden timezone parity remains Task 4, as instructed.
- The compiled revision is `kycg_20261014_staging`. Staging requires
  `verdaxis_app_staging` on `127.0.0.1:5432/verdaxis_staging`; production is refused.
  Pool min/max is 0/9, with 30-second acquisition and read-only session defaults.
- Effective budget tests cover 61/109, changed workers/overflow/reserve,
  independent configured/observed limits, and integer overflow.
- Preparation CLI: `python scripts/prepare_rust_budget.py --environment-file PATH
  --release-sha FULL_SHA --output NEW_PATH`. Add `--verify` for prestart drift
  checks. Output creation is exclusive and mode 0600. Inputs use no-follow
  regular-file reads. Settings run in an isolated child with no ambient values.
- Staging `connect` requires `RUST_PREPARED_BUDGET` containing that artifact's
  JSON, validates its release and arithmetic against Config, and checks observed
  capacity. Task 12 owns passing this metadata and invoking verification under
  the deployment lock. Rust never reads `.env` or opens migrator credentials.

### Evidence and checks

All Cargo commands used local Rust 1.95 and a fresh
`export RELEASE_SHA=$(git rev-parse HEAD)`. Commands ran in the foreground.

- Resumed identity test: pass (1 test).
- Runtime test first failed with missing exports (exit 101), then passed.
  Evidence: `work/rust-phase1/runtime-red.txt`.
- Prepared-budget test first failed with the missing validator (exit 101).
  Evidence: `work/rust-phase1/prepared-budget-red.txt`.
- Budget helper tests first failed because the helper did not exist (exit 1).
  Evidence: `work/rust-phase1/budget-red.txt`.
- `cargo test --manifest-path rs/Cargo.toml --locked --workspace`: pass,
  1 identity test plus 7 runtime tests.
- `cargo clippy --manifest-path rs/Cargo.toml --locked --workspace --all-targets
  --all-features -- -D warnings`: pass, including contract test compilation.
- `cargo fmt --manifest-path rs/Cargo.toml --all -- --check`: pass.
- `cargo test --manifest-path rs/Cargo.toml --locked --features contract-tests
  --test runtime_postgres --no-run`: pass (compile only).
- `work/rust-phase1/venv/bin/python tests/unit/test_rust_budget.py`: pass, 3 tests.
  Initial execution exposed missing isolated Python packages, then a missing
  RELEASE_SHA in the synthetic fixture. Both were corrected. Temporary Python
  packages are in `work/`; no system packages were installed.
- `git diff --check`: pass.

### Remaining gates and scope

- PostgreSQL tests were not executed: no local server exists and starting one
  was prohibited. No database or live host was contacted. The feature-gated
  tests cover role membership, session limits, max_connections, health bodies,
  app denial on alembic_version, and zero/multiple/singleton revision rows using
  the unchanged migrator verifier. They require pre-provisioned disposable roles
  and `RUST_TEST_DATABASE_URL`, `RUST_TEST_MEMBER_DATABASE_URL`,
  `RUST_TEST_MIGRATOR_DATABASE_URL`, and an absolute `RUST_TEST_PYTHON`.
- Full Python unit suite was not run: no project `venv` is present. Only the new
  standalone budget tests ran in the temporary environment.
- Added a separate contract target and Python helper tests to keep offline tests
  independent of PostgreSQL. Added direct URL/percent-decoding dependencies and
  the target manifest entry. These support Task 2 only.
- Updated ARCHITECTURE.md. Codesight refresh remains deferred as in Task 1;
  generated summaries are not acceptance evidence.
- Task 2 is not fully accepted until the PostgreSQL tests pass. Task 3 was not
  started. No routes, deployment wiring, pushes, agents, or background jobs.

## Task 3 — Stopped at cancellation ownership gap

- Date: 2026-10-15. Base: `1f155e2`.
- Read the required plan sections, progress, project instructions, architecture,
  Python admission/error/support middleware, and current Rust runtime source.
- No implementation or tests were added. The source review found a gap in the
  cancellation guarantee at plan lines 495 and 535, before the test-first step.
- The plan gives REST a request/body-owned semaphore permit and gives MCP tools
  an owned permit. It requires database work to stop before capacity returns.
  The current `AppState` exposes a plain `PgPool`; neither it nor the proposed
  permit interface binds connection cleanup to the permit.
- In resolved SQLx 0.9.0, `sqlx-core/src/pool/connection.rs:199-209` implements
  `PoolConnection::drop` by spawning `return_to_pool`. That cleanup awaits a
  ping at lines 307-325. In `sqlx-postgres/src/connection/mod.rs:180-187`, ping
  sends Sync and waits for ReadyForQuery; lines 94-107 drain pending responses.
  Dropping the query future therefore does not prove server-side cancellation
  or completed connection return. Dropping the separate admission permit can
  restore capacity while that cleanup still runs.
- A response-body wrapper covers streaming lifetime but does not cover this
  earlier handler-cancellation case. The plan's pure semaphore test cannot
  establish the required database guarantee.
- Resume point: approve a shared query/connection ownership contract for REST
  and MCP that retains admission through database completion and pool cleanup,
  including caller cancellation. Define that interface before later route
  tasks use the plain pool. Then start Task 3 with failing tests, including the
  feature-gated `pg_sleep` regression. No ownership redesign was implemented.
- Checks not run: cargo test, clippy, and rustfmt; no Rust files changed.
  PostgreSQL cancellation and health corpus tests were not run because local
  PostgreSQL execution is prohibited. Task 2 PostgreSQL acceptance remains
  pending; the current user request authorizes Task 3 work, not those runs.
- This is a stop report, not Task 3 completion. No empty route modules or MCP
  router were added. No later task, database access, live access, push, agent,
  or background job was started. The existing untracked `.claude/` is untouched.

## Task 3 — Ownership correction received; CSV stop condition reached

- Date: 2026-10-15. Resumed after plan correction `d6352e6`, with rebased
  Task 1 `132f57e`, Task 2 `2097fab`, and the prior stop note `4f716e3`.
- Read decisions 11 and 12. These resolve the prior ownership design question
  with two pools (read 8, health 1) and task-owned admitted work. Disposable
  PostgreSQL through Colima is now authorized under decision 12; the earlier
  prohibition recorded above is historical, not the current permission state.
- Stopped before tests or implementation at decision 11's explicit condition:
  "If an export has no row bound in Python, stop and report."
- `GET /api/prices/reference/export` has no row bound:
  `app/routers/price_discovery.py:613-614` makes both dates optional;
  lines 630-641 pass them to `compute_reference_prices` and build the CSV from
  the complete returned list. Date validation at lines 106-114 only rejects
  reversed dates. The query at lines 443-492 applies optional filters, groups
  and sorts, but has no LIMIT; `result.all()` collects all matching rows.
  No maximum date span or output-row cap is enforced by this path.
- No row cap, truncation, new date requirement, or streaming redesign was added.
  Each would need an approved contract decision rather than an inferred fix.
- Checks: `git diff --check` passed before the documentation commit. Cargo test,
  clippy, and rustfmt were not run because no Rust files changed. Task 2
  PostgreSQL tests and Task 3 cancellation tests were not run: the explicit
  stop condition was reached before implementation or container startup.
- Resume point: approve how the unbounded reference-price export fits the
  task-owned CSV contract, or explicitly defer that export decision while
  authorizing the remaining Task 3 work. Then begin with failing tests.
- Task 3 remains unimplemented. No container, database connection, live access,
  push, agent, or background job was started. `.claude/` remains untouched.

## Task 3 — Implemented; HTTP, admission and cancellation gates pass

- Date: 2026-10-15. Base: `11ba285` (the stopped Astra session's partial work).
- Read the plan's Task 3 in full, decisions 9-12, the inventory details, the
  Python admission/support/CORS/rate-limit sources, and this worktree's
  Starlette 1.3.1 `routing.py`, `middleware/cors.py`, and `app/main.py`.
- Kept the WIP's two-pool split, `run_admitted`, empty route modules, and
  `mcp_router`. Fixed what did not match the plan; added every missing test.

### Implementation

- `runtime/admission.rs` exports `ADMISSION_SLOTS = 8`, `selected_read`,
  `try_read`, and `run_admitted`. `run_admitted` reuses the request's task-local
  permit when the admission middleware already took one, otherwise acquires its
  own (the MCP path). The spawned task owns the permit and the pooled
  connection; on success it pings, runs SQLx's drop handler eagerly through the
  `#[doc(hidden)]` `PoolConnection::return_to_pool`, drops the connection and
  only then the permit; on failure it closes the connection.
- `runtime/http.rs` composes, from outside inward, request-id logging, CORS,
  admission, the support-context and rate-limit boundary, then routing. CORS
  reproduces Starlette's simple and preflight responses, including the exact
  `Disallowed CORS ...` bodies, `Vary`, credentials, expose headers, and the
  absent `Allow-Origin` for a denied origin. Price limits are one fixed window
  per resolved client IP and route (`60/30/10`), with no invented `Retry-After`.
- Health GETs now use axum's `get`, because Starlette adds `HEAD` to every GET
  route. `HEAD /health/live` is 200 with an empty body; other methods are 405
  with `Allow: GET, HEAD` and the Python `{"detail":"Method Not Allowed"}` body.
- `lib.rs` sizes the semaphore from `ADMISSION_SLOTS`; `config.rs` gained
  `public_read_slots(workers, pool, overflow)` and the prepared budget now
  refuses to start unless `workers * (pool + overflow - 1) == 8` (AD-02).

### Checks

All Cargo commands used local Rust 1.95 with a fresh
`export RELEASE_SHA=$(git rev-parse HEAD)`. Foreground only.

- `cargo test --manifest-path rs/Cargo.toml --locked --workspace`: **pass**,
  24 tests (http_contract 15, runtime 7, identity 1, request_log 1), 0 failures.
- `rs/read/tests/run_postgres.sh`: **pass**, exit 0, 29 tests in the workspace
  with `--features contract-tests`, including admission_postgres 1 and
  runtime_postgres 4. The script pins
  `postgis/postgis:17-3.6-alpine@sha256:49b4d46c...b7272`, binds a
  Docker-assigned loopback port, and removes its container on exit; no container
  was left behind and no deployed database was contacted.
  Evidence: `work/rust-phase1/postgres-final.txt`.
- The cancellation test starts eight concurrent `pg_sleep` reads, asserts the
  ninth request gets the unchanged 503 plus `Retry-After: 1`, cancels all eight
  clients, asserts the database work continues and all eight slots stay taken,
  waits for connection return with fewer than ten app-role connections
  throughout, then runs eight fresh requests that all succeed inside 20 seconds
  (the pool acquisition timeout is 30 seconds).
- `cargo fmt --all -- --check` and
  `cargo clippy --locked --workspace --all-targets -- -D warnings`: **pass**.
  `cargo clippy ... --all-features` also passes, compiling the contract targets.
- `git diff --check`: pass. Task 2's PostgreSQL acceptance is now satisfied.

### Deviations and notes for later tasks

- The captured-log test lives in `rs/read/tests/request_log.rs`, not
  `http_contract.rs`. `tracing` caches a callsite's interest the first time it
  is hit; a shared test binary that reaches the request-log callsite before any
  subscriber is installed never delivers the event, which made the assertion
  order-dependent. A one-test target is deterministic.
- The `Allow` value on 405 is `GET, HEAD`. Python joins an unordered method set,
  so the recorded order is unstable between processes; Task 4 must record it.
- `app/middleware/market_support_scope.py` also has a
  `STATE_CHANGING_GET_NOT_ALLOWED` 405 branch. It fires only for the four paths
  in `KNOWN_STATE_CHANGING_GET_PATHS`, none of which Rust serves, so the port
  implements only the 403 context branch. Re-check when a route task adds one.
- `try_read` refuses with the busy contract when the limiter is absent; Python
  also fails closed when its capacity is zero. Rust cannot reach zero capacity
  because the prepared budget refuses to start unless the slots equal eight.
- Rate-limit counts stay outside golden and shadow parity as AD-01 requires;
  only exact limits, reset boundaries, client and route keying, and the 429
  envelope are asserted.
- Data routes remain unmounted, so `/api/prices` and `/mcp` are plain 404s.
  That is Task 13's assembly, and it is why corpus tests must fail honestly
  until then. Trailing-slash redirects are implemented only for the mounted
  health paths; `MOUNTED_GET_PATHS` in `runtime/http.rs` is the list to extend.
- Task 3 is complete. No later task, push, live host, deployment, agent, or
  background job. The untracked `.claude/` directory is untouched.

## Task 3 defect fix — PostgreSQL runner masked setup failures

- Date: 2026-10-15. Base: `4897842`.
- Fixed `rs/read/tests/run_postgres.sh`. Its `EXIT` trap ended with a
  successful `docker rm`, and bash replaces the script's exit status with the
  trap's last command status. `env -u RUST_TEST_PYTHON` therefore exited 0
  even though `${RUST_TEST_PYTHON:?}` failed and no test ran.
- The trap now captures `$?` on entry and exits with it after cleanup, and a
  `mktemp` log holds the test output. `RUST_TEST_PYTHON` is required before any
  container starts. A cargo run that exits 0 but executed no test (no
  `test result: ok. N passed` line with N >= 1) now exits non-zero.
- RED: `work/rust-phase1/run-postgres-red.txt` — `env -u RUST_TEST_PYTHON`
  exited 0.
- GREEN: `work/rust-phase1/run-postgres-green.txt` — the same call prints the
  guard message, exits 1, and starts no container.
- The "no test ran" branch is unreachable through the real workspace while its
  tests pass, so it was exercised on two `work/rust-phase1/guard-*.sh` copies
  that replace only the cargo command: zero-test output exits 1, one-test
  output exits 0.
- No container was left behind. No live host, database, push, agent, or
  background job. `.claude/` untouched.

## Task 4 — Disposable oracle and golden comparator (harness built; recording partially deferred)

- Date: 2026-10-15. Base: `b47e197`. Implementation commit:
  `test: record public Python contracts on disposable PostgreSQL`.
- Built the whole Task 4 harness: the strict comparator, the recorder, the
  corpus, the clock manifest, the assertion ledger and checker, the seed
  helper, the one disposable runner, the independent `verdaxis-contract`
  package, and the test-only contract server.
- No live host, database, deployment, push, agent, or background job was used.
  Every database was a disposable Colima container on a Docker-assigned
  loopback port, removed at exit. `.claude/` is untouched.

### Files

- `scripts/rust_contract/{compare,record,seed_corpus,run_disposable,check_assertions}.py`
- `tests/rust_contract/{server,clock,test_compare,test_clock,test_inventory}.py`,
  `tests/rust_contract/{cases,assertions,ordered-object-paths}.json`,
  `tests/rust_contract/golden/`
- `rs/contract/src/{lib,main}.rs`, `rs/contract/tests/{health,echo}.rs`,
  `rs/read/examples/contract_server.rs`
- Modified `rs/read/src/runtime/http.rs` (`router_with`) and both Cargo manifests.

### What is proven

- The comparator unit tests pass (11). Decimal scale, null versus absence,
  object membership, array order, duplicate keys, header presence, exact body
  bytes, and the bounded-read deadline all have a test; each was written before
  its code (RED evidence in `work/rust-phase1/`).
- The clock manifest tests pass (5) and `manifest_gaps() == []`, so every named
  symbol still exists in the imported source.
- The inventory tests pass (4): the corpus covers all 33 planned method/path
  pairs, ids are unique, and the ledger accounts for all 334 discovered Python
  assertions. `check_assertions.py` reports OK.
- `run_disposable` runs end to end. `--group echo` passes (2 cases) and
  `--group health` runs and reports its real differences. The echo run first
  failed on real header differences (`/headers:keys`) and passed after the two
  projections were aligned, which proves the comparator rejects a difference.
- With a fixed clock (`RUST_CONTRACT_CLOCK`) every data group records
  identically twice: `simple_reads` 12, `orderbook_snapshot` 3,
  `orderbook_lists` 10, `orderbook_map` 6, `prices` 9, `curves_legacy` 4,
  `curves_canonical` 6 — all `unstable=0`. Evidence:
  `work/rust-phase1/stability-clock.txt`.
- Golden corpus written for the two green groups under
  `tests/rust_contract/golden/b47e197…/` (10 cases, 40 files).

### Checks

All Cargo commands used local Rust 1.95 with a fresh
`export RELEASE_SHA=$(git rev-parse HEAD)`, in the foreground.

- `cargo test --manifest-path rs/Cargo.toml --locked --workspace`: **pass**,
  24 tests (http_contract 15, runtime 7, identity 1, request_log 1).
- `cargo fmt --all -- --check`: **pass**. `cargo clippy --workspace
  --all-targets -- -D warnings`: **pass**, also with `--features contract-tests`.
- `RUST_TEST_PYTHON=… bash rs/read/tests/run_postgres.sh`: **pass**, exit 0,
  29 tests; the container was removed and none was left behind.
- `venv/bin/python -m pytest tests/rust_contract -q`: **pass**, 20 tests.
- `venv/bin/python -m pytest tests/unit -q`: **19 failed, 1919 passed**. All 19
  are in `tests/unit/test_deploy_health.py` and are pre-existing/ environmental:
  they run `scripts/deploy.sh` under macOS `/bin/bash` 3.2, which lacks
  `mapfile` and cannot infer the environment from a `/private/var` temp path.
  Task 4 changed no deploy code and no file those tests read.

### Rulings

- **`scripts/seed.py` fails on the disposable database.** `python3 -m venv` is
  unavailable (no `ensurepip`), so the harness venv cannot be a true `python3.12`
  environment; the only working interpreter here is Python 3.14. On the
  disposable database the ordinary seed raises
  `CheckViolationError … ck_orderbook_orders_lifecycle` (a `PARTIALLY_FILLED`
  row with `remaining_quantity_mt == quantity_mt`) in `app/seeds/market_seed.py`.
  The failure is deterministic across runs. Ruling: do not patch the app seed
  from Task 4. The recorder, comparator, and runner are done and proven against
  the groups that need no seed; the seeded corpus families stay owned by their
  route tasks. Cost if wrong: the corpus seed must run on an interpreter that
  the app seed supports before Tasks 6–11 can record data cases.
- **MCP transport recording is deferred to Task 11.** The `GET /mcp` streamable
  case holds an SSE event stream open. The recorder now reads every body under
  an absolute per-case deadline, so it cannot hang unbounded, but the MCP
  transport's exact response is Task 11's surface. The MCP corpus cases exist,
  so the inventory still covers all 33 pairs. Cost if wrong: Task 11 must
  finish MCP recording before it can claim parity.

### Findings for later tasks

- Recorded `GET /health/live/` answers **200** in Python (Starlette serves the
  route at the slash path, not a redirect), and Python answers **HEAD** on a GET
  route with **405** and `Allow: GET`. The Rust health routes use axum's `get`,
  which serves HEAD with 200 and redirects `/health/live/` with 307. Task 13's
  exact-routing assembly must reconcile both. Evidence: `work/rust-phase1/health-run.txt`.
- Recording normalizes only the redirect `Location` origin (to `{endpoint}`) so
  the two endpoints stay comparable; every other header and byte is compared.
- Only `/generated_at` fields were ever unstable; the fixed clock removes them.
  Nothing was normalized away to reach stability.

## Task 4 — App seed fix: partial-fill remaining quantity

- Date: 2026-10-15. Base: `900ed61`.
- Root cause, with evidence. `app/seeds/market_seed.py` chose a
  `PARTIALLY_FILLED` order's remaining quantity as
  `round(quantity * uniform(0.2, 0.8) / 500) * 500`, then replaced a value
  `<= 0` with `Decimal("500")`. For a 500 MT lot the rounded value is always 0,
  so the replacement made `remaining == quantity`, which
  `ck_orderbook_orders_lifecycle` forbids for `PARTIALLY_FILLED`
  (`remaining_quantity_mt > 0 AND remaining_quantity_mt < quantity_mt`). A
  1000 MT lot can reach the same state. Per-lot range check: only 500 and 1000
  can produce a value outside `(0, quantity)`.
- It is the seed's own data, not the harness. The formula is wrong in pure
  arithmetic, independent of the interpreter, the fixed test clock, and the
  database. It reproduced on Python 3.12 and 3.14. The CI unit job runs
  `tests/unit` with SQLite and only inspects the seed source; the CI Postgres
  job runs `tests/postgres/test_demo_market_seed.py`, which narrows `PRICING`
  and shifts the RNG stream away from the offending draw. No test seeded the
  full book under a constraint-enforcing PostgreSQL, so the bug was latent.
- Fix, TDD: two failing unit tests in `tests/unit/test_market_seed.py`
  (`test_partial_remaining_stays_strictly_inside_the_lot`,
  `test_partial_remaining_uses_the_lot_grid_or_the_half_lot`) fail with
  `ImportError`. Added `_partial_remaining(quantity)`, which falls back to half
  the lot when the grid draw is not strictly inside, and used it in both the
  BID and ASK branches. The draw count is unchanged, so the RNG stream and all
  other seeded values are unchanged.
- Checks: `tests/unit/test_market_seed.py` 22 passed (20 + 2); full
  `tests/unit` 1921 passed with the same 19 pre-existing `test_deploy_health.py`
  failures; the ordinary seed now completes on a disposable database
  ("Market data seeded successfully.", 267 orders). Evidence:
  `work/rust-phase1/seed-trial2.txt`, `work/rust-phase1/seed-trial.txt`.

## Task 4 — Oracle moved to Python 3.12

- The oracle now runs on `work/rust-phase1/venv312` (CPython 3.12.2), the
  version production and CI use. `venv` points at it; the 3.14 environment was
  removed. `requirements.txt -c constraints.txt` installed cleanly, and the
  former 3.14 deprecation noise is gone (harness tests: 5 warnings, not 30).
- The harness reads `venv/bin/python`, so no code changed for the switch.

## Task 4 — Seeded corpus recorded under the fixed clock

- Base: `775a75c` (the seed fix). The earlier "seed is a blocker" ruling is
  superseded: the ordinary seed now runs, so the seeded groups record.
- `run_disposable.py` gained `--seed-mode {corpus,ordinary}` (default `corpus`).
  `ordinary` runs only `scripts/seed.py`; the corpus fixture families still
  arrive with the route tasks that consume them. Intent is unchanged; the
  default keeps the plan's fail-closed seed procedure.
- Recorded every group twice against one seeded, fixed-clock database:
  `health` 8, `echo` 2, `simple_reads` 12, `orderbook_snapshot` 3,
  `orderbook_lists` 10, `orderbook_map` 6, `prices` 9, `curves_legacy` 4,
  `curves_canonical` 6 — **TOTAL_UNSTABLE=0**. Evidence:
  `work/rust-phase1/record-seeded.txt`.
- Golden corpus rewritten under `tests/rust_contract/golden/775a75c…/`:
  60 cases, 240 files, real seeded data. The previous `b47e197…` corpus was
  removed because the source changed.
- MCP transport recording is still deferred to Task 11.

### Checks (all on Python 3.12)

- Offline `cargo test --workspace`: **pass**, 24 tests.
- `cargo fmt --all -- --check`: **pass**; `cargo clippy --workspace
  --all-targets -- -D warnings`: **pass**.
- `venv/bin/python -m pytest tests/rust_contract -q`: **pass**, 20 tests,
  5 warnings.
- `RUST_TEST_PYTHON=… bash rs/read/tests/run_postgres.sh`: **pass**, exit 0,
  29 tests, container removed.
- `tests/unit`: 1921 passed with the same 19 pre-existing `test_deploy_health.py`
  failures.

## Task 5 — Catalog policy exported as attested data

- Date: 2026-10-15. Base: `d2ad58c` on `feat/rust-phase-1-20261015`.
- Implementation commit: the commit titled
  `feat: load canonical market policy from attested data`. Resolve its full SHA
  with `git log --format=%H --grep 'load canonical market policy'`.
- Read the plan's Task 5 in full, the global constraints, the public-market
  rules, `app/market_catalog.py`, `app/models/catalog.py`,
  `app/routers/catalog.py`, `app/mcp_server.py:50-71`, and the Task 4 corpus.

### Files

- `scripts/rust_contract/export_policy.py`, `deploy/rust/market-policy.json`,
  `deploy/rust/mcp-catalog.json`, `deploy/rust/market-policy.manifest.json`
- `tests/rust_contract/vectors/catalog/{products,points}_{rows,expected}.json`
- `tests/rust_contract/test_policy_export.py`
- `rs/read/src/market/{mod,policy}.rs`, `rs/read/src/lib.rs` (`pub mod market;`),
  `rs/read/tests/policy.rs`
- `ARCHITECTURE.md`

### Decisions

- The policy `source_sha` is the recorded oracle corpus SHA
  `775a75c4dce449b875de5dd21998f1c34f9766aa`, read from the single
  `tests/rust_contract/golden/` directory. Re-recording the corpus forces a
  policy regeneration, and the export test fails on drift.
- MCP-only fields (`unit`, `min_lot_size`, `spec_description`, delivery-point
  names, EU scope) live in a **separate** payload, `deploy/rust/mcp-catalog.json`,
  because Task 5 step 3 offers that option and the last paragraph of that step
  keeps mutable fields in the database while the plan-fixed `ProductPolicy`
  struct stays unchanged. Rust does not load it yet: `list_markets` acceptance is
  Task 11. The Rust policy test only checks that the payload covers the policy.
- `project_catalog` projects product rows or delivery-point rows (they carry
  `fuel_type` or `region`). Rows that fail the strict identity tuple, or are
  inactive, are dropped; the result follows policy display order. Vectors are
  the recorded R03/R04 golden bodies plus three decoys each.
- Added `point_matches` as the delivery-point counterpart of `product_matches`
  (policy identity belongs to Task 5); Task 6 must consume it, not reimplement.
- `validate_policy` and `load_policy` do not change the plan's declared types.

### Checks

All Cargo commands used local Rust 1.95 with a fresh
`export RELEASE_SHA=$(git rev-parse HEAD)`, in the foreground.

- RED: `cargo test -p verdaxis-read --test policy` failed with the three
  expected errors (missing `market` module, two absent artifacts). Evidence:
  `work/rust-phase1/policy-red.txt`.
- RED: `pytest tests/rust_contract/test_policy_export.py` failed on the missing
  `export_policy` module. Evidence: `work/rust-phase1/policy-export-red.txt`.
- GREEN: `cargo test --workspace`: **pass**, 36 tests (http_contract 15,
  policy 12, runtime 7, identity 1, request_log 1), 0 failures.
- `RUST_TEST_PYTHON=… bash rs/read/tests/run_postgres.sh`: **pass**, exit 0,
  41 tests with `--features contract-tests`. Container removed; none left.
  Evidence: `work/rust-phase1/postgres-task5.txt`.
- `cargo fmt --all -- --check`: **pass**. `cargo clippy --workspace
  --all-targets -- -D warnings`: **pass**, also with `--all-features`.
- `venv/bin/python -m pytest tests/rust_contract -q`: **pass**, 36 tests.
- `venv/bin/python -m scripts.rust_contract.export_policy --check`: **pass**.
  Regenerated into `work/rust-phase1/regenerated/` and byte-compared with the
  committed policy, MCP catalog, manifest, and four vector files: identical.
- `tests/unit`: 1921 passed with the same 19 pre-existing
  `test_deploy_health.py` failures recorded in Task 4. Task 5 changed no file
  those tests read.
- `git diff --check`: pass.

### Notes for later tasks

- Task 8 (R03/R04) and Task 11 (`list_markets`) load the policy and the MCP
  payload through `load_policy`/`project_catalog`; they must not re-derive
  identity or hardcode names.
- The EU-scope flag `is_eu` in `mcp-catalog.json` is exported from
  `app.services.voyage_fuel_plan.EU_PORTS`; Task 11 owns its Rust loader.
- Task 5 is complete. PostgreSQL authority and cancellation gates already pass
  from Task 3. No later task, push, live host, deployment, agent, or background
  job. `.claude/` and `work/` remain untracked.

## Task 6 — Shared visibility, decimal, digest, and public projection rules

- Date: 2026-10-15. Base: `239b452` on `feat/rust-phase-1-20261015`.
- Implementation commit: the commit titled
  `feat: preserve public market visibility and reviewed terms`. Resolve its full
  SHA with `git log --format=%H --grep 'preserve public market visibility'`.
  The disposable-runner fixes are the commit titled
  `fix: run the market contract matrix on a migrated disposable database`.
- Read the plan's Task 6 in full, decisions 11-12, the public-market rules
  table, `app/services/{market_data_eligibility,public_order_scope,kyc_execution,
  execution_policy,order_terms,lng_order,fame_order,market_provenance,
  availability_windows}.py`, and `app/schemas/{orderbook,lng_order,fame_order,
  fame,supplier_offer}.py`.

### Files

- `rs/read/src/market/{decimal,windows,provenance,terms,visibility}.rs`,
  `rs/read/src/market/mod.rs`, `rs/read/src/lib.rs` re-exports
- `rs/read/sql/{public_orders,trade_evidence}.sql`
- `rs/read/tests/{market_contract.rs,market_contract_postgres.rs}`,
  `rs/read/tests/market_contract_oracle.py`, `rs/read/tests/run_market_contract.sh`
- `scripts/rust_contract/export_terms_vectors.py`,
  `tests/rust_contract/test_terms_vectors.py`,
  `tests/rust_contract/vectors/terms/{private,non_ascii,fame}.json`
- `rs/read/Cargo.toml` (the `market_contract_postgres` target),
  `rs/read/tests/run_postgres.sh`, `ARCHITECTURE.md`

### Interfaces (the plan's declared names, as implemented)

- `decimal_text(Decimal) -> String`; `digest_terms(&Json) -> Result<String,String>`;
  `redact_terms(profile,&Json) -> Result<Json,String>`;
  `canonical_terms_from_row(&Json) -> Result<Json,String>` (the reviewed-terms
  builder, needed to make the digest testable from a projected row);
  `normalize_window(value, today) -> Result<String,String>` plus
  `window_start_date`, `window_sort_key`, `window_display_label`;
  `public_order_sql()`, `trade_evidence_sql()`;
  `select_public_order_ids`/`select_trade_evidence_ids` with
  `PublicOrderFilters`, `TradeEvidenceFilters`, and `VisibilityClock`;
  `order_is_execution_qualified(policy, &QualificationInput)`.
- `QualificationInput` is exactly the plan's field list.
- **Clock deviation, recorded:** the plan's `order_is_execution_qualified` takes
  no clock, but the ported FAME rule compares `certificate_valid_until` and the
  quality dates against the current Singapore date (`fame_order.py:44-54`).
  `order_is_execution_qualified` therefore reads the process clock and
  delegates to `order_is_execution_qualified_at(policy, order, today)`, which is
  the deterministic entry point for tests and for Task 6a's `now` parameter.
  No rule was dropped. Task 6a and 10b must call one of these two functions and
  must not reimplement qualification.
- **Digest input contract:** `canonical_terms_from_row` takes the order
  projection as JSON text exactly as `executable_order_terms` reads it: UUID and
  timestamp strings, decimal text at stored scale, JSON arrays, and absent
  `lng_terms` left absent. Route code must project those fields before calling
  it; `digest_terms` alone takes the already-canonical payload.

### Committed vectors and digests

`tests/rust_contract/vectors/terms/` (synthetic, so outside the golden layout):

| Vector | SHA-256 digest |
| --- | --- |
| `private.json` (private ASK LNG declaration) | `9066e752d0f5f8d7a4e380467d51d2f646a68421c6665aa52074f71146a7dd41` |
| `non_ascii.json` (non-ASCII and escaping) | `d00d8bc422c929f04d4251334c823ea18a6261530265e7abae92a577972ce29d` |
| `fame.json` (private ASK B100 declaration) | `e43de0af597fd8ceebe05ac19356b384d97ad3db7728a8b94ec1199f877c987a` |

- Rust reproduces `canonical_terms_from_row` byte for byte on all three, so the
  digest matches Python's `order_terms_digest` (Review Focus 2).
- sha2 0.11 returns a hybrid `Array` that is not `LowerHex`; the digest is
  hex-encoded byte by byte. The plan's illustrative `format!("{:x}", …)` line
  does not compile on the resolved versions.
- Public redaction uses an allowlist per profile, not deletion of two known
  secrets. A BID declaration carries no supplier identity, so
  `redact_terms` returns it unchanged; Python returns the same validated BID
  model, and stored BID declarations already carry every pydantic default
  because they are written from `model_dump`. This equivalence is asserted for
  both profiles but is an assumption about stored rows, not a new rule.

### SQL shape

- `public_orders.sql` binds the policy as `$1::jsonb` text and expands
  `jsonb_to_recordset($1->'products')` / `('points')` as CTEs. Product, lane,
  point, window, currency, owner/assisted admission, off-spec, declaration, and
  provenance predicates are expressed through those CTEs, so the statement
  names no product. `$2` is the captured instant, `$3` the Singapore date,
  `$4` the off-spec toggle, then the route filters and the page window.
- **KYC switch note (from the task brief):** production and staging now run
  `KYC_EXECUTION_GATE_ENABLED=false` (Private Beta; staging PR #82, merged at
  `b218ab2`). This branch is based on the older staging `2a7137b`, which has no
  switch, so the ported owner-admission rule has the gate always on, matching
  this branch's Python. **When this branch is rebased on current staging, the
  Rust visibility rule must follow the switch.** Do not rebase now.
- `trade_evidence.sql` admits immutable version-1 snapshots, exact REAL/REAL or
  DEMO/DEMO provenance, formal statuses, and a confirmation instant; it never
  joins mutable order metadata.

### Evidence and checks

All Cargo commands used local Rust 1.95 with a fresh
`export RELEASE_SHA=$(git rev-parse HEAD)`, in the foreground.

- RED: `tests/rust_contract/test_terms_vectors.py` and the two Rust targets were
  written first. Evidence: `work/rust-phase1/market-red.txt`,
  `work/rust-phase1/market-contract-red.txt`. The Rust targets were re-run with
  the new modules temporarily removed to capture the failing state for this
  record; the implementation order in this session was vectors, then modules,
  then tests, so that run reproduces rather than precedes the failure.
- `cargo test --workspace`: **pass**, 52 tests (http_contract 15, identity 1,
  market_contract 16, policy 12, request_log 1, runtime 7), 0 failures.
- `cargo fmt --all -- --check`: **pass**. `cargo clippy --workspace
  --all-targets -- -D warnings`: **pass**, also with `--all-features`. Seven
  `collapsible_if` findings were collapsed with let-chains (edition 2024).
- `RUST_TEST_PYTHON=… bash rs/read/tests/run_market_contract.sh`: **pass**,
  exit 0. The oracle selected 10 of 30 orders (13 with off-spec included) and
  3 of 5 trades; Rust matched both sets exactly, 2 tests. Evidence:
  `work/rust-phase1/market-contract-11.txt`.
- `RUST_TEST_PYTHON=… bash rs/read/tests/run_postgres.sh`: **pass**, exit 0,
  57 tests; the container was removed and none was left behind.
- `venv/bin/python -m pytest tests/rust_contract -q`: **pass**, 42 tests.
- `venv/bin/python -m pytest tests/unit -q`: 1921 passed, 19 failed. All 19 are
  the pre-existing `tests/unit/test_deploy_health.py` failures recorded in
  Task 4 (macOS `/bin/bash` 3.2 lacks `mapfile`). Task 6 changed no file those
  tests read.
- `git diff --check`: pass.

### Deviations and rulings

- **`run_postgres.sh` now names the two matrix tests in `--skip`.** That
  workspace runner creates a hand-made `alembic_version` database with no
  catalog, so the fixture-dependent matrix cannot run there. Naming the tests
  keeps the skip explicit instead of silent; `run_market_contract.sh` runs them
  against their own migrated database. This is a small edit to a Task 3 file.
- **Some matrix rows are unrepresentable in a freshly migrated database.** The
  database refuses them before any read can see them, so they are recorded here
  rather than forced into the matrix: `CANARY` provenance is refused outright;
  `TEST` and `DEMO` provenance are limited to the deterministic seed
  identities; an order's provenance must equal its organization's; a new trade
  requires snapshot version 1, which admits only REAL/REAL and DEMO/DEMO, and
  its snapshot labels must match the referenced product and point. The matrix
  therefore covers REAL, DEMO, UNKNOWN, and TEST orders and REAL/REAL and
  DEMO/DEMO trades. A CANARY read case must be added only if the guard is ever
  relaxed.
- **Ownerless orders are the Python exception, not a Rust addition.** The
  oracle confirmed that an ownerless order in an `APPROVED=None` organization is
  still visible, because Python short-circuits `owner_user_id IS NULL` before
  the organization check. The port keeps that behavior.
- **The ISO 3166-1 alpha-2 list** is embedded in `terms.rs` as the 249 assigned
  alpha-2 codes, generated from `app/reference/iso_countries.py`, because the
  LNG declaration rule rejects unassigned codes.
- No new crate features were added; `contract-tests` is unchanged.

### Resume point

- `order_is_execution_qualified(policy, &QualificationInput)` is frozen; Task 6a
  and Task 10b consume it and must not fork it. Route workers consume
  `market::visibility` and `market::terms` and must not fork the predicates.
- Task 8 (R03/R04) and Task 11 (`list_markets`) still own their own loaders.
- Task 6 is complete. No later task, push, live host, deployment, agent, or
  background job. `.claude/` and `work/` remain untracked.

## Task 6 acceptance repairs — 2026-10-15

Reference: `outputs/research/rust-task6-review-2026-10-15.md` in the parent Verdaxis workspace. PM assigned the repairs to this session; no agents, push or live access.

### Findings and red/green evidence

- M1: Python-generated `decimal_validation.json` proves trailing-zero declarations accepted by Pydantic. `work/rust-phase1/m1-red.txt` failed on `95.500`; normalized-scale validation then passed.
- M2/N1: `public_terms.json` compares whole Python public projections, including `120.00`, signed zero, scientific notation, minimal BIDs, pending evidence and null. `m2-n1-red.txt` failed both tests; `n1-pending-red.txt` exposed missing default results. Rust now fills defaults and preserves wire spelling separately from digest spelling.
- M3: `large_decimals.json` proves 10^29, 29 significant digits, half-even ties, very small values and negative zero. `m3-red.txt` failed before text-based canonicalization. Non-null parse errors now propagate instead of becoming null.
- N2: `windows.json` proves Unicode years, leading-zero labels and year-zero date failure. `n2-red.txt` failed before repair. The Unicode decimal ranges match the pinned Python 3.12/Unicode 15 oracle.
- M4: `m4-red.txt` failed the exact Python listings comparison. Shared relations now expose internal rows and private digest input without pagination. Listings, ordered pages, post-eligibility depth and projected digest comparisons pass.
- N3: `n3-red.txt` failed the named coverage ledger before the missing fixtures were added. The expanded matrix compares 41 stored orders (12 visible) and 6 trades (4 evidence rows). It adds rejected-with-grace, wrong KYC/grace organization, +/- one-microsecond grace, revoked organization, rejected/mismatched admin, admin-KYC-only, expiry equality, typed-invalid-but-shallow-visible, mixed lane filters, DELIVERED and cutoff equality. Guard-refused cases remain explicit rather than disabling database guards. Singapore midnight and future quality dates have pure Python oracle vectors.

### New frozen interface (supersedes the original Task 6 interface)

Use `market::visibility::{OrderSurface, OrderQuery, TradeQuery, order_query_sql, trade_query_sql, bind_order_query, bind_trade_query}`. Constructors accept only a developer-owned `&'static str` tail and return sealed query types. Bind helpers return a SQLx `Query` with shared values bound. Execute on the connection owned by `run_admitted` (or its transaction). Do not call the pool from route code.

- `order_query_sql(tail) -> OrderQuery`: tail reads `eligible_orders`. Bind policy/clock/filter values via `bind_order_query(&query, &policy, &clock, &filters)`; extra binds begin at `$13`.
- `trade_query_sql(tail) -> TradeQuery`: tail reads `eligible_trades`. Use `bind_trade_query(&query, &policy, &filters)`; extra binds begin at `$7`.
- `PublicOrderFilters.surface`: `Orderbook`, `Listings`, or `Candidates`. Candidates are a shared admission base, not final typed benchmark/curve qualification. Tail code owns surface-specific qualification, exact sparse keys, ranking, grouping and pagination.
- Order rows include catalog labels and `private_digest_input`, with exact numeric text. This object is private; construct public DTOs explicitly. Trade rows contain immutable snapshots, not mutable order joins.
- Existing ID helpers remain compatible adapters with deterministic ID ordering, not a substitute for route-specific ordering. Filter `skip`/`limit` apply only to these adapters.
- `QualificationInput` and existing public function signatures remain; captured-clock consumers **must** call `order_is_execution_qualified_at` with `VisibilityClock::at(now).singapore_date`.
- `canonical_decimal_text(&str) -> Result<String,String>` handles digest precision without narrowing. `decimal_text` remains digest-only, never a public wire formatter.

Updated Task 6 Interfaces and Tasks 6a, 7a-7c, 9, 10b Interfaces in the approved plan. Architecture documents the relation composition. Python behavior was not changed. Its large-CI and Unicode/year-zero edge contracts are recorded under Product issues found.

### Coverage limits and merge notes

- Fresh migrations refuse OPEN zero-remaining, wrong typed lanes and some malformed assisted rows; the oracle records guard refusal. CANARY, unregistered TEST, mixed trade and new version-0 trade guards are tested. Registered TEST rows remain in the selection matrix.
- Legacy version-0 history cannot be newly inserted through the current guard. This change proves its insert refusal and retains the explicit version-1 evidence predicate; it does not fabricate migrated legacy history by disabling guards. A historical upgrade fixture remains an integration follow-up.
- Mutable catalog edits after confirmation are checked by the Python oracle inside a rolled-back savepoint. Rust's trade relation has no mutable joins. Public DTO privacy is still owned by the later route tasks; internal projected rows must not be serialized wholesale.
- No Task 8 files or catalog policy were changed. Task 8 may consume `decimal_text` (same signature; now matches Python context rounding), `normalize_window`, `window_start_date`, `window_sort_key`, `window_display_label` (same signatures; corrected edge semantics). New visibility exports live under `market::visibility`; root module registration and Cargo manifests were not changed by these repairs.
- Lesson for later ports: Decimal validation, digest canonicalization and Pydantic wire spelling are three separate contracts. Compare whole Python projections, not just digest bytes or selected fields. Keep surface eligibility separate from ordered pagination. Recorded here rather than editing lessons outside Task 6 ownership.

### Final checks on code commit 7ee865b

With a fresh `RELEASE_SHA=$(git rev-parse HEAD)`:

- Offline workspace: **58 passed**, zero failures.
- Migrated market runner: **2 passed**, 12/41 orders and 4/6 trades selected; listings, filters, projection, depth and cutoff assertions also pass.
- Runtime PostgreSQL runner: **63 passed**, two matrix tests explicitly filtered because they run in the separate migrated runner.
- Python `tests/rust_contract`: **42 passed**, five existing warnings.
- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass. All-features clippy also passed before the final code commits.
- Logs: `work/rust-phase1/repair-{workspace,market,postgres,python}.txt`.

Resume: merge Task 6 repairs before consumer route work; use the new static relation interface. No push or live activation was performed.

## Staging merge and Task 6 follow-up — 2026-10-16

Merged staging `b218ab264b92f99007bacc05d4871413e024b72a` into the build branch after Task 8. No conflicts. Staging's Python behavior is retained; the `775a75c` partial-fill seed fix remains unchanged. No rebase, push or live access.

The golden directory and policy manifest stay pinned to oracle `775a75c4dce449b875de5dd21998f1c34f9766aa` until Task 4b records the populated corpus and regenerates artifacts together. Existing source-SHA checks must use that oracle, not relabel old golden bytes as the merged source. Task 6 targeted vectors and the disposable market oracle can test the new rules independently.

KYC switch: red comparison failed for `KYC gate false`; green comparison passes both pinned Python states (46 stored orders, 14 strict-mode public orders; four formal trades). Added pending/submitted no-grace and foreign-review-with-grace owners, plus isolated assisted customer organizations. Runtime config defaults enabled, accepts the `KYC_EXECUTION_GATE_ENABLED` JSON key, and rejects duplicate aliases/non-boolean values. The shared binder now takes `&Config`; gate is `$13`, tail binds start at `$14`. Trade bindings do not change. Logs: `work/rust-phase1/kyc-switch-{red,green,final}.txt`. A SQL cleanup briefly failed syntax validation; the corrected final runner passes two tests.

R1: added stored-form Python vectors for FAME/LNG ASK/BID unconstrained-scale fields, quality results, signed zero, exact range edges and constrained decimal_places. Red failed on stored `cfpp_c=1E-29`; green passes all 23 market tests and six Python vector tests. Typed validation now compares exact coefficient/exponent values to schema integer bounds, and applies Python's 28-digit context only to decimal_places validation. Wire spelling and digest normalization remain separate. Logs: `work/rust-phase1/r1-{red,green}.txt`.

R2: red failed on `region Singapore`. The shared relation now matches region OR delivery-point name and treats an empty string as no filter. Oracle cases cover Singapore/Asia, Rotterdam/Europe, unmatched, empty and absent filters. Green: two migrated market tests pass. Logs: `work/rust-phase1/r2-{red,green}.txt`.

Implementation complete for this step; final full checks follow. R3 remains assigned to Tasks 9, 10a and 10b under decision 16. R4 is already corrected by the PM.

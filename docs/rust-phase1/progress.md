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

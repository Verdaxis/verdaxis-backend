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

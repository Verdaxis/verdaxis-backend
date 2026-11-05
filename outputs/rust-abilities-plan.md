# Rust User Abilities Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Serve schema-version 1 user ability hints from Rust with the same admission rules as the Rust actions.

**Architecture:** Mount a read-only auth route beside `/api/auth/me`; reuse its authenticated transaction and shared user lock. Compose existing Rust market, KYC, role, provenance and staff gates. Preserve all 23 action IDs; RFQ actions always deny with ACTION_RETIRED. Add a separate exact Caddy snippet, with equal upstream arguments disabling it.

**Tech Stack:** Rust/Axum/SQLx/PostgreSQL; pytest and local Caddy contract checks.

## Constraints and review

- Existing user contract is the approved design. No Python application edits, migrations, live server access or push.
- Architecture: PASS reuse the existing authenticated route pattern.
- Quality: WARN verify each gate against its real route; do not add a second policy implementation.
- Tests: PASS add ordering/reason cases, HTTP contract and disposable PostgreSQL coverage.
- Performance: WARN reuse bounded fact reads and avoid per-ability queries.

## Steps and checkpoints

1. Read all four reference copies, frontend parser/fixtures, architecture, and relevant lessons. Locate real Rust policies and document differences.
2. Add failing abilities tests and exact Caddy route tests. Run to demonstrate the missing route/snippet. Commit tests locally before product implementation.
3. Add `rs/read/src/auth/abilities.rs`, compose shared gates and mount in auth. Extract a shared helper only where a real action currently has an inline rule. Do not change action behavior. Add SQLx offline metadata by the repository method if a new checked query is required.
4. Add `deploy/caddy/rust-abilities.caddy`, exact GET/HEAD/OPTIONS matcher and disabled-equal-upstream expression. Update current route ownership contracts, leaving historical phase inventories historical.
5. Add adapted `docs/user-abilities.md`, update `ARCHITECTURE.md`, and run all new tests. Commit implementation in small coherent steps.
6. Run final gates and review simplification/correctness. Record exact results in `outputs/rust-abilities-verification.md`; commit documentation/results. No push.

## Verification commands

Run from this worktree. For cargo use the `rs/Cargo.toml` workspace and:

```
PATH=/opt/homebrew/opt/rustup/bin:/opt/homebrew/opt/util-linux/bin:$PATH
RUSTUP_TOOLCHAIN=1.95.0
SQLX_OFFLINE=true
CARGO_TARGET_DIR=$HOME/verdaxis-test/runs/.target-phase10
CARGO_INCREMENTAL=0
RUST_TEST_PYTHON=$HOME/verdaxis-test/venv312/bin/python
flock $HOME/verdaxis-test/runs/.lock-target-phase10 cargo ...
```

Required: `cargo fmt --check`, clippy `-D warnings` default and `--features verdaxis-read/contract-tests`, new Rust tests (including real disposable PostgreSQL following the existing auth runners), changed Python contract tests. Run local test services only; never connect to staging/production.

## Resume point

Initial reference and test-pattern review in progress. No product code changed. Next: finish Rust policy map, add failing contract tests, commit tests.

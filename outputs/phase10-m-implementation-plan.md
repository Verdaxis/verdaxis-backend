# M release window implementation plan

> **For Claude:** Use the executing-plans guidance to implement and verify each step.

**Goal:** Provide noninteractive M release, protected recovery and ordered window entry points without deleting Python.

**Architecture:** Extend the existing shell/native deploy transaction. Keep immutable source approvals, both locks and blocked-state startup guards. Separate unit installation, group transfer, Caddy preparation and complete-bundle recovery; missing prerequisites refuse rather than fall back.

**Tech stack:** Bash, jq, existing Rust CLI/sqlx, Python contract tests and offline Caddy candidate tools.

## Design review

- Architecture: PASS. Reuse canonical deploy and installer; no second migration owner.
- Quality: PASS with constraints. Fixed command sets, protected files, source-bound approvals and no arbitrary shell acceptance callbacks.
- Coverage: PASS with constraints. RED/GREEN contracts for each entry point, real file publications and before/after kill points; Linux/systemd acceptance is separate.
- Performance: PASS. Bounded fixture data and probes; no new daemon or application query.

## Steps and owners

1. Done: `8476cadd5`, `docs/phase10/window-gaps.md`, exact baseline gaps and reuse map.
2. Identity worker (isolated `rust-p10-m-identity`): write/run failing `tests/rust_contract/test_phase10_m_identity.py`; update runtime config/artifact/allowlist and exact policy rows; preserve drift tests. Review and commit locally.
3. Main (`rust-p10-m`): write/run failing `tests/rust_contract/test_phase10_m_deploy.py`; extend `scripts/deploy.sh` for explicit adoption, sqlx repeat deploy, Python stop/disable, Rust-only restart and durable publications. Keep every standing gate and add acceptance evidence before state clear. Extend `deploy/tests/deploy-transaction.sh`; review and commit.
4. Bundle worker (isolated `rust-p10-m-bundle`): write/run failing Python and shell bundle contracts; add protected capture/verify/restore entry points, actual-old-binary checks, both-lock blocked restore, publication kill points and fail-closed acceptance. No schema or DB role changes. Review and commit.
5. Caddy worker (isolated `rust-p10-m-caddy`): write/run failing `test_phase10_window_caddy.py`; compose reviewed helpers/snippets from saved bytes, all groups/production CRUD, private exact rollback, validation and probes. No host reads or live reload.
6. W2 worker (isolated `rust-p10-m-w2`): write/run failing Python/shell handover contracts; install immutable units then stop/drain/fence and transfer exact groups, one invocation per service, per-group rollback. Verify retired recovery and the restart/watchdog replacement. Resolve timer sets from static inventories; do not fabricate units.
7. Main integrates reviewed commits, updates removal ledger and independent inventory pins, ShellCheck list, source ownership pins and architecture. Run all required gates in the specified shared target directory; every cargo command uses its existing flock.
8. Review complete diff for safety and simplicity, run final checks and commit. Record Linux-only checks as not run; no push or host access.

## Gate commands

From repository root, use the user-specified environment: Rust 1.95.0, SQLX_OFFLINE=true, CARGO_INCREMENTAL=0, CARGO_TARGET_DIR=$HOME/verdaxis-test/runs/.target-phase10, RUST_TEST_PYTHON=$HOME/verdaxis-test/venv312/bin/python, full RELEASE_SHA. Wrap every cargo invocation with `flock -w 7200 $HOME/verdaxis-test/runs/.lock-target-phase10`.

- `cargo fmt --manifest-path rs/Cargo.toml --all --check`
- `cargo clippy --manifest-path rs/Cargo.toml --all-targets -- -D warnings`
- Same clippy with `--features contract-tests`.
- `$RUST_TEST_PYTHON -m pytest tests/rust_contract/test_phase10_*.py -q`
- ShellCheck every actual path in `deploy/tests/shellcheck-files.txt`.
- Relevant runtime/source-ownership and new portable shell fixtures.

## Resume and Task 31 boundary

Read this file and `docs/phase10/window-gaps.md`, inspect local branch history/status and collect worker results before editing shared files. Main owns the shared ledger/list and deploy script. No worker may overwrite another lane.

Task 31 still supplies the disposable Linux/systemd driver, signed old/new artifacts, installed inventories, real identities/ACLs, full interruption matrix, auth/session/passive evidence and timed full run. Recovery is retired: verify its mask/absence and the restart-drop-in/watchdog replacement rather than activating an unavailable CLI. W5 deletion is not part of this lane.

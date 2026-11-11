# Agent Audit Ledger Implementation Plan

> **For Claude:** Use executing-plans to implement this approved task step by step.

**Goal:** Add a redacted append-only agent audit ledger with intent-first receipts and read-only chain checks.

**Architecture:** Rust owns explicit microsecond timestamps and deterministic JSON. One transaction advisory lock serializes appends. Intent handles are private and issued only after commit. PostgreSQL schema and ACL policy remain unchanged.

**Tech Stack:** Rust, Tokio, SQLx, PostgreSQL, serde_json, SHA-256, regex.

## Steps and checks

1. Reuse the recursive sorted JSON helper in `rs/read/src/market/terms.rs` with crate visibility. Add `agent/audit.rs`, minimal `agent/mod.rs`, and the `agent` export in `lib.rs`.
2. Add unit tests for canonical encoding, each redaction pattern, configured values, key collisions, and payload limits. Use fake strings built at runtime. Keep all errors free of payload data.
3. Implement pool or transaction append with one advisory transaction lock, the all-zero genesis hash, and explicit UTC microsecond timestamps. Receipt kinds require a committed intent handle. Keep receipt intent IDs outside capped outcome content.
4. Implement bounded, read-only snapshot verification with predecessor anchoring for ranges. Report the first broken ID without exposing stored data.
5. Add `rs/read/tests/agent_audit_postgres.rs` to `group_crud.rs`. Cover chain links, concurrent writers, tampering, receipt references, runtime ACLs, transaction rollback and range checks.
6. Add `run_agent_audit.sh` using the existing pinned disposable PostgreSQL runner pattern. Require the exact passed test count and zero ignored cases. Register the runner in `.github/workflows/rust-read-ci.yml` and `deploy/tests/shellcheck-files.txt`. Update `ARCHITECTURE.md`.
7. Run `cargo fmt`, `python3 scripts/check_test_groups.py`, shell syntax checks, and available ShellCheck only. Do not run local builds or PostgreSQL tests. Review changes with demand-elegance and an independent code review.
8. Commit as Jon <tech@marinachain.io> with the requested Claude Opus 5.5 trailer. Push `feat/agent-audit-ledger`. Open PR to `staging` titled `Add agent append-only audit ledger` with the requested final attribution line. Iterate on GitHub Actions until green. Do not merge or use admin access.

## Resume point

Approved design. Source and integration patterns located. Implementation starts at step 1. All compilation and database execution belong in GitHub Actions. Leave this clone and other work intact. Do not change migrations, dependency snapshots, host configuration, or the reference repository.

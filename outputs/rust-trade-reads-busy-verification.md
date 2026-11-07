# Trade read hotfix verification

## Release scope

Base: `0920be378` (`origin/staging` at assignment). Branch: `fix/rust-trade-reads-busy`.
Normal current-line release only. No change to the revision allowlist, migration checkpoints, Alembic chain, runtime grants, deployment helpers, or Phase 10 files. No push or host access.

## Cause and change

On the base, `rs/read/src/order_write/trades.rs:374-378` assigns ordinary list and summary reads to `InventoryEdit`. The live `OperationLockPlan` takes the organization row `FOR UPDATE` at base `locks.rs:520-521`, then the user row `FOR UPDATE` at base `locks.rs:526`. These are not the older unused scaffold locks in transaction.rs. Live `locks.rs:488-493` sets the 500 ms deadline. `error.rs:371` maps SQLSTATE 55P03 to MARKET_BUSY.

`TradeRead` now shares organization and user locks (`locks.rs:525-537`). The lock order, preview revalidation, transaction lifetime and admission checks stay unchanged. The offboarding marker is checked before the cleanup exception (`admission.rs:101-132`), so reads retain protection against account changes. FOR SHARE conflicts with non-key updates as well as FOR UPDATE; FOR KEY SHARE would not suffice. Assisted-context reads keep their existing writer graph because they can persist expiry (`support/contexts.rs:444-505`).

The base list selects page IDs then runs `self.load` per item (`trades.rs:960-966` on base). The new bounded page hydration uses one statement, retains selected order and viewer redaction, and includes ACTIVE freezes (`trades.rs:558-604`, `971-1002`). Summary was already an aggregate and has no item loop.

A disposable PostgreSQL 100-trade plan showed an admin with an unrelated UNKNOWN organization scanning/joining 100 rows before filtering to zero (membership relation, 29.552 ms execution in the emulated local database). This explains additional admin work without returned items; it is not a production latency measurement. No live plan/cardinality was available. The small proven per-item roundtrip cost was removed; membership semantics were not rewritten.

## Test-first evidence

- `organization_admin_trade_reads_overlap_and_protect_admission` (`market_write_pg_trades.rs:1290`): an APPROVED admin in an APPROVED UNKNOWN organization; runtime-role RLS delays a real trade SELECT for 0.8 s. The test observes PgSleep after admission, then tests all list/summary pairs. Before the change: 409 MARKET_BUSY instead of 200. After: both readers return 200; NOWAIT non-key account and organization writers still receive 55P03.
- `trade_page_hydrates_in_one_statement_with_active_freezes` (`:1375`): before, three hydration statements for three items; after, one, with stable order and active-freeze fields.
- Both tests passed in the final complete PostgreSQL runner, not only targeted runs.

## Final gate exits

| Gate | Exit | Observed result |
| --- | --- | --- |
| cargo fmt --all -- --check | 0 | Rechecked after final test change |
| clippy workspace --all-targets -D warnings | 0 | Full workspace |
| clippy contract-tests --all-targets -D warnings | 0 | Rechecked after final test change |
| cargo test --locked --workspace --no-fail-fast | 0 | 1,008 passed, 0 failed; 119 database-marked cases ignored here |
| rs/read/tests/run_postgres.sh | 0 | Final full rerun: 1,763 passes across targets, 0 failures; runner-selected migrated suites ran |
| rs/read/tests/run_market_contract.sh | 0 | 34 passed, 0 failed; one optional timing case ignored |
| run_phase4_authority.sh | 0 | Migrated authority |
| run_offboarding_schema.sh | 0 | Schema |
| run_offboarding_admission.sh | 0 | Admission |
| run_offboarding_credentials.sh | 0 | Credentials |
| run_offboarding_command.sh | 0 | Command |
| run_offboarding_market.sh | 0 | Matching, conservation and locks |
| run_offboarding_composition.sh | 0 | Composition |
| run_offboarding_acceptance.sh | 0 | Cross-stack acceptance |
| run_abilities.sh | 0 | Migrated abilities |
| Real Caddy ownership/routing tests | 0 | Current CI routing files and Phase 5 matrix |
| Offboarding real-Caddy exact case | 0 | One ignored-by-default case explicitly executed |
| SQLx metadata check | 0 | Disposable migrated schema |
| Combined contract gate, all groups and targets | 0 | Complete ledger and meaning profile |
| Phase 2 all scenarios | 0 | Current CI command |
| Phase 2 qualified race schedules | 0 | Current registry |
| Backend CI: pytest tests/unit tests/monitor -q | 0 | Disposable Linux with pinned dependencies/Caddy and real system tools: 2,950 passed, one root-only skip |
| Skipped different-UID backend case, non-root rerun | 0 | One passed |
| git diff --check / added-text scan | 0 | No forbidden added terms or internal IDs |

Cargo used toolchain 1.95.0, SQLX_OFFLINE=true, CARGO_INCREMENTAL=0 and only the prescribed shared target under its flock. A first routing attempt was stopped after it spawned Cargo without that environment; the full routing matrix was rerun under the shared lock and passed.

## Earlier failures, retained for traceability

- Initial full PostgreSQL runner exit 101: `kyc_submit_vs_membership_change_revalidates`, first assertion: `(409, MARKET_BUSY)` instead of `(409, KYC_ORGANIZATION_MISMATCH)`. The exact base's isolated case passed. No source change was made for this timing result. The final complete runner passed the same case and all others.
- Initial market contract runner exit 101: `biofuel_supplier_metadata_clause_hides_only_failing_asks` and `eligibility_sets_match_legacy_for_the_curve_relation`, first server error: `syntax error at or near ")"`. The exact base reproduced both failures. Both test compositors now call the shared KYC renderer instead of substituting a retired marker; complete runner exit 0.
- Initial macOS backend run exit 1: missing systemd-analyze and Caddy. Linux setup attempts then found missing system packages and PID-1 reaping. The final complete run used the required packages plus Docker --init and passed. No backend application code was changed.

Raw logs and resumable plan are local under `work/hotfix-busy/`; they are not release files. Final report is this file. The hotfix is committed locally only.

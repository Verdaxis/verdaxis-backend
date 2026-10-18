# Rust Phase 1 integration — 18 October 2026

Status: correctness integration passed. Draft [PR #89](https://github.com/Verdaxis/verdaxis-backend/pull/89) targets `staging`: **do not merge**. No deployment is authorized.

## Joined sources

- Speed snapshot: `51c0b08c889c92d7668eb8cbfd9015fe36688822` from `feat/rust-phase-1-20261015`.
- Accepted gate: `92a97587`, merged by `853b32aa85e345afce29752070d0157a72faef02`.
- Accepted admission: `fa0185df`, merged by `de2868e391c798fa6f33fdd5ece80449231db3a9`.

The speed branch and its worktree were not changed. Later speed commits require a new PM request. No performance or capacity measurement was run by this lane.

## Conflicts

The gate merge had no conflicts. The admission merge had two:

- `docs/rust-phase1/go-no-go.md`: gate supplied the current decisions 86/87 report; admission supplied a decision 88 configuration note with an older draft introduction. Kept the current report and added the admission note; did not restore the old draft introduction.
- `scripts/rust_contract/report.py`: gate required `frozen_verdict`; admission lacked that import. Kept the import and the automatically merged configured-admission wording.

No integration defect required a source change beyond these resolutions.

## Test-host gates

All builds and tests ran through `verdaxis-prod-test` and `heavy run 3600`; none ran on the Mac. Tested source: `de2868e391c798fa6f33fdd5ece80449231db3a9`.

| Gate | Exit | Evidence |
| --- | --- | --- |
| `cargo test --locked --workspace` | 0 | `work/integration-offline.log` |
| `cargo fmt --all -- --check` | 0 | same |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | 0 | same |
| `pytest tests/rust_contract tests/unit/test_rust_budget.py tests/unit/test_rust_release.py -q` | 0 | same; 665 passed, 5 warnings |
| Rust `gate` and `frozen_gate` test targets | 0 | same; 31 passed |
| Admission and runtime PostgreSQL targets, `contract-tests`, serial | 0 | `work/integration-disposable-retry.log`; 4 + 5 passed |
| `scripts/rust_contract/sqlx_offline_check.py` | 0 | same |
| Full disposable CI contract: `--group all --rust-target all --mixed-caddy --require-complete-ledger` | 0 | same; assertion ledger OK |

The PostgreSQL run used a scratch copy of the existing container runner with only the Cargo target selection narrowed to admission/runtime. The full disposable runner separately exercised all contract targets. Its report remains at `work/integration-ci-contract.json` on the test host. Negative-input contract cases emitted Python availability-window exceptions; all comparator targets and the complete assertion ledger passed.

An initial disposable-call wrapper had a shell quoting error (call exit 2) before tests started. The corrected call completed with all gate exits 0. The host call waited for another lane's exclusive measurement; this lane did not interrupt it. Both heavy calls finished.

## GitHub CI

For integrated source `de2868e3`, all five jobs passed:

- Rust `offline`: 5m23s.
- Rust `contract`: 12m12s.
- Backend `test`: 1m52s.
- Backend `postgres-phase0`: 4m1s.
- Backend `postgres-analytics`: 4m31s.
- `publish`: skipped as expected for a draft PR.

Runs: [Rust](https://github.com/Verdaxis/verdaxis-backend/actions/runs/39119229420), [backend](https://github.com/Verdaxis/verdaxis-backend/actions/runs/39119229424).

Resume: keep this PR as CI-only. Await the PM's request before merging further speed commits or scheduling any measurement.

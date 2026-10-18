# Rust Phase 2 referrals

## Task 17 — reads and public resolution

R02/R03/R05 are implemented in `rs/read/src/crud/referrals/reads.rs` and composed by `referrals::routes`. Authentication settings and frontend URL are injected; runtime environment projection remains with the integration owner. `routes/mod.rs` exports the route factory. No Python application, migration, runtime configuration or mail behavior changed.

Statistics retain exact source disclosure, totals and timestamp ordering. Leaderboard aggregation and user/organization projection use batch queries. Count ties retain source ordering: ranks and tenth-place membership must not be normalized away. Public resolution ignores bearer headers and preserves Python's final-newline code grammar and exact database lookup. Explicit HEAD refusals match FastAPI.

### Gates

All host calls used staging `heavy run 3600`. No measurements ran.

| Gate | Exit | Result |
| --- | --- | --- |
| Initial test invocation | 101 | Build required RELEASE_SHA; corrected invocation |
| Focused projection tests | 0 | 10 passed; DB test returns early without explicit disposable URLs |
| HEAD regression RED | 1 (Cargo 101) | Real disposable route returned 200 instead of 405 |
| Component GREEN | 0 | 10 passed including disposable PostgreSQL route test |
| Interrupted final call | not available | Wrapper recorded interruption; no completion marker; not counted |
| Final Clippy | 0 | Referral library/read target, warnings denied |
| Final disposable component | 0 | 10 passed, full migration chain and app-role reads |
| Final wrapper | 0 | CALL_EXIT=0 |
| Twin-database response/state comparison | pending the runner lane | Not qualified by the component scenario |
| CI at d5572a38 | 0 | test, postgres-phase0, postgres-analytics, offline and contract passed; publish skipped |

Private local logs: `work/rust-phase2/task17-{red,red2,green,final,final2}.log`. Scenario definitions: `tests/rust_contract/phase2/referral-reads.json`. The inherited runner registration is explicitly component-only, not a substitute for shared twin-database differential coverage. The runner owner will replace/register the scenario when its guide lands.

## Task 18 — code allocation

R01 is registered with the referral router. Allocation holds the shared authenticated transaction's exclusive actor lock, probes at most ten candidates and updates only `users.referral_code`. Existing codes and the raw frontend URL are preserved. Candidate generation uses secure UUID random bytes with rejection sampling; deterministic sequences are available to explicit component tests and the contract feature, not request input.

- RED focused target: exit 101, missing Task 18 exports.
- GREEN focused target: exit 0, 3 passed.
- Clippy: exit 0, warnings denied.
- Migrated PostgreSQL component: exit 0, 10 passed; includes existing/unverified users, same-user concurrent allocation and exact whole-users-row comparison.
- Wrapper: CALL_EXIT=0 (`work/rust-phase2/task18-green.log`).
- Twin-database state/response comparison, collision barriers, ten-collision schedules and mixed writer: pending the runner lane. Definitions are in `tests/rust_contract/phase2/referral-code.json`.
- CI: pending this task's push.

Draft PR: https://github.com/Verdaxis/verdaxis-backend/pull/102.

## Resume

Commit/push Task 17 and obtain CI results. Then complete Task 18 code allocation with focused tests, followed by Task 19 invite delivery. Task 18 preliminary source and vectors are not Task 17 evidence. No real test email is authorized. Shared runner race, mixed-writer and captured-send gates remain pending that lane. Runtime settings, environment projection, new dependencies and comparator/CI changes require the PM boundary stated in the brief.

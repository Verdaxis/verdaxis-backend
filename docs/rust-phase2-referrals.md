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
- CI at 14961e1a: exit 0; test, postgres-phase0, postgres-analytics, offline and contract passed; publish skipped.

Draft PR: https://github.com/Verdaxis/verdaxis-backend/pull/102.

## Task 19 — invite delivery component

R04 validates, authenticates, normalizes the address and consumes the shared endpoint quota before business checks. Existing recipients get the generic response without code allocation or delivery. Missing code allocation commits before the provider request, so a refused send retains only that code. Repeated accepted requests send again. Provider requests are bounded to ten seconds, accept only 200/201, ignore response bodies and log no addresses or provider content. Source HTML is held in `invite.html`; substitutions are escaped in one pass. R04 trims the frontend slash and quotes the code, unlike R01.

Already-locked dependencies `reqwest=0.13.5` and `idna=1.1.0` were reported before declaration. The lockfile adds only those direct dependency edges, no new package. Mail is disabled by default; tests never obtain a production key.

| Gate | Exit | Result |
| --- | --- | --- |
| Focused RED | 101 | Missing invite module |
| First component test | 0 | 3 passed |
| First lint | 101 | Collapsible conditional; corrected |
| Final focused component | 0 | 3 passed, including local status matrix and disabled transport |
| Final Clippy | 0 | Warnings denied |
| Migrated PostgreSQL component | 0 | 10 passed; includes failed-send retained code and two captured successful sends |
| Final wrapper | 0 | CALL_EXIT=0 in task19-green2.log |
| CI | pending | Recorded after push |

## Pending integration / resume

These components are not a cutover qualification. Shared runner response/state differentials, deterministic collision barriers, mixed-writer schedules, injected transaction faults, complete EmailStr diagnostics/Unicode normalization vectors, timeout transport vectors and full captured payload comparison remain pending. Scenario data is in `tests/rust_contract/phase2/referral-{reads,code,invite}.json`; it must be adapted to the runner lane's final schema rather than silently treated as executable coverage.

Needed runtime projection (not implemented): construct `ReferralSettings` with the shared `VerificationSettings` and raw `FRONTEND_URL`; inject `EMAIL_FROM`, `MailClient::production(RESEND_API_KEY)` and the composition's shared `SharedLimits`; merge `routes::referral_routes(settings)`. No Config/AppState, secret/environment, deploy, comparator or CI workflow changes were made. Public HEAD refusal is local to this router. EmailStr compatibility is not complete: the current address validator covers basic local/domain and pretty-address cases, but its generic invalid-address diagnostic is not Python's detailed error envelope; this must be resolved before qualification. Cancellation after pre-send commit is also not yet tested against Python. Keep referrals on Python until those gates pass.

No real test email, measurement or live deployment ran. The repository's optional commit security hook returned HTTP 404 for Task 17, so that review did not run; details remain in the private local security log.

After CI, wait for the PM runner handoff. Do not implement shared runner features in this lane.

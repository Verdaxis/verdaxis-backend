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

These components are not a cutover qualification. The gap gates below now cover shared runner response/state differentials, deterministic collision barriers, mixed writers, injected transaction faults, the captured EmailStr corpus and timeout transport vectors. Scenario data is registered through `tests/rust_contract/phase2/scenarios.json` and the lane block in `phase2_scenarios.py`.

Needed runtime projection (not implemented): construct `ReferralSettings` with the shared `VerificationSettings` and raw `FRONTEND_URL`; inject `EMAIL_FROM`, `MailClient::production(RESEND_API_KEY)` and the composition's shared `SharedLimits`; merge `routes::referral_routes(settings)`. No Config/AppState, secret/environment, deploy, comparator or CI workflow changes were made. Public HEAD refusal is local to this router. Cancellation after pre-send commit is covered by the lost-response schedule, which classifies the request as ambiguous and never replays it. Keep referrals on Python until the PM integrates the runtime projection.

No real test email, measurement or live deployment ran. The repository's optional commit security hook returned HTTP 404 for Task 17, so that review did not run; details remain in the private local security log.

## Runner integration follow-up — 19 Oct

Merged `origin/feat/rust-p2-runner` through `808f4fa8` as `31bbc6d7`. Added one marked real registry block and a test-only Rust HTTP server. No shared runner behavior or comparator changed.

### Completed gates in this follow-up

| Gate | Own exit | Evidence |
| --- | ---: | --- |
| First twin adapter build | 101 (wrapper 1) | ApiError conversion in example; corrected |
| First real HTTP differential | 1 | Invalid email lacked Python reason/ctx; earlier actions passed |
| Python EmailStr vector capture | 0 | 24 synthetic inputs, no network |
| Exact EmailStr RED | 101 | Missing detailed reason/ctx |
| First vector GREEN attempt | 101 | Unclosed angle bracket diagnostic; corrected |
| Focused invite vectors | 0 | 4 tests passed; captured vectors compare exact normalized strings and errors |
| Real referral group with races | 0 | `referral-twins-green2.log`, full HTTP/state/mail comparison |
| Timeout/no-replay | 0 | One local attempt; ten-second provider deadline; no retry |
| All-target Clippy | 0 | Warnings denied |
| Shared registry regression | 0 | 10 passed with populated registration |
| Final check wrapper | 0 | CALL_EXIT=0 |

The real scenario compares empty stats/leaderboard, public valid/unknown/newline/invalid-bearer resolution, unauthenticated reads, existing-code R01, five provider status cases, exact complete mail payloads and invalid-email diagnostics. Three fixed-seed same-user existing-code client-barrier schedules preserve all committed state. They do **not** prove database lock order or allocation races.

`unicode-normalization=0.1.25` was reported before direct declaration; it was already locked. NFC local normalization and specific Python diagnostic branches now pass the captured vector set. This is not proof of full EmailStr grammar: unsupported Unicode categories, arbitrary invalid-character names, lengths and some pretty-address details still need complete parity. No approved-difference exception was added.

### Remaining blockers at that point (closed below)

The follow-up scenario lacked the gap fixtures; the section after this one records their closure. Keep all referral routes on Python until the PM integrates the runtime projection.

Task 23 runtime change stays exactly: project raw `FRONTEND_URL` into `ReferralSettings::new` without trimming; inject existing verification settings, EMAIL_FROM, production mail client from RESEND_API_KEY, and shared quota authority; merge the exported route factory. No production runtime projection was edited.

Private run report: `work/rust-phase2/referral-twins-green2.json`. Source and vectors are committed; raw provider payloads remain memory-only. CI for this follow-up is reported after push.

## Gap closure — 19–20 Oct (6adea51a)

All calls used staging `heavy run 3600`. No measurement ran. Each gate ran alone; every call finished with its own marker.

| Gate | Own exit | Result |
| --- | ---: | --- |
| Python EmailStr corpus regeneration | 0 | 1535 synthetic vectors, byte-identical to the committed file |
| Committed corpus comparison | 0 | 1535 vectors, exact normalized address or complete error list; no difference |
| Python Unicode sweep generation | 0 | 387171 lines, sampled code points in nine address positions |
| Focused Rust unit targets | 0 | `crud_referral_invite` 5 passed, 1 ignored; `referral_mail_timeout` 2 passed |
| Unicode sweep replay (`--ignored`) | 0 | 387171 lines, no difference |
| Referral-target Clippy | 0 | Warnings denied |
| `referral-contract` with races | 0 | 14 actions, 3 races, state and mail digests equal |
| `referral-allocation` with races | 0 | ten-collision and nine-collision HTTP schedules; four database-ordered schedules with lock-wait observation in both runtimes |
| `referral-faults` | 0 | injected before-commit rollback; ambiguous commit retained, no replay |
| `referral-mixed` | 0 | both writers on one database, then Python dispatcher; `mixed_writer_dispatch` recorded |
| `referral-invite-failures` | 0 | nine missing-code and existing-recipient actions |
| `referral-invite-timeout` | 0 | two provider timeouts at the ten-second deadline; first code retained, no replay |
| `referral-invite-no-key`, `referral-reads-populated`, `referral-invite-addresses` | 0 | 2, 14 and 34 actions; every state digest equal |

The allocation gate installs a fixture-only trigger that takes an advisory transaction lock on the updated row. The schedule holds that lock, waits until `pg_stat_activity` reports a lock wait for the scenario database, then releases it. Both runtimes therefore take the same database order, not only the same client order.

The populated gate returns a full top ten whose counts tie inside the ranking and across tenth place, with blank, missing, padded and non-English names. The comparison keeps ranks, page membership and order.

The address gate sends 34 requests through the route, including quoted local parts, IP literals, special-use domains, control and bidi characters, length boundaries, non-string bodies, display names, decomposed and internationalized domains, and self- and existing-recipient cases after normalization.

### EmailStr corpus and approved difference

`referral_email_vectors.py` derives the corpus from the pinned Python schema without network or database access. It covers ordinary and special-character local parts, quoted local parts, display names, IP literals, internationalized local parts and domains, punycode, normalization forms, every length boundary and representative code points. The Rust test compares the normalized address, or the complete pydantic error list including `loc`, `type`, `msg` and `ctx`.

Decision 94 exception, recorded with vectors in `tests/rust_contract/phase2/referral-email-approved-differences.json`: a lone surrogate escape is valid JSON, and Python rejects the address with a 422 whose echoed input Starlette cannot encode, so Python answers 500. Rust refuses the broken JSON with 422 `json_invalid` and never answers 5xx. Only status and body may differ; state and provider sends must match. Three vectors cover high, low and reversed surrogate escapes.

Referral routes stay on Python until the PM integrates the runtime projection. The transaction-safety change in `rs/read/src/auth/transaction.rs` is not in this branch.

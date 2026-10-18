# Task 16: organization price-alert CRUD

## Status and scope

Task 16 implementation and correctness gates are complete with approved difference PD8 (PM, decision 94). Rust serializes organization count/insert with a transaction-scoped advisory lock after the existing authenticated user lock. Both fixed-seed actor directions leave five active Rust alerts with statuses 201/403; unchanged Python leaves six with 201/201. The historical source-race record below is superseded by the PD8 record. The PM's 19 October ownership update permits the marked additive module and route-export blocks. No runtime configuration, manifests, dependencies, comparator, CI workflow, Python application, or migration was changed.

The component router is `routes::alerts_router(app, verification, limits)`. The integration lane must supply its existing verification settings and shared quota authority. Runtime secret/environment projection and activation are not implemented here.

## Implementation

- A01 creates a separate organization-owned row per accepted request, counts only active rows, and preserves the five-active cap.
- A02 returns all organization rows, including inactive rows, in created-time descending order.
- A03 distinguishes missing-row 404 from foreign-organization 403, permits another member of the same organization, and returns empty 204 on success.
- Request validation replays 65 Python vectors, including direction case, UUID types, non-object bodies, threshold positivity and nonfinite values. Independent errors retain schema order.
- Threshold text is bound to PostgreSQL NUMERIC without binary floating-point or rust_decimal narrowing. Storage checks cover half-cent rounding, positive-to-zero rounding, overflow and both foreign keys.
- Product metadata includes inactive/legacy rows. Market product derives from the attested exact label tuple, not the database row UUID or activity flag.
- The shared authenticated transaction owns current-user admission, row locks and commit. No replay or organization row lock was added; PD8 adds only the transaction advisory lock. Python retains triggering and event production.

## Evidence

All builds, formatting, lints and tests ran through `verdaxis-staging-test` and `heavy run 3600`. No measurements were run. Local temporary logs are under `work/rust-phase2/` and are not committed.

| Gate | Exit | Evidence |
| --- | --- | --- |
| Initial RED attempt | 101 | `alerts-red.log`: missing required RELEASE_SHA; not a valid feature RED |
| Corrected RED | 101 | `alerts-red2.log`: missing `verdaxis_read::crud` module |
| Intermediate implementation checks | nonzero | `alerts-green1` through `green4`: syntax/compile and exact UUID diagnostic differences; fixed, not concealed |
| Python oracle freshness | 0 | `alerts-green4.log`: 65 vectors, 48 rejected |
| Focused Rust tests | 0 | `alerts-green7.log`: four offline tests plus conditional PostgreSQL driver entry; the standalone run does not qualify DB coverage |
| PostgreSQL storage + HTTP admission + Python trigger | 0 | `alerts-green7.log`: one pytest driver passed, no skips; it runs the Rust PostgreSQL test with app-role credentials on the existing disposable migrated fixture |
| Focused Clippy, warnings denied | 0 | `alerts-green7.log` |
| Owned-file formatting | 0 | `alerts-green5.log`; subsequent owned-file formatting also completed |

The PostgreSQL driver proves no-org refusal on all three routes, foreign delete refusal, same-org member list access, path validation, cap/inactive behavior, rounding, FK/overflow failures, repeated create/delete, and Python trigger compatibility. The trigger event payload, participant organization and deactivation state are checked. The only pytest warning was an existing Pydantic class-config deprecation.

## Historical pending runner lane (superseded by the record below)

`tests/rust_contract/phase2/alerts-scenarios.json` is declarative coverage data, not a runnable or passing contract. Register it only after the shared runner interface lands. Required pending gates:

1. Paired HTTP responses and complete committed-state comparison after every action.
2. Controlled IDs and action clocks, full users/unrelated-table preservation, fault and ambiguous-commit cases.
3. Deterministic different-user/same-organization cap races in both schedule directions.
4. Mixed-writer dispatcher/outbox cell. The direct Python-trigger component check does not qualify dispatcher integration.

The source cap race has not been proven by a paired deterministic schedule. Do not silently repair it or claim equivalence. Section 0 of the plan overrides later Python-first repair instructions; a new approved Rust difference requires PM review.

## Runner integration record — 19 October

Merged the approved runner branch with merge commit `5b45319c`. Registered `alerts-crud` and `alerts-crud-reverse` in one marked Task 16 block. Both are real route scenarios, with no smoke fallback. `task16-runner-evidence.json` records 22 paired actions, two fixed-seed SQL-count barrier schedules, and two mixed dispatcher cells. Each action compares HTTP and full fresh-connection committed state, asserts users/unrelated tables unchanged, and checks zero mail sends. IDs and created_at values are controlled in disposable-only adapters; no timestamp or business ID is dropped.

The before-commit fault runs after insertion and leaves all business state unchanged. The after-commit response fault retains exactly one row and returns 503; the runner sends it only once and the assertion rejects replay. This tests response unavailability after a committed unit, not a simulated network disconnect during COMMIT acknowledgement.

The mixed schedule exercises Python create/delete and Rust create on the third database. The real Python helper deactivates all five rows and writes its normal outbox events. The real Python dispatcher publishes five distinct stream sequences; event direction, price, Rust-row threshold, and deactivation are checked. No mail delivery applies to alerts; the send ledger remains empty.

| Continuation gate | Own exit | Evidence |
| --- | ---: | --- |
| Adapter attempts 1–4 | 1 each | `alerts-runner1`–`4.log`: adapter imports, clone-name runtime refusal, incorrect GET expectation |
| Attempt 5 | 1 | Postcommit fault response omitted request-id; corrected in test adapter |
| Attempt 6 | 1 | Real DELETE 204 lacked Python's application/json content type; corrected in Rust route |
| Attempt 7 | 0 | One scenario: paired HTTP/state, rollback, retained commit, race and mixed dispatcher |
| Final focused Rust | 0 | `alerts-runner8.log`, four offline checks plus conditional DB driver entry |
| Final feature-enabled Clippy | 0 | `alerts-runner8.log`, lib and test adapter, warnings denied |
| Final group with both actor directions | 0 | `alerts-runner8.log`; seeds 20261018/20261019; full evidence JSON |

The cap-race **safety result is FAIL**: both SQL counts complete at four before either insert, then both return 201 and leave six active alerts. Two fresh cloned scenarios reverse actor submission order. No Rust-only lock or Python fix was added. PM approval is needed for a corrected Rust contract under plan section 0. The runner's zero exit means all declared differential/source-race assertions passed; it must not be described as a passing five-active cap invariant.

The disposable Rust adapter uses the assigned nonowner app-role pools because runner clone names do not satisfy the runtime connector's `_test` suffix rule. Runtime Config, its validation and release projection remain unchanged. Test seams are feature-gated; release builds have neither header-controlled IDs/clocks nor faults/barriers.

## PD8 completion record

PM approved PD8 under decision 94: Rust must enforce the cap; Python stays unchanged. Runtime ACL verification used the actual disposable application role after the existing ACL convergence. Both `has_function_privilege` and execution of `pg_catalog.pg_advisory_xact_lock(bigint)` succeeded. No new grant is needed.

Rust derives a signed 64-bit advisory key from SHA-256 of fixed namespace `verdaxis:price-alert-cap:v1:` and the full organization UUID. Hash collisions can only serialize unrelated organizations. `run_authenticated` acquires the user lock first; create acquires the transaction advisory lock before count/insert. Commit and rollback release it. No organization row lock or Python change was added.

The lane-owned PD8 checker is registered through one marked dispatch block. It applies only to the two explicit A01 race vectors in `pd8-differences.json`. It requires Python 201/201 and six rows, Rust 201/403 and five rows, exact cap-refusal body, equality of the successful actor response, and full-state equality after removing exactly the refused candidate row from Python. All unrelated tables, users and mail remain unchanged. Non-race comparisons continue through the unmodified shared comparator and full-state equality. This is an explicit approved difference, not silent normalization.

| PD8 gate | Own exit | Result |
| --- | ---: | --- |
| Runtime advisory privilege + storage/trigger interop | 0 | `pd8-acl.log`, one pytest driver passed |
| First RED attempt | 1 | Missing scenario import; not feature RED |
| Corrected RED | 1 | `pd8-red2.log`: Rust returned two successes instead of 201/403 |
| First GREEN, full alerts group | 0 | `pd8-green1.log`, both directions and mixed cells |
| Final full alerts group | 0 | `pd8-green2.log`, 22 paired actions, two approved PD8 races, two mixed dispatcher cells |
| Final focused Rust | 0 | `pd8-green2.log` |
| Final feature-enabled Clippy | 0 | `pd8-green2.log`, warnings denied |
| Final formatting | 0 | `pd8-green2.log` |

Final evidence: `task16-pd8-evidence.json`. No measurements or deployed operations ran. The lock coordinates Rust alert creators; legacy Python does not participate, so this is not a claim that a simultaneous legacy Python writer obeys the repaired cap.

### Plan-record text for integration owner

PD8 — Concurrent A01 calls by different users in one organization at four active alerts can both return 201 and leave six active rows in Python. Approved Rust behavior: after authenticated user admission and before active count, take a per-organization transaction advisory lock in a fixed price-alert-cap namespace; admit one create and return the existing cap-refusal 403 to the other, leaving five active alerts. Keep Python unchanged. Only this race response and its refused candidate row are an approved difference; all non-race contracts remain unchanged. Evidence/vectors: `tests/rust_contract/phase2/pd8-differences.json` and `docs/rust-phase2/task16-pd8-evidence.json`.

The integration-owned `docs/rust-phase2-plan-record.md` was not edited; PM can forward the paragraph above.

## Historical resume point

When `feat/rust-p2-runner` lands on the integration branch, read `docs/rust-phase2-runner.md`, add only the marked Task 16 scenario registration, and run the alerts differential/race and mixed-writer gates. Preserve the current implementation and vectors. Do not qualify a harness-smoke run as alert coverage.

Codesight regeneration is deferred: the machine-local helper runs project analysis, which this brief forbids on the Mac, and it is not provisioned as part of the authorized test-host path. ARCHITECTURE.md was updated directly.

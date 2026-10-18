# Phase 2 Task 8 — notifications

## Final acceptance update — runner fix 1

This section supersedes the historical blockers and pending gates below.

Runner fix `808f4fa8` was merged as `4f60edd4`. The actual notifications group
with `--phase2-races` passed: 16 sequential HTTP/full-state actions and six
four-client race repetitions, seeds 20261019–20261024. Each race compares replies
by actor and checks complete committed state. Schedules cover same-user
single/read-all, same-org different user, and cross-org rollback. These are
commuting-write invariants; no deterministic SQL lock order is claimed.

The sequential actions include two trigger-induced rollback failures, repeated
hidden-row marks, and one postcommit response-loss classification. The test-only
adapter returns 503 after the route committed, retaining exactly the intended
read-state unit. The executor sends that action once, with no automatic replay.
This is an injected response failure, not proof of TCP disconnect behavior.
Full notification data, other users and unrelated tables remain unchanged;
mail ledgers are empty. Task 8 produces no events and needs no mixed dispatcher
or invite-delivery gate.

Current own exits (all calls finished):

| Gate | Own exit | Evidence |
| --- | ---: | --- |
| First fixed-runner route attempt | 1 | Python fault adapter dropped request-ID headers; corrected task-owned hook |
| Corrected real notification group with races | 0 | 16 actions, six race repetitions; `ROUTE_GATE_EXIT=0`, `CALL_EXIT=0` |
| Required PostgreSQL component | 0 | Seven tests passed, including approved offset vectors; no skips |
| Clippy | 0 | Component test and contract adapter, warnings denied |
| Registry suite | 0 | Ten tests passed with isolated registries |
| Workspace formatting check | 0 | Passed |
| Final enclosing quality call | 0 | `CALL_EXIT=0` |

`skip > i64::MAX` is an **approved difference**, PM 19 October under decision 94,
not unresolved drift. `notifications.json` records maximum/overflow vectors and
the component test executes the Rust cases. Python accepts the large integer
then fails in the driver; Rust returns 422 before database work. No comparator
exception was added. The ordinary route scenarios remain strict comparisons.

Temporary logs and the runner JSON are in `work/rust-phase2/`; no tokens or raw
responses are committed. The successful route report identifies base HEAD
`4f60edd4` with the tested uncommitted adapter header correction; the subsequent
commit preserves that correction and the approved vectors. CI is run on the
pushed final commit through draft PR #99. No measurements or deployments ran.

## Historical record

## Status and resume point

The Rust component is implemented and passes its focused PostgreSQL tests. The runner branch was merged as `2507d442`; real scenario registration and task-local adapters are added. **Route acceptance is blocked by two shared-runner compatibility defects. Task 8 is not yet complete.** No cutover qualification is claimed.

### Runner follow-up, 19 October

The authorized notifications group was run twice through staging `heavy run 3600` with `--phase2-races`. Each call completed with `ROUTE_GATE_EXIT=1` and `CALL_EXIT=1`:

1. Fixture timestamp binding failed before requests (asyncpg requires a datetime); the task-owned fixture loader was corrected.
2. Rust correctly refused startup because `phase2_routes.py:94` creates names such as `phase2_route_0_rust`, but `runtime/config.rs:250` requires disposable test database names ending in `_test`.

Required owner change: suffix the three shared route cell names with `_test`. Do not weaken runtime target validation or substitute development mode. The adapter uses the runner's existing `Asia/Singapore` database timezone.

An independent check completed with component invocation exit 0, adapter Clippy exit 0, registry tests exit 1, enclosing call exit 1. The component invocation ran the offline checks; its database section was not enabled in this invocation (the earlier required PostgreSQL gate remains recorded below). Registry tests found another shared defect: `test_phase2_scenarios.py:19-27` assumes that no real notifications or all-group scenarios can ever be registered. The owner should test selection against an isolated empty registry instead of the growing lane registry. Eight other registry tests passed.

Ownership permits scenario registration entries only, not changes to shared lifecycle behavior or another lane's tests. These exact fixes are reported rather than implemented here. Current route adapters compile and pass focused Clippy. No shared comparator, runtime settings, dependency, app, migration or CI changes were made.

The registered scenario supplies HTTP/full-state assertions, recipient scope, private-field preservation, database-trigger rollback, test-adapter postcommit response-loss classification, no-mail assertions and six fixed-seed concurrent schedules. Those schedules have not executed because startup refused. They assert commuting read-state invariants, not deterministic SQL lock order. No mixed producer/dispatcher gate is required by Task 8, which creates no event. This task has no invitation delivery; its mail requirement is an empty send ledger. The scenario data and adapters remain unqualified until the actual route gate runs.

After the runner lands on `feat/rust-phase-2`, integrate its guide, register `notification_scope_and_email_state`, adapt the task-owned `tests/rust_contract/phase2/notification-scenario.json` to its data format, and run the notification differential gate. Do not use harness smoke as notification evidence.

## Implementation

- `crud/notifications.rs`: recipient-bound list/count and both read-state writes. List/count use PostgreSQL boolean coercion for `_email_only`. Writes include email-only rows. Response projection removes only top-level `_email_` keys and preserves null versus empty object and nested JSON.
- `crud/notification_authority.rs`: explicit composition-time verification settings and shared quota authority, with ordered authentication/validation/admission. No runtime settings, environment projection, secret loading, dependencies, Python app code, or schema changes.
- Minimal marked blocks expose `crud` in `lib.rs`, the notification module in `crud/mod.rs`, and `routes::notification_routes` in `routes/mod.rs`. The serving binary is not activated.
- `crud_notifications.rs`: offline vectors and real HTTP-router tests using the audited app role. Two users share one organization; a third belongs to another. Checks cover scope, redaction, SQL null/empty data, PostgreSQL email-only coercion, pagination/ties, invalid parameters, foreign/missing IDs, repeated writes, count transitions, unchanged full notification data and users, and no outbox/watchlist events.
- `run_notifications.py`: focused component-test driver that reuses the existing Runner container lifecycle and migration verifier, installs existing audited role policy, and requires the database test. It is not a second lifecycle owner or twin-cell runner.
- `notification_oracle.py`: unchanged Python DTO validation vectors, including UUID diagnostics and rejection of non-object data.

## Evidence

All builds and tests ran on the staging test host through `heavy run 3600`. No measurements ran. Local temporary logs remain under `work/rust-phase2/` and are not committed.

| Gate | Result | Own exit code |
| --- | --- | --- |
| Initial compile attempt | Missing build `RELEASE_SHA`; corrected command | 101 |
| Focused RED | UUID diagnostic mismatch; fixture timestamp incorrect | 101 |
| Python DTO oracle | UUID positions confirmed as one-based; scalar/list data rejected | 0 |
| PostgreSQL RED | Also exposed missing UUID casts in draft test setup | 101 |
| First PostgreSQL GREEN | 6 tests passed, no skips | 0 |
| First focused Clippy | Passed | 0 |
| Final PostgreSQL component | 6 tests passed, no skips; full notification/user invariants added | 0 |
| Final focused Clippy (`-D warnings`) | Passed | 0 |
| Final workspace formatting check | Passed | 0 |
| Final enclosing staging call | Completed (`CALL_EXIT=0`) | 0 |
| Twin-cell route comparison, fault/race and mixed writer | Pending the runner lane | Not run |
| Full suites | GitHub CI, recorded separately below | Pending |

The migration verifier emitted the existing SQLAlchemy warning about the `rfq_quotes`/`rfqs` dependency cycle, then reported no new upgrade operations. This did not fail verification.

The inherited prior remote RED log had no `CALL_EXIT=` marker. Its exact exit is unknown and it is not used as a current gate. All calls started during this continuation completed.

## Integration requirements and known limits

1. Runtime composition must supply `VerificationSettings` from the canonical secret loader and pass the same `SharedLimits` instance to the notification route authority and HTTP middleware. This lane does not add runtime settings or secret projection.
2. Shared HTTP middleware still needs the private-route trailing-slash/405 inventory. The lane does not change existing middleware behavior.
3. The inherited `skip > i64::MAX` draft boundary remains: Rust rejects with 422, while Python accepts its arbitrary-size integer before the database driver fails. This is documented drift, not an approved repair or a passing differential cell.
4. UUID diagnostics are matched for the recorded invalid-character and standard UUID vectors; broader invalid UUID grammar needs the runner's error corpus.
5. JSON numbers inside arbitrary notification data use the existing serde_json representation. The exact same-meaning comparator must qualify them against Python; do not infer whole-route equivalence from SQL text or component tests.
6. The direct-mark path runs inside the shared authenticated transaction and returns after commit; Python also performs a post-commit refresh. The runner must classify fault-schedule differences rather than treating a 2xx alone as state evidence.

## GitHub CI

Draft PR: https://github.com/Verdaxis/verdaxis-backend/pull/99. It targets `staging`, is CI-only, and must not be merged directly. The PM integrates this through `feat/rust-phase-2`.

Candidate `7b164361` passed both GitHub workflows: Rust run `39166184664` (`offline` and `contract`) and backend run `39166184675` (`test`, `postgres-analytics`, `postgres-phase0`). `gh pr checks --watch` exited 0. `publish` was skipped by design for the draft PR. These shared CI gates do not replace the pending Task 8 twin-cell scenario. Implementation commit: `d1535a68`.

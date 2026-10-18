# Phase 2 Task 8 — notifications

## Status and resume point

The Rust component is implemented and passes its focused PostgreSQL tests. The twin-database HTTP, fault, race, and mixed-writer gates are **pending the runner lane**, per the PM decision of 19 October. No cutover qualification is claimed.

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

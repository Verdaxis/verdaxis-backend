# Phase 2 Task 10: feedback and administrator listing

## Scope and status

Implementation commit: `4406ca7d` (base `dee697f3`).

F01 POST `/api/feedback` and F02 GET `/api/admin/feedback` are available as
composable routers, not activated in the serving binary. They use the shared
`run_authenticated` transaction and `validate_and_admit` endpoint quota boundary.
No runtime settings, dependencies, Python application code, migrations,
comparator, CI workflow, or deployment code were changed.

**Feedback stays on Python at cutover until Task 22 supplies the shared
Python–Rust quota counter across real processes.** Process-local quotas do not
qualify this route for mixed-runtime cutover.

## Contract

- Messages use Unicode code-point bounds and Python-compatible stripping.
- Invalid page metadata is dropped; overlong or wrongly typed metadata is refused.
- Repeated accepted submissions with the same ignored Idempotency-Key create
  distinct rows. Ambiguous commits use the CRUD database envelope; no replay.
- Administrator role is checked from current database state. The admin list
  retains explicit nulls, identified user/organization fields, source name
  construction, created-time descending order, and totals beyond the last page.
- No email, analytics, outbox, or notification path is called by these handlers.
- Missing required bodies are validated after authentication; malformed JSON
  remains before authentication, through the shared parser.

## Verified gates

All builds/tests ran on the staging test user through `heavy run 3600`.
No measurements or live-service/database operations were performed.

| Gate | Exit | Evidence |
| --- | --- | --- |
| Initial build without required RELEASE_SHA | 101 | `work/rust-phase2/feedback-red-build.log`; setup error, not behavioral RED |
| Inherited source compile | 101 | `feedback-red.log`; private transaction import |
| Inherited tests compile | 101 | `feedback-red-http.log`; SQLx dynamic SQL and temporary borrow |
| Behavioral RED | 101 | `feedback-red-behavior.log`; POST returned 200 rather than 201; vector generator needed FastAPI attribute mode |
| Python schema vector generation | 0 | `feedback-vectors.log`, `feedback-green-retry.log` |
| Missing-body precedence RED | 101 | `feedback-empty-red.log`; 422 rather than unauthenticated 401 |
| Final focused Rust HTTP/database suite | 0 | `feedback-final-green.log`; 14 passed, 0 failed, 0 ignored |
| Focused Clippy, warnings denied | 0 | `feedback-final-green.log` |
| Workspace formatting check | 0 | `feedback-final-green.log` |
| Final host wrapper | 0 | `feedback-final-green.log`, `CALL_EXIT=0` |

The gateway-interrupted `feedback-green.log` has no completion marker and is
not used as passing evidence. Its host unit was confirmed absent before retry.
The HTTP/database tests use a small fixture schema, not the real Alembic/ACL
chain; this does not substitute for the twin-database contract gate. Offline
workspace runs do not execute database-backed bodies without their explicit URLs.
The source-text coverage claims from the incomplete handoff were removed.

## Runner qualification (19 October follow-up)

Merged runner PR #100 as `be398b2c`; real feedback registration and adapters are
in `21c1cdf5`. Group `feedback` selects `feedback-routes`, never harness smoke.
The final run executes 31 actions through the real Python and Rust handlers.
Both cells use real migrations and runtime ACLs. Every action compares HTTP
through the pinned same-meaning comparator and all committed business rows,
including complete users rows, on fresh connections. Lane assertions require
exact inserted IDs, stripped messages, pages, timestamps, unchanged unrelated
tables and an empty captured-mail ledger. Admin fixtures include organization
and name nulls plus created-time ties.

Rollback is a fixture CHECK failure after admission. The retained-commit action
replaces the response after a successful handler return, requires exactly one
committed row, and is sent once without replay. It is a postcommit error envelope,
not a TCP disconnect or commit-acknowledgement fault. Task 10 has no plan-required
race, database lock-order schedule, mixed-writer dispatcher, or invite delivery.
Those gates are not claimed by this scenario. Shared auth and HTTP boundary
matrices remain owned by Tasks 6 and 7; this is route qualification, not a full
release-manifest GO or cutover authorization.

| Follow-up gate | Own exit | Evidence |
| --- | ---: | --- |
| Group attempts 1–4 | 1 each | Adapter build, cell-name validation and pool-budget setup failures |
| Group attempt 5 | 1 | Request-ID round-trip correctly rejected test loss envelope |
| Group attempt 6 | 0 | 31 actions before extra null-author fixtures |
| Combined final attempt group | 1 | Seed timestamp was string, not datetime |
| Combined focused Rust | 0 | 14 passed |
| Combined feature-enabled Clippy | 0 | Includes contract adapter, warnings denied |
| Combined formatting | 0 | Workspace formatting check |
| Typed-seed retry | 1 | Missing datetime import |
| Final seeded feedback group | 0 | `work/rust-phase2/feedback-runner-seeded.log` |
| Final seeded wrapper | 0 | `CALL_EXIT=0` |

Machine evidence: `work/rust-phase2/feedback-routes.json` (not committed).
No measurements, deployed settings, Python app changes or schema migrations.
The test adapter connects only to the assigned disposable application-role URL;
its state uses development validation because runner cell names lack `_test`.
Production configuration and validation are unchanged. UUID overrides compile
only with `contract-tests`, and clocks/IDs are controlled by lane test adapters.

## Integration handoff

The integration owner merges the marked Task 10 module and registry blocks.
Supply one shared limits authority to route composition and HTTP middleware.
Feedback stays on Python at cutover until Task 22 provides the shared cross-process
Python–Rust quota counter. No schema change is needed.

Draft CI-only PR: https://github.com/Verdaxis/verdaxis-backend/pull/103.
The PM integrates through `feat/rust-phase-2`; do not merge that PR.

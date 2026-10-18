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

## Pending runner lane: not passing gates

`tests/rust_contract/phase2/feedback.json` contains synthetic schema vectors and
scenario definitions as data. It is not registered with the old runner, whose
default Phase 2 path can fall back to harness smoke. No feedback comparison pass
is claimed from that path.

Pending the runner lane:

1. Register feedback group/scenarios with explicit dispatch, no smoke fallback.
2. Supply identical action clocks and generated UUIDs to both stacks. The route
   currently offers a fixed clock for focused tests, but production UUIDs remain
   random; use the runner's test-only ID facility when it lands.
3. Run `run_disposable --phase2 --phase2-group feedback` on the staging host
   through `heavy run`, with real migrations and runtime ACLs.
4. Compare complete HTTP responses and committed state in twin databases after
   each action, including users and all unrelated business tables, captured email
   attempts, no analytics, notifications, and outbox changes.
5. Complete the full manifest auth/support/CORS/method, pagination/tie-boundary,
   failure, repeat, and state matrix; the list of required cases is not evidence
   that these differential cases have run.

## Resume point

The integration owner merges the marked Task 10 blocks in `lib.rs`, `crud/mod.rs`,
and `routes/mod.rs`. Supply one shared limits authority to route composition and
HTTP middleware. Do not activate feedback until Task 22 passes. Resume this lane
when the runner merges into `feat/rust-phase-2`; adapt only this lane's scenario
registration and test seams, then run its pending gates. No schema change is
needed by the implementation.

GitHub CI and the draft PR are recorded in the task's final chat report. CI-only
PRs target `staging`; the PM merges through `feat/rust-phase-2`, not that PR.

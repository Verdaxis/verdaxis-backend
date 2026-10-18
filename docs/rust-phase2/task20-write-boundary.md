# Task 20 — order-book write boundary

## Scope and status

Branch: `feat/rust-p2-write-boundary`, based on `dee697f3`.

This is a boundary scaffold, not an enabled market command. Market HTTP writes
remain on Python. There is no matching engine, trade-hit adapter, lifecycle
implementation, runtime configuration change, dependency change, migration,
deployment or measurement in this lane. Task 21 is not included.

The production `execute_order_command` entry point returns 501 before database
access. The PostgreSQL session and all lock/write methods are private. The
transaction protocol is exercised by internal recording doubles; external tests
exercise source ownership, constants and Python-derived vectors. A successful
protocol-double test is not evidence of a committed PostgreSQL transaction.

## Changes

- Python exporter and source-digest-pinned spec for advisory hashes, canonical
  windows, side-independent slice identity, sorted multi-slice locks,
  idempotency scopes and request hashes.
- Private transaction scaffold with 500 ms lock and 5 s statement bounds,
  two attempts for SQLSTATE 40001/40P01, receipt replay/conflict handling,
  outbox/receipt insertion and ambiguous-commit refusal.
- SQLx-owned transactions provide rollback on drop. Scaffold connections close
  on drop, including uncertain commit paths; no runtime pool setting changes.
- Order-row locks retain the declared sequence; only slice identities and the
  separate user/organization lists are sorted. No business operation has yet
  supplied or proved its concrete row plan.
- Source checks reject orderbook writes and row locks outside `order_write/`.
  They do not permit arbitrary integration-test orderbook mutations.
- Scenario data is in `tests/rust_contract/phase2/order-write-scenarios.json`.
  It is not registered with the shared runner.

PD4 is not implemented as an emitter in this scaffold. No production command
can emit an event. Before enabling any command, its order-bearing events and
best-price calculation must consume the existing shared public-order scope,
not a copied predicate. Historical PD5 remediation is not this lane's decision.

## Verification evidence

All local builds, format checks, lints and tests ran on the staging test host
through `verdaxis-staging-test` and `heavy run 3600`. No project code ran on the
Mac. Full suites run in GitHub CI, not on the host.

The recovered `work/phase2-pass2.log` had no `CALL_EXIT` marker, incorrect inner
exit markers, a Python constant-parser failure and a missing-RELEASE_SHA Rust
build failure. A read-only host check found no surviving lane unit. That log is
not passing evidence.

Recovery and correction logs (uncommitted under `work/`):

| Run | Own gate exits | Outer marker |
| --- | --- | --- |
| `task20-recovery-red.log` | Rust 101, Python 1 | `CALL_EXIT=1` |
| `task20-compile-check.log` | Rust 101 (26/29 pass), Python 0 | `CALL_EXIT=1` |
| `task20-seal-red.log` | privacy regression 1, as expected | `CALL_EXIT=1` |
| `task20-private-check.log` | unit 101, Rust 101, Python 0 | `CALL_EXIT=1` |
| `task20-green.log` | private unit 0, external Rust 101 | `CALL_EXIT=1` |
| `task20-final-focused.log` | unit 0, Rust 0, lint 101, Python 0 | `CALL_EXIT=1` |
| `task20-committed-gates.log` | unit 0, Rust 0, lint 0, format 0, Python 0 | `CALL_EXIT=0` |

The main focused run tested implementation commit `cf50f7c2`:
14 private protocol tests, 15 external Rust tests, and 8 Python tests passed.
A final review found that millisecond-aligned command timestamps lost Python's
six-digit fractional precision. `task20-timestamp-red.log` proves the failure
(exit 101). `task20-timestamp-green.log` proves 16 external Rust tests and strict
Clippy pass (both exit 0); its format gate returned 1 for import ordering.
That ordering was corrected and separately checked in `task20-format-final.log`.
The timestamp fix is commit `2ca2a593`.
Python reports one existing Pydantic class-based-config deprecation warning.
The earlier handoff does not supply a valid feature-RED run. The boundary
privacy correction has a recorded failing test before its implementation.

## CI and commits

Draft PR: https://github.com/Verdaxis/verdaxis-backend/pull/101

CI only. Do not merge. The PM merges this through `feat/rust-phase-2`.

- `f2b59414`: `test: pin Python market lock and request hash vectors`
- `cf50f7c2`: `feat: establish one sealed order-book write boundary`

CI completed successfully for implementation SHA
`cf50f7c26b64c776c353c665da67e55bf850390f`:

- Rust run `39167011015`: offline and contract passed; publish skipped by design
  on a pull request.
- Backend run `39167010902`: test, postgres-phase0 and postgres-analytics passed.

`gh pr checks 101 --watch --interval 60` returned exit 0. These existing full
suite gates do not execute the unregistered Task 20 database scenarios.
Automated commit/push security review returned HTTP 404; it is not a passing
security review. Local details: `/Users/jonathanjie/.claude/security/log.txt`.

## Pending the runner lane — resume point

The following gates remain **pending the runner lane**, not passing or skipped
successes:

1. Register the scenario group without a harness-smoke fallback.
2. Run `run_disposable --phase2 --phase2-group order-write-boundary` through
   staging `heavy run` after reconciling the runner's accepted scenario format.
3. Prove real PostgreSQL timeout values, contention, rollback before commit,
   atomic outbox/receipt state, replay without duplicate events, and uncertain
   commit behavior using fresh-connection committed-state evidence.
4. Exercise both SQLSTATE retry schedules with the two-attempt bound, and
   cancellation/connection cleanup. Doubles currently prove protocol ordering
   only, not database effects.
5. Before a future command is enabled, use authenticated actor authority,
   revalidate the concrete actor and organization inside its transaction,
   prove that operation's actual row-lock order, and enforce PD4 via the shared
   public-order relation. Do not enable the caller-supplied scaffold result or
   event descriptors as a business implementation.

Codesight refresh was checked but deferred: its helper regenerates shared
CLAUDE.md/AGENTS.md and context-map files outside this lane's allowed blocks.
ARCHITECTURE.md records the new structure; the integration owner must refresh
Codesight when combining lanes.

No shared runner, comparator, Config/AppState, runtime projection, CI workflow
or Python application code was changed. There is no candidate GO or cutover
approval from these focused results.

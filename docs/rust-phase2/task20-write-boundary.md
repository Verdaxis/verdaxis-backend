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
  The executable `order-write-postgres` scenario is registered in the shared runner.

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

## Runner acceptance — 19 October 2026

Merged `origin/feat/rust-p2-runner` with merge commit `76af0844`.
Implementation and scenario commit: `1f6181989492be12d98787a1c47070b17401edc8`.
The production command still returns 501 before database access. The only new
HTTP adapter is an example invoking a `contract-tests`-gated boundary module.
No market write route, lifecycle operation, schema or runtime setting is added.

The group is registered as one marked Task 20 block. The runner has no fallback.
Command:

```sh
venv/bin/python -m scripts.rust_contract.run_disposable --phase2 \
  --phase2-group order-write-boundary --phase2-races \
  --report work/rust-phase2/task20-postgres.json
```

Every call ran on staging with `heavy run 3600` and finished. The final
`task20-runner-final-gates.log` records these independent exits:

| Gate | Result | Own exit |
| --- | --- | ---: |
| Private protocol | 14 passed | 0 |
| Rust vectors/source ownership | 16 passed | 0 |
| Feature-enabled Clippy | no warnings | 0 |
| Pinned Rust formatting | passed | 0 |
| Python boundary and registry | 9 passed | 0 |
| Registered PostgreSQL group with races | passed | 0 |
| Whole wrapper call | `CALL_EXIT=0` | 0 |

`task20-postgres-committed.log` repeats the database gate on committed
`1f618198` with own exit 0 and `CALL_EXIT=0`. Its SHA-bound JSON report is under
`work/rust-phase2/task20-postgres.json` (temporary, not committed).

Database evidence covers:

- Nine twin actions, with HTTP envelopes compared by the unchanged comparator
  and every business-table column captured through fresh reader connections.
- Real division-by-zero after outbox insertion: no outbox, receipt or unrelated
  row survives rollback. Successful command inserts exactly one of each.
- Explicit receipt replay returns the saved result and adds no event.
- A held slice advisory lock produces the expected 409 without a committed delta.
  Real transaction settings are asserted as `500ms` and `5s`.
- Database scheduling establishes a holder before the boundary command starts,
  observes that command's ungranted advisory lock in `pg_locks`, then releases
  the holder. A client barrier is not used as proof of SQL ordering.
- Server-raised SQLSTATE 40001 reruns the complete transaction once and commits
  one unit; server-raised 40P01 on both attempts returns 503 with no state delta.
  These are injected server errors, not claims of a naturally occurring deadlock.
- Postcommit response-failure simulation retains exactly one complete unit and
  records one attempt. The explicit later replay adds nothing. This is a
  controlled response-failure classification, not a severed COMMIT socket test;
  ambiguous COMMIT no-retry itself remains covered by the private protocol test.
- Three actors contend on the same receipt key, roll back, and preserve all
  state; three repeats use seeds 20261019–20261021 and recorded actor order.
- Python and Rust each write to the third cell; the existing Python dispatcher
  consumes both outbox events. Two receipts and two outbox rows remain. No mail
  is sent; twin mail ledgers are asserted empty. Invite delivery is out of scope.

The runner labels selected groups `route-contract`/`route_qualification=true`.
For this group that means **boundary acceptance only**, never acceptance of a
market HTTP route, matching engine, lifecycle, candidate or cutover.

Correction runs are retained in `work/`: registry RED exit 1; initial group exit
2 (missing CLI choice); subsequent adapter attempts exit 1 (handshake path,
rollback status, typed timestamp binding). Corrected runs 6, final, retry and
committed each exit 0. No failed attempt is counted as passing evidence.

CI on `1f618198`: Rust run `39173872723` and backend run `39173872730` both
completed successfully. Offline, contract, test, postgres-phase0 and
postgres-analytics passed. Publish was skipped by design. The CI watch returned
0. Automated security review returned HTTP 404, not a passing review; details
are in `/Users/jonathanjie/.claude/security/log.txt`.

Task 20's accepted boundary scope is complete. Before a future business command
is enabled, use authenticated actor authority, prove that operation's actual
row-lock plan and enforce PD4 through the shared public-order scope. No order
mutation or wider lifecycle is smuggled into this evidence adapter. Task 21 and
all measurements remain excluded.

Codesight regeneration remains integration-owned because it changes managed
instruction files outside this lane's allowed blocks. ARCHITECTURE.md records
the new module. No comparator, Config/AppState, runtime projection, CI workflow
or Python application code was changed. Shared runner edits are limited to the
marked Task 20 registration and group entry.

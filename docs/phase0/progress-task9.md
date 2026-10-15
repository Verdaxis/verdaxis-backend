# Phase 0 Task 9 — integrated acceptance

## Status

**Blocked, not complete.** The signed evidence review is
`outputs/phase0/acceptance.md`. It records eleven categories, all review finding
owners, exact candidate SHAs/digests, frontend reuse and Linux rehearsal gates.
No independent reviewer signature or release approval is claimed.

Candidate backend: `4be3fbb5ac39558fbacba6d3b6ae1f5057a2525c`.
Candidate frontend: `7b7d91b33db60ce3cb2d4408ca90a7a7b2562272`.
Union: `p0_20261016_union`; structural digest:
`54a40050c8af02f0132fe3c8516089212cf0e7494c9b1e24eee6b42885d85ffa`.
Rust may investigate this candidate but must not call it a final accepted oracle.
Decision 23's legacy-list tie-break difference does not affect Phase 0 acceptance.

## Test-first implementation

No standalone runner test file existed. Added `tests/unit/test_phase0_runner.py`
for the existing `tests/phase0/runner.py`, not a new acceptance engine. Initial
run: **8 failed** for missing coverage/count checks. After implementation:
**8 passed**. Commit: `4be3fbb5`.

The no-argument runner now includes all Phase 0 files plus the three Phase 0
PostgreSQL files. It checks JUnit for nonzero successful tests, no skips, complete
suite coverage and the six required local boundary names. The fifth Review Focus
case is the frontend two-bundle run, reviewed through immutable prior evidence.
Commands, counts and hashes are written beside synthetic fixture evidence.
Existing subprocess failure and missing-table checks remain in force.

## Completed runs

- Phase 0 runner: **153 passed**, no failures/skips; 306.75 seconds.
- Required full unit suite: **2155 passed, 19 failed**, no skips; 81.28 seconds.
  Every failure is the known macOS Bash 3.2 `test_deploy_health.py` case.
- Monitor: **290 passed, 1 skipped** (passwordless nobody unavailable).
- All remaining PostgreSQL suites: **256 passed, 5 failed, 1 error**, no skips;
  507.01 seconds. Both initial Alembic drift checks passed. Failures are old head/
  downgrade expectations; the SSE error needs Linux `/proc`.
- Disposable HTTP: **24 passed, 49 failed, 6 errors**, no skips. Old JWT helpers
  omit required claims, reserved fixture emails fail validation, and repeated
  logins hit the rate limit. The full suite was run, not bypassed or relabelled.
- Reused frontend evidence: **53/54**, one PM-accepted old ETS display exception;
  new bundle 27/27. Both environment builds and full frontend suites are prior
  lane evidence, not rerun claims.

All commands finished before this report. Every owned container was removed with
its volumes. The unrelated existing container was not touched. Raw synthetic
logs/reproduction scripts remain under `work/task9-*`; exact hashes and commands
are recorded in acceptance. No shared venv install, push, host access or real data.

## Deviations and conflict

- Runner tests were created because no dedicated runner self-tests existed;
  existing no-skip conftest behavior is preserved.
- The general PostgreSQL shell runner was copied to `work/` only to bind this
  root, increase readiness wait and remove volumes. Its tracked source is unchanged.
- Temporary HTTP fixture setup lives only in `work/`. Authentication, email and
  rate-limit controls were not weakened. No other lane's source was changed.
- Task 9 requires a complete passing acceptance gate, but existing PostgreSQL/HTTP
  tests need fixture updates outside the listed runner/doc ownership. This report
  records the scope conflict instead of changing those tests or claiming success.
  Integration owner/PM must authorize that follow-up, preserving historical lock
  proofs and current security contracts. Linux-only proofs remain Task 10 work.
- Read all six available independent lane reviews. Concrete earlier findings
  are closed or explicitly deferred with an owner in acceptance. Host-mode apply
  remains closed; the Task 10 command examples cannot run as currently implemented.

## Resume

Resolve the fixture ownership conflict, fix historical tests with independent
predecessor databases, modernize HTTP fixtures without security bypass, then
rerun complete gates and freeze the accepted oracle. Obtain an independent
reviewer/PM signature before Task 10. The report lists outstanding browser and
host proofs rather than converting them to inferred passes.

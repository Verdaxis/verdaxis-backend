# Release fixture health-pipeline flake

## Scope and cause

Branch: `fix/rust-release-fixture-flake`, based on `aea484d7`.
This is a test-only change in `tests/unit/test_rust_release.py`. No deployed
helper, unit, application, migration, or live service changes.

The original gate is recorded in `readiness-task14b.md`: the combined release
and unit-policy suites failed in the matching-host-key test, then passed without
source changes. The resumed investigation also reproduced that exact test's
failure, as well as failures in other users of `RustReleaseFixture`.

`deploy.sh` enables `pipefail` and pipes the readiness response into
`venv/bin/python scripts/validate_health_response.py`. The real helper reads
stdin before returning. The fixture's fake interpreter instead recorded the
command and returned without reading stdin. If the reader exited before the
writer wrote the response, the writer received SIGPIPE. The deploy then reported
a health failure despite a valid response and a successful fake validator.
The host-key comparison was not the failing operation.

The fixture is function-scoped and uses each test's `tmp_path` for its Git
repositories, keys, command ledger, curl counter, runtime artifacts and state.
The throwaway signing key and host-key copy agree within each fixture. File modes
are set explicitly. Time-based release markers only make commits distinct.
Inherited environment variables and the cached runtime-preparer module were
reviewed; the observed failures occur at the downstream health pipeline, not at
key, artifact, environment, or mode checks. Process scheduling explains why the
failure can move between tests and disappear on an unchanged rerun.

## Fix and regression

The fake health validator now drains stdin before logging and applying the
existing injected health failure. It still returns the injected exit code 44.
It does not retry, skip, change deploy policy, or weaken assertions.

A focused regression sends 1 MiB through a `pipefail` pipeline to the fixture's
fake validator. This exceeds pipe capacity and exposes a reader that exits
early without relying on a scheduler delay. Both success and injected-failure
cases are checked.

## Saved pre-fix evidence

All suites ran on the staging test host through `verdaxis-staging-test` and
`heavy run 3600`. The shared host reported Python 3.12.13; pytest was 9.1.1.
`pytest-randomly` was not installed. No local suites or measurements ran.

| Gate | Individual pytest exit codes | Wrapper exit |
| --- | --- | --- |
| Original-order combined baseline, 52 tests | 1 (51 passed, 1 failed) | 1 |
| First 10 combined repetitions | 0, 0, 1, 0, 1, 0, 0, 0, 0, 1 | 0 |
| Next 10 diagnostic repetitions | 0, 0, 0, 0, 0, 1, 1, 1, 1, 0 | 0 |
| Next 12 diagnostic repetitions | 0, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0 | 0 |

The diagnostic loops deliberately continued after failures, so their wrapper
exit 0 does **not** mean all pytest invocations passed. The matching-host-key
test failed in the first repetition batch (run 5) and the next batch (runs 7
and 9). The stop-after-60-unreproduced-runs condition therefore did not apply.
Local logs: `work/00-combined-baseline.log`, `work/02-loop10.log`,
`work/03-loop10-exitcodes.log`, `work/05-loop12-pipestatus.log`.

## Fixed-fixture gates

- Original suite order: release suite, then unit-policy suite; 30/30 runs passed,
  each with 52 tests and pytest exit 0. Two 15-run wrapper calls each exited 0.
  Logs: `work/06-verify-A1.log`, `work/06-verify-A2.log`.
- Shuffled node order: first 15/15 runs passed, each with 52 tests and pytest
  exit 0; wrapper exit 0. Log: `work/07-verify-B1.log`.
- Regression against the old fixture: pytest exit 1; the success pipeline
  returned 141 (SIGPIPE), while the injected-failure case retained exit 44.
- Regression against the fixed fixture: pytest exit 0; both cases passed.
- Remaining shuffled node order: 15/15 runs passed with explicit seeds 16–30,
  each with 54 tests (the original 52 plus the two regression cases) and pytest
  exit 0. The driver disables `pytest-randomly`, collects both suites, and
  shuffles the node IDs. Wrapper `CALL_EXIT=0`.
  Log: `work/08-finish-verification.log`.

Total fixed-fixture repetitions: **30 original-order + 30 shuffled-order**, all
pytest exits 0. The last 15 also include the new regression. Every host call
finished; no call from this lane remains running.

## Delivery

Commit only this report and the scoped test change; do not commit `work/` or
`.claude/`. Push only the named branch, open the requested PR into `staging`,
read CI to completion, and do not merge. No architecture update is needed:
structure and runtime relationships are unchanged. CI results and the PR URL
are recorded in the final handoff.

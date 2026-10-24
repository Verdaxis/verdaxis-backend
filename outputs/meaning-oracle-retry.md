# Meaning harness transient oracle retries

Scope: PD2-M3 follow-up; branch `fix/meaning-oracle-transient-retry`, base `97521636a608c6e9d9f6845b224cbc747a215c63`.

## Contract

- Python startup: `ORACLE_STARTUP_TIMEOUT_SECONDS`, default 180 seconds per attempt; positive finite values only. One initial start and at most two restarts. Failed children are terminated and reaped before restart.
- Python capture: one initial request and at most three retries, only for a proven timeout, connection reset, or a 503 with an exact known temporarily-busy JSON detail and a valid `Retry-After` value. Delay-seconds and HTTP dates are honored. Other HTTP responses (including non-busy 503/500) and transport failures (for example connection refused) are not retried.
- Rust captures and startup are not retried. The first complete, nontransient Python answer is compared normally. Exhaustion is `ORACLE_UNAVAILABLE`, fails the gate, and is separate from a meaning difference.
- Reports list group, request, Python leg, reason and retry attempt. Startup events also record the timeout; capture events record the delay. Attempt 1 means the first retry, not the initial request.
- Comparator, rules and rules digests are unchanged. Verified in the worktree: `tests/rust_contract/phase1_meaning/normalization.json` still hashes to the pinned `f68125dd…99e0e`; the pinned Phase 2 inventory (`tests/rust_contract/phase2/routes.json` `sources`) covers app routers/schemas/models/services and `requirements.txt`, `deploy/migration-checkpoints.tsv` — none of `run_disposable.py`, `record.py` or `simulate.py`. No re-pin is needed; PR #122's input-then-inventory method does not apply.

## Verification ledger

All tests run through the existing isolated test-host wrappers. No tests or builds run on the Mac. Each wrapper call starts detached and its `CALL_EXIT=` marker is checked before completion is claimed.

| Check | Host | Result | Evidence |
| --- | --- | --- | --- |
| Initial regression RED | prod test host | 7 assertion failures, 6 passed; CALL_EXIT=1 | `work/oracle-red.log` |
| Initial full suite attempt | prod test host | failed: missing imports removed by an intermediate save hook; 859 passed, 1 skipped, 9 failed, 16 errors; CALL_EXIT=1 | `work/oracle-green.log` |
| Actual busy-body RED | prod test host | actual producer bodies failed expected assertions; missing exception import also exposed | `work/oracle-red-busy.log` |
| Final harness suite | prod test host | 897 passed, 1 skipped; CALL_EXIT=0 | `work/oracle-green2.log` |
| Startup-report RED then local fix | prod test host | 1 assertion failure (`groups` lost prior results); fixed in worktree | `work/oracle-report-red.log` |
| Full suite after the report fix | prod test host | 899 passed, 1 skipped; CALL_EXIT=0 (includes the previously failing report test) | `work/oracle-green3.log` |
| Final suite after `gate_passed` default init | prod test host | 899 passed, 1 skipped; CALL_EXIT=0 — exact bytes of the commit candidate | `work/oracle-green4.log` |
| Staging meaning gate | staging test host | live (run 5): load-wait loop admitted only below 8; readings 9.33→8.18→10.91→14.37→13.18→14.84 in the first minutes; hard deadline 10:27:20Z | `work/oracle-staging-meaning5.log` |

## Resume point

Staging meaning gate in flight (second attempt; the first was cut off by the GPT Sol usage limit mid load-wait, not by any harness failure — no `CALL_EXIT=` marker, and both the local wrapper and the detached remote unit were confirmed stopped before relaunch). When it ends: record `MEANING_EXIT` and the report outcome, then commit, push branch `fix/meaning-oracle-transient-retry`, open PR to `staging`, and record CI. No live deployment is authorized or attempted.

## Local-poll interruption note (24 Oct, 09:07Z)

The run-5 wrapper's local poll falsely reported GONE (CALL_EXIT=70) while the
remote unit kept running; three GONE polls under heavy load were not proof of
death. Direct host inspection confirmed the unit active and still load-waiting
(readings up to 220.11). The remote run remains the authority; it is being
polled directly over SSH until it writes its exit file. The wrapper's remote
log/exit files were removed by the exited wrapper only if it stopped the unit;
it did not (stop_remote_run succeeded only for units it could stop — here the
poll exited without stopping, so log and unit survive).

# Meaning harness transient oracle retries

Scope: PD2-M3 follow-up; branch `fix/meaning-oracle-transient-retry`, base `97521636a608c6e9d9f6845b224cbc747a215c63`.

## Contract

- Python startup: `ORACLE_STARTUP_TIMEOUT_SECONDS`, default 180 seconds per attempt; positive finite values only. One initial start and at most two restarts. Failed children are terminated and reaped before restart.
- Python capture: one initial request and at most three retries, only for a proven timeout, connection reset, or a 503 with an exact known temporarily-busy JSON detail and a valid `Retry-After` value. A present `Retry-After` is honoured up to `MAX_RETRY_AFTER_SECONDS` (30 s); a larger, non-finite, negative, non-numeric or HTTP-date value uses that cap. A missing header is not a retry instruction. Other HTTP responses (including non-busy 503/500) and transport failures (for example connection refused) are not retried.
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
| Staging meaning gate | staging test host | one authorized run (run 5): load-waited 08:27–09:34Z (readings 9.3→220→…), admitted at 09:34:41Z load1=7.42; 9 of 11 groups OK; **one live `ORACLE_RETRY` recorded** and it recovered, but note it came from the **Task 9 `prices_oracle.py` probe**, not the main 56-case comparator recording: the log order is `contract group prices OK: 56 case(s)` → `Task 9 populated probes matched: 17` → `ORACLE_RETRY {"group":"prices","request":"fixture-11","leg":"python","reason":"503 temporarily busy","attempt":1,"delay_seconds":1.0}` → probes matched again; that id/leg is the probe's second `compare_populated()` call (`rs/read/tests/prices_oracle.py`), which uses the same `record()` → `_perform_oracle` path, and the Python log shows the admission middleware's 503 on `/api/prices` with `Retry-After: 1`. The run then aborted spawning group 10: `FileNotFoundError` for `rs/target/debug/examples/contract_server` — the shared build artifact (symlinked to the `rust-phase2` lane's target) disappeared mid-run under concurrent host load; MEANING_EXIT=1; no meaning difference and no `ORACLE_UNAVAILABLE`; environment race, not a harness or comparison result | `work/oracle-staging-meaning5-full.log` |
| Review fix RED (`Retry-After` cap, persisted shape) | prod test host | 6 intended assertion failures, 27 passed | `work/oracle-cap-red.log` |
| Review fix focused GREEN | prod test host | 33 passed on `tests/rust_contract/test_oracle_retry.py` | `work/oracle-cap-green2.log` |
| Review fix full suite | prod test host | 904 passed, 1 skipped; CALL_EXIT=0 | `work/oracle-cap-full.log` |

## Resume point

All gates are done. The single authorized staging meaning-gate run was spent with the result recorded above (environment race after 9/11 groups; the new retry behaviour demonstrably fired and recovered). Next: push branch `fix/meaning-oracle-transient-retry`, open the PR to `staging`, record CI. No live deployment is authorized or attempted.

## Local-poll interruption note (24 Oct, 09:07Z)

The run-5 wrapper's local poll falsely reported GONE (CALL_EXIT=70) while the
remote unit kept running; three GONE polls under heavy load were not proof of
death. Direct host inspection confirmed the unit active and still load-waiting
(readings up to 220.11). The remote run remains the authority; it is being
polled directly over SSH until it writes its exit file. The wrapper's remote
log/exit files were removed by the exited wrapper only if it stopped the unit;
it did not (stop_remote_run succeeded only for units it could stop — here the
poll exited without stopping, so log and unit survive).

## Publication (24 Oct)

Branch `fix/meaning-oracle-transient-retry` pushed; PR #125 to `staging` opened.
CI: Backend CI (test, postgres-phase0, postgres-analytics) — all pass.
Rust read CI: offline pass, contract pass (7m04s); `publish` correctly skipping
(a feature-branch PR cannot publish; release publication requires a push to a
live branch).

## Review fixes (24 Oct, PR #125)

Head `51aa7d139`. Fix 1: `Retry-After` is capped by the named
`MAX_RETRY_AFTER_SECONDS` (30 s); a larger, non-finite, negative, non-numeric
or HTTP-date value uses the cap instead of stalling or aborting the gate; a
missing header still means no retry. Fix 2: `stable()` strips
`response_headers`, so retired headers live only in memory. The Task 9 probe
that produced the live retry is now named in the staging row above.
Tests first: 6 intended assertion failures, 27 passed; then the focused file
33 passed and the full `tests/rust_contract` suite 904 passed, 1 skipped.
CI on `51aa7d139`: test, postgres-phase0, postgres-analytics, offline and
contract all pass; publish skips (feature branch).

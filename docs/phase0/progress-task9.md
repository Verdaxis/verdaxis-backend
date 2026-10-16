# Phase 0 Task 9 — integrated acceptance

## Current status — 2026-10-16

N1 cleanup makes [the acceptance report](../../outputs/phase0/acceptance.md) the
single source for the current release/oracle table, eleven-category status table,
Linux counts and resume point. Candidate: `e61400f36ee528dfdb2476dd0dbaf2df457a2c54`.
The final oracle follows M2 and independent-review fixes; it is not frozen yet.
M1 and M3 are done. B1/B2 production host tools are implemented/merged and under
independent review. M2's runbook appendix is in progress in another session;
`docs/runbooks/phase0-cutover.md` is not edited here.

The host-tool/CI/appendix changes are schema-stable operational changes. P0-2's
stored-email output and M1's inventory semantics are application golden changes.
Rust must rerun the affected goldens at the final identity; unchanged schema alone
is not an application compatibility proof.

Linux test calls run one at a time through the approved test tool, not on this Mac.
Current results and the only resume point are in the acceptance report. Historical
counts below do not describe this candidate. No direct SSH or live service access.

## Linux completion — 2026-10-16

All calls finished through the authorized tool, one at a time. Unit: **2232 passed**
(the 19 macOS Bash failures are absent). Phase 0: **177 passed**. Other PostgreSQL:
**265 passed**, including four-worker durable SSE. Three migration drift checks
passed. Runtime-ACL HTTP: **79 passed** after test-only port fix `d1d44c75`:
Linux's automatic ephemeral range includes ports below the disposable guard's
49152 minimum; the harness now selects within the unchanged allowed range.

Monitor: **286 passed, 4 failed, 1 skipped** on the initial run. Four
external-recovery tests reached `deployment_active()`, which reads the module
constant `DEPLOY_STATE_FILES` pointing at the fixed production checkout; the
directory exists but is unreadable to the isolated user, so `Path.exists()`
raised `PermissionError` before the mocked recovery step.

Test-only fix: `tests/monitor/test_external_recovery.py` now redirects
`DEPLOY_STATE_FILES` to a private temporary directory through the helper
`isolate_deploy_state(recovery, tmp_path)`. Production paths, permissions and
behavior are unchanged; only the in-process constant is replaced. A new test
`test_present_deploy_state_refuses_recovery` keeps the guard verified: with an
absent temp directory `deployment_active()` is False and recovery proceeds, and
with a present temp state file recovery refuses (return 2, no command runs,
`outcome=ineligible`, `deployment_guard=refused`). This strengthens, not
weakens, the previously incidental coverage.

After the fix, focused recovery tests: **7 passed**. Full monitor suite:
**291 passed, 1 skipped, 0 failed**, exit 0 (`work/task9-linux-monitor2.log`).
The single skip is the unchanged passwordless-`nobody` different-UID case at
`tests/monitor/test_runtime_identity.py:212`; it remains a Task 10 item because
the isolated test user cannot switch to another UID. No live service action or
privilege bypass occurred, and no application or host-tool source changed.

N1 structure commit: `0ad4b6fb`; Linux harness correction: `d1d44c75`.
Unit ran at exact `e61400f3`; later calls used documentation-only descendants,
plus that one HTTP harness fix. The report distinguishes these from candidate
application/schema bytes. M2/runbook ownership was preserved. No local test,
build, Docker use, SSH, host setup change or push was performed.

## Historical evidence — 2026-10-16 — superseded checkpoints

All status and resume language below belongs to the named historical checkpoint.
It is retained for audit only; use the linked current acceptance tables above.

### Historical — 2026-10-16 — CI PostgreSQL partition correction

Read failed run `38813274619` with `gh run view --log-failed`: analytics collected
Phase 0 suites without `PHASE0_PORT`. The runner now explicitly excludes the
three `tests/postgres/test_phase0_*` files and `tests/phase0`. Their guards are
unchanged. New Linux job `postgres-phase0` runs the complete Phase 0 runner with
full Git history, CPython 3.12, constrained dependencies and its expected venv.
Both runners use the same pinned PostGIS image; analytics cleanup now also removes
volumes. The old unit-job fence step moves into the complete Phase 0 job, avoiding
duplicate execution. A unit regression compares the complete/disjoint file sets
and checks the new job's history, venv and invocation.

TDD: partition tests **2 failed → 2 passed**. Exact local runner commands finished:
- `bash scripts/run_product_analytics_postgres_tests.sh tests/postgres`:
  **264 passed, 1 error**, only the known macOS `/proc` SSE fixture; no Phase 0
  collection or guard failures. Both initial migration drift checks passed.
- `bash scripts/run_phase0_postgres_tests.sh`: **153 passed**, zero skips/failures.
Logs: `work/task9-ci-{failed,analytics,phase0}.log`. Owned containers and volumes
were removed. No push; PM owns publication. Linux CI is not claimed verified
until the PM pushes and the new run finishes.

### Historical — 2026-10-16 — Sign-off M1 correction — current inventory authority

Commit `48c2cc305dd5c6085ae18352b9e16f8f4e1c915e` supersedes P0-2's
key-share read. PostgreSQL requires UPDATE for that read; the runtime correctly
has SELECT only on ports. Use a plain existence SELECT. If deletion wins before
INSERT, roll back the transaction and map only SQLSTATE 23503 with constraint
`inventory_items_port_id_fkey` to the same 404. Other DB failures retain their
existing treatment. No ACL, schema or reference write authority changed.

New real app-role PostgreSQL/ASGI HTTP tests assert SELECT=true, UPDATE=false,
existing-port success, missing-port refusal and a coordinated separate-connection
deletion after the existence read. Refusals leave no inventory or outbox event.
After correcting the test token setup, the pre-fix run returned **3 failures**
(HTTP 500). Final run: **3 passed**. A fixture-isolation correction clears old
outbox rows before each case; it does not change runtime behavior.

The reproducible HTTP harness is now `venv/bin/python tests/phase0/http_runner.py`.
It migrates as owner, seeds synthetic rows as administrator, applies and validates
the unchanged ACL policy, then runs the server as `phase0_app_test`, never owner.
Full HTTP: **79 passed** on both the initial and committed harness runs. Focused
unit regression: **18 passed**. Both runner setup drift checks passed. Logs are
`work/task9-m1-{red2,final,http-committed,unit}.log`.

All runs finished and owned containers/volumes were removed. B1/B2 source files
were not touched. Other independent-review findings remain open in their lanes;
M1 closure is not whole-branch sign-off. Rust inventory goldens need this new
application identity; the schema digest is unchanged. Full unit/monitor/Phase 0
suites were not repeated for this narrow fix; their previous counts are historical.

### Historical — 2026-10-16 — P0-2 — approved user-facing fixes and completed local rerun (historical)

PM decision **P0-2** permits two user-facing Python bug fixes during the freeze.
Commit `a73703ba` fixes both; their tests failed first (**2 failed**) and then
passed with the behavior suite (**40 passed**).

- Inventory creation checks the referenced port before INSERT and returns
  **404 `Port not found`**, matching `app/routers/ports.py`. A key-share row lock
  keeps a valid reference from being deleted during creation. No inventory,
  reservation or event is written for a missing port.
- `UserResponse` is an output-only BaseModel with a plain string email. It no
  longer inherits input email validation/normalization and preserves stored bytes.
  `UserCreate` and `LoginRequest` retain strict EmailStr validation. No stored
  user row, email, KYC decision or copy mapping changes.

Commits `cc4c9245` and `82f41484` repair HTTP cookie/origin/catalog fixtures and
migration expectations. Historical cutoff, fee, receipt and organization guard
proofs use independently migrated pre-union databases. Their specific refusal,
lock and successful downgrade assertions remain intact; union head and union's
own unconditional refusal are tested separately. One intermediate full run
caught five historical tests still using the new head; the final correction
keeps those historical boundaries explicit instead of weakening assertions.

Final completed results:
- HTTP: **79 passed**, zero failures/errors/skips (24.03 seconds).
- All non-Phase-0 PostgreSQL files: **261 passed, 1 error**, zero test failures;
  error is the unchanged Linux `/proc` four-worker fixture (625.83 seconds).
- Phase 0 runner: **153 passed**, no failures/skips (282.11 seconds).
- Full unit: **2158 passed, 19 known macOS Bash failures** (74.82 seconds).
- Monitor: **290 passed, 1 known passwordless-nobody skip** (15.43 seconds).
- Both ordinary drift checks and the union fresh/upgraded-schema checks pass.

Candidate oracle: `82f414847c2c8c2203a7ca42edd6be87a9830ba4`. Unit/monitor/Phase 0
ran at `cc4c9245`; the only subsequent source change is the explicit historical
revision constants in one PostgreSQL test. The final full PostgreSQL run exercised
those exact bytes and HTTP reran at the candidate. Application/schema bytes did
not change after the full unit run. All runs finished, including the long
PostgreSQL command. Owned containers/volumes were removed; no host or push.

The PM's local no-new-failures bar is met for the measured regression set;
HTTP is fully green. Linux-only deploy/SSE/monitor proofs and independent sign-off
remain explicit gates, not passes. The exact baseline comparison is HTTP plus
five migration cases; this is not a claim that every suite was rerun on staging.
See the current section of acceptance for the remaining frontend/host limits.

### Historical — 2026-10-16 — PM-authorized follow-up checkpoint (historical; superseded above)

Baseline on exact staging `b218ab2`: HTTP **24 passed, 49 failed, 6 errors**,
with all 55 failing/error node IDs identical to Phase 0. The five migration tests
all passed on staging: their failures are new union test-expectation drift.
Temporary baseline worktree was removed after saving evidence under `work/`.

Commit `4ee95ef1` repairs configured JWT claims, accepted synthetic email domains
and per-test limiter reset through a secret-protected test-only ASGI endpoint.
Focused checks: **9 passed**. HTTP rerun: **46 passed, 28 failed, 5 errors**.
No skips. Tests still need cookie-contract and stale catalog fixture repairs.

Stopped under the PM's product-defect rule: the missing synthetic `sg-sin` port
exposed `POST /inventory` returning 500 for an invalid FK reference. The source
handler is inherited from both staging and prod. PM must decide whether to track
this product error separately and permit completion of the fixtures. No product
code or error expectation was changed. Full acceptance reruns have not run after
this partial repair; prior full-run counts are not new verification.

Reserved-email facts are in acceptance: staging reproduces the profile 500;
prod has the same EmailStr output-validation path by source inspection. Database
email strings can hold reserved domains, and copy preserves them. The inspected
prod canary uses a valid `canary.verdaxis.exchange` subdomain. Actual production
row contents are unknown; no live inspection was performed.

Resume: after PM decision, seed the synthetic port/publication metadata, update
refresh-cookie and canonical-ID tests, implement independent predecessor lock
fixtures and union expectations, then rerun all six acceptance groups and drift
checks. Every started run finished; owned containers and volumes were removed.
No push, package install, host operation or application change occurred.

### Historical — 2026-10-16 — Status

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

### Historical — 2026-10-16 — Test-first implementation

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

### Historical — 2026-10-16 — Completed runs

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

### Historical — 2026-10-16 — Deviations and conflict

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

### Historical — 2026-10-16 — Resume

Resolve the fixture ownership conflict, fix historical tests with independent
predecessor databases, modernize HTTP fixtures without security bypass, then
rerun complete gates and freeze the accepted oracle. Obtain an independent
reviewer/PM signature before Task 10. The report lists outstanding browser and
host proofs rather than converting them to inferred passes.

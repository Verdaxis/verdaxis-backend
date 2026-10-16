# Phase 0 integrated acceptance and Rust handoff — 2026-10-16

## Current release and oracle — 2026-10-17

**Release `56d75770` is LIVE in production since 17 October 02:39 SGT and is the
final accepted Python oracle.** This section and the current eleven-category
table are the only current status. Everything under the dated historical section
is retained evidence, not an instruction or release selection. The post-cutover
branch (`fix/phase0-post-cutover`, cut from `56d75770`) carries the window's
source and runbook corrections on top of the accepted oracle; it is tooling and
documentation hygiene, not a new release selection, and no live deployment is
authorized here. See the Task 11 cutover record below for the window evidence.

| Identity | Current value |
|---|---|
| Backend candidate | `56d7577085af99411a6f9c3e34e64ad929d09954` (`56d75770`), merged head of `feat/phase0-convergence-20261016`, deployed as `origin/staging` |
| Final accepted Python oracle | **`56d7577085af99411a6f9c3e34e64ad929d09954`** — live in production since 17 Oct 02:39 SGT (window 02:19–02:39 SGT) |
| Live frontend release | `5f3cd698272cba0026bb2eaa527463e3e2b0e68b` (`5f3cd698`), promoted by workflow run `38948394269` |
| Frontend M3 source | `513c2d77d2f85cf8c1d68a27528000a65ab1e365` |
| M3 measured backend | `64c4a8653f9257f1b1519f9be75b5cbcd9e70f71` |
| Union revision | `p0_20261016_union` |
| Structural schema SHA-256 | `54a40050c8af02f0132fe3c8516089212cf0e7494c9b1e24eee6b42885d85ffa` |
| Migration graph SHA-256 | `748dbcdfba75e5b9b2b563c814a5d42808c05a7c4139218fee23402cc7742df9` |
| Copy-manifest file SHA-256 | `a5e27570803d5d00c43f83ebbee734acf97bea9891cf51ce8347b596ba958b1c` |
| Bootstrap-policy file SHA-256 | `e794312c20819ad342a4bbd083a1bbe073b29ec96aefe804e154899c035472c5` |
| Revision allowlist SHA-256 | `b840a03ce47add08fe3e89e40aeb7fd92ceb17985a15aa43c7eb16984d0b4713` |
| Pilot configuration | UTC; KYC execution gate off; eight distinct product IDs; live DB `verdaxis_prod` |

M1 is done: runtime SELECT-only port lookup and exact FK-race rollback/404,
with constrained-role tests. M3 is done: measured profile matrix, copied nullable
RFQ history, typed snapshots and KYC 503/500/post-commit 504 cases. Frontend evidence
is in its `docs/phase0/progress-task8.md` and `cached-bundle-options.md`.
The gate is **109/111**, new 56/56, old 53/55. Both old failures remain visible:
unknown ETS shown as zero and KYC review receipt shown as approval. The PM's
16 October 06:40 SGT extension explicitly accepts the second cached-tab window;
retain the banner, new-entry-HTML check and founder refresh note. No blanket waiver.

B1/B2 host tools are implemented and merged, and the independent re-review of
2026-10-16 found no blocking code defect. Its fresh production CLI chain passes
**47** cases and the 18-case scratch smoke exits 0 at `b0d66251`. The earlier
15-pass synthetic CLI proof is prior evidence, not a new result. M2's finite host
appendix is complete; its two re-review gaps (the scratch smoke fixture password
and this stale record) are fixed by the change set that contains this file.

### Task 10 rehearsal attempt 2 — 2026-10-17 (prod host, PM-run)

The PM ran P0-5 rehearsal attempt 2 on the prod host at release
`3ef69848b30a78a2ad6bf17652bab857906936f3` (17 Oct 00:08–00:21 SGT), against dump
`verdaxis-20261015-200015.dump`. Evidence is in `work/rehearsal2/` (step scripts
and logs; no secrets). A1/A2 passed with live's locale `en_US.UTF-8`; restore
took 4 s; the restored source outbox was `0|0` of 1778 rows, and the source
database ACL equalled live. The source inventory gate passed on real data.
Bootstrap 36 s, copy 64 s and verify 34 s all exited 0 with equal counts:
26,242 orders, 127 trades, 44 users, 50 organizations; the three legacy NULL
checks were 0/0/0. A3: runtime ACL convergence passed; readiness was exact
(`status=ok`, `db=ok`, `production`, the release SHA); 4 Uvicorn workers; the app
role reconnects as UTC and is denied `UPDATE ports` and `SELECT
alembic_version`; KYC storage is off (no key) with the gate off.

Business smoke: 17 of 18 cases passed; `public:copied-orders` reported
`served=0`. The PM compared the scratch backend with live by hand and confirmed
the application is right and the test was wrong: bids 11,521 vs live 11,520 and
asks 11,522 vs live 11,521 (the +1 on each side is the smoke's own order), and
the first page was 48 of 50 identical.

Three findings and their fixes:

1. Smoke sampling defect — `create_scratch_identities` sampled the three oldest
   orders of any status and expected them on the first page, but real copied
   data has FILLED orders among the oldest rows, which the public path correctly
   hides. The smoke now samples the newest copied orders that the application's
   own public predicate makes visible (`public_order_scope`, not a second copy of
   the rules) and checks the public per-side totals against the visible copied
   count plus the smoke's own orders. A bounded page walk finds each sampled
   order without depending on page position, and fewer than 3 visible copies
   still fails. Boundaries unchanged: scratch-only base URL and fixture, no
   secret or response body in the report.
2. `p0run` could not write evidence — `ProtectHome=read-only` made `/root`
   read-only inside the transient unit, so the source observation and every tool
   report under `/root/verdaxis-phase0` failed with "Read-only file system". The
   appendix `p0run` definition now adds
   `-p ReadWritePaths=/root/verdaxis-phase0`; nothing else under `/root` or
   `/home` gains write access.
3. Target `public` schema not reset — PostgreSQL 15+ gives a new database's
   `public` schema to `pg_database_owner` with PUBLIC USAGE, so
   `deploy/postgres/converge_runtime_object_acls.sql` refused at its
   public-owner guard. The shared A2/B4 target block now resets the schema, after
   the CONNECT and TEMP lines, to live's owner and ACL
   (`verdaxis_migrator`, `{verdaxis_migrator=UC/verdaxis_migrator,verdaxis_app=U/verdaxis_migrator,verdaxis_backup=U/verdaxis_migrator}`),
   with that text as the stop condition. Findings 2 and 3 are pinned in
   `tests/unit/test_phase0_appendix_commands.py`.

### Task 11 production cutover — 2026-10-17 — `56d75770` LIVE

The PM ran the production cutover under the appendix's fenced procedure. Window
**02:19–02:39 SGT (17 October 2026), 19.5 minutes total**; the API was closed to
public traffic about **8 minutes** (maintenance Caddyfile in place from B2 until
the B10 reopen). Release `56d7577085af99411a6f9c3e34e64ad929d09954` is live on
`verdaxis_prod` at revision `p0_20261016_union`, and frontend
`5f3cd698272cba0026bb2eaa527463e3e2b0e68b` is promoted. The PM's step scripts
and logs are host-private; only bounded facts are recorded here.

**Lossless evidence.** The frozen source and the copied target agree:
**26,242 orders, 127 trades, 44 users, 50 organizations** on both sides
(bootstrap → copy → verify all exit 0). The pre-deploy `final.dump` restored
into a scratch database with **62 of 62** per-table counts equal to the frozen
live source, and the post-copy B9 backup restored with every target table count
equal to the independent verify report (`restored-counts-accepted.json`).
Public orderbook per-side totals equal on old and new. Readiness reported exact
`status=ok`, `db=ok`, `production`, `56d75770…`; four Uvicorn workers; old
`verdaxis` ended with zero sessions and a fully drained outbox. The one frontend
promotion workflow (run `38948394269`, dispatched 18:34:32Z, report 18:38:12Z)
promoted `5f3cd698` with successful production and canary smokes.

**In-window deviations (all resolved by the PM inside the window, then fixed in
source by the post-cutover branch):**

1. `assert_host --check tuple/readiness` refused on git's dubious-ownership
   protection: the committed helper ran git as root in the service-user-owned
   checkout. The PM prefixed only the git calls with
   `sudo -n -u verdaxis-prod` (no root `safe.directory`) and every other
   assertion passed. Fixed in source: the helper now resolves the ROOT owner
   and runs every git call as that owner, refusing root or failed lookups.
2. The new backend unit could not start without a key file: systemd 255.4 logs
   the empty `SetCredential=kyc-document-keys:` as "Invalid syntax, ignoring",
   and `LoadCredential` with a missing file fails 243/CREDENTIALS (proved with
   a throwaway unit). The PM created an **empty root:root 0400**
   `/etc/verdaxis/kyc-document-keys.json`; `LoadCredential` then succeeds and
   the vault treats the empty credential as missing (storage 503, readiness
   unaffected). Fixed in source: both units drop the invalid line, and the
   installer preflight refuses an absent source with the exact placeholder
   command. Staging (systemd 249, real 93-byte ring) never hit the case.
3. The B4 target block lacked live's two `ALTER DEFAULT PRIVILEGES FOR ROLE
   verdaxis_migrator IN SCHEMA public GRANT SELECT ON TABLES/SEQUENCES TO
   verdaxis_backup` statements, which `validate_roles.sql` requires; the PM
   applied them by hand before B4a acceptance. The B7 curve probe also needed
   `?product_id=<first product id>` (422 without it, on old and new). Both are
   folded into the appendix by the post-cutover branch.

**Demo activity fix after the window.** The rearmed
`verdaxis-demo-activity.service` failed its runtime identity check: the fresh
clone's `.git/HEAD` is mode 0600 and the old tree carried
`user:verdaxis-demo-production:r-x` plus a default ACL on `.git`. The PM copied
that ACL to the new `.git` only (`.env` and other secrets stay closed) and the
job then passed. The appendix now carries that ACL copy in B5 and re-checks the
demo identity as the demo user in B10b before any rearm.

**Open items after the cutover:**

- **B11 key ring with the founder**: `/etc/verdaxis/kyc-document-keys.json` is
  still the empty placeholder; KYC storage is 503 and readiness unaffected
  until the founder's real ring replaces it (security runbook B11 procedure).
- **Cleanup only after a quiet period and founder approval**: remove
  `prod/be.phase0-old`, drop the old `verdaxis` database and the
  `17/p0rehearsal` scratch cluster. Nothing is deleted yet.

Name `56d75770` as the final accepted oracle. The Task 10 rehearsal record
below remains the data-path evidence; the window above is the live proof.

### Change and golden boundaries

P0-2 stored-email output and M1 inventory error/authority changes are application
changes: Rust must rerun affected profile/inventory goldens. Host-tool B1/B2,
CI partitioning and M2 operations are schema-stable operational changes. They
need authority/recovery/command proofs, not a claim of changed customer economics.
There is no `app` or `alembic` diff from the M3 measured backend to this candidate.
Schema digests are unchanged. Future review fixes can invalidate that conclusion;
freeze the final full SHA only after their affected gates finish. Keep Rust's
legacy `created_at DESC, id DESC` delta explicit, UTC and KYC-off fixtures, and
migrator-backed revision admission without app SELECT on `alembic_version`.

## Current Linux acceptance results

All builds/tests run through `verdaxis-prod-test "$PWD" 'COMMAND'`, one call at a
time. The latest recorded counts are the merged-head unit run (**2272 passed** at
`b0d66251`), the last pre-merge unit run (**2233 passed** at `a9e20adb`), the
host-tools production CLI chain (**47 passed**, `tests.phase0.linux_database`) and
the Phase 0 PostgreSQL runner (**177 passed**). The independent re-review of
2026-10-16 reproduced the production CLI chain (47 passed,
`work/rereview2-linux-database.log`) and the 18-case scratch business smoke
(`work/rereview2-smoke.log`), both exit 0, at `b0d66251`. The change set that
contains this file adds one acceptance case (a malformed secret file must refuse
with `PHASE0_SECRET`), so the next full chain run reports **48**. No application,
migration or deploy byte differs from the M3 measured backend, so the Task 9 rows
below stay valid. No local test environment was created. The tool uses its
isolated capped Linux amd64 test user and rootless disposable containers; no
direct SSH, production/staging connection or test-host setup change is permitted.
Every call below has finished with a recorded `CALL_EXIT` marker.

| Check | Identity | Result | Evidence |
|---|---|---|---|
| Full unit at merged head | `b0d66251` | **2272 passed, 0 failed/skipped**, exit 0 | merged-head Linux unit call |
| Full unit before merge | `a9e20adb` | **2233 passed, 0 failed/skipped**, exit 0 | pre-merge Linux unit call |
| Host tools production CLI chain | `b0d66251` | **47 passed**, exit 0 | re-review `work/rereview2-linux-database.log` |
| Scratch business smoke | `b0d66251` | **18 cases**, exit 0 | re-review `work/rereview2-smoke.log` |
| Full Phase 0 runner | Task 9 run set | **177 passed, 0 failed/skipped**, 723.54 s, exit 0 | `work/task9-linux-phase0.log` |
| Other PostgreSQL suites | Task 9 run set | **265 passed, 0 failed/skipped**, 1048.49 s, exit 0; includes four-worker durable SSE | `work/task9-linux-postgres.log` |
| Runtime-ACL HTTP suite | Task 9 run set plus port fix | **79 passed**, 56.81 s, exit 0 | `work/task9-linux-http{,-retry}.log` |
| Full monitor / different-UID case | Task 9 run set | **291 passed, 1 skipped, 0 failed**, 15.84 s, exit 0 | `work/task9-linux-monitor2.log` |
| Ordinary and union migration drift | Task 9 run set | **3 checks passed**, no new upgrade operations | Phase 0 and PostgreSQL logs |

### Linux limits, failures and reproducibility

The 19 macOS-only Bash deploy failures do **not** occur on Linux: all 2272
merged-head unit tests passed, matching the PM's reported count. The `/proc` four-worker durable
SSE test also passed within the 265-case PostgreSQL run. These close the measured
Linux portions of sign-off section 6 item 9, not actual systemd installation.

The different-UID monitor test was attempted by the full monitor run and skipped
at `tests/monitor/test_runtime_identity.py:212`: passwordless `nobody` execution
is unavailable. Task 10 must run it with authorized real UID switching. No sudo
policy, root privilege or host setup was changed or simulated.

The initial monitor run failed four tests in `tests/monitor/test_external_recovery.py`
(`test_recovery_is_allowlisted_and_hourly`, `test_frontend_and_capacity_fail_closed`,
`test_database_and_invalid_caddy_fail_closed`,
`test_healthy_loopback_uses_one_validated_caddy_reload`). Each reached
`deployment_active()`, which reads the module constant `DEPLOY_STATE_FILES`
pointing at the fixed production checkout. That directory exists but is
unreadable to the isolated user, so `Path.exists()` raised `PermissionError`
before the mocked recovery step — a test-isolation gap, not a real recovery.

Test-only fix: the file redirects `DEPLOY_STATE_FILES` to a private temporary
directory via `isolate_deploy_state(recovery, tmp_path)`. Production paths,
permissions and behavior are unchanged; only the in-process constant is replaced.
A new `test_present_deploy_state_refuses_recovery` keeps the guard verified —
absent temp state proceeds, present temp state refuses with return 2, no command
runs and `deployment_guard=refused`. Focused recovery run: **7 passed**. Full
monitor suite after the fix: **291 passed, 1 skipped, 0 failed**, exit 0
(`work/task9-linux-monitor2.log`). The skip is the Task 10 `nobody` case above.
No permission bypass or direct host inspection was attempted.

The first HTTP call failed before collection because Linux port-0 allocation
selected a port below the existing 49152 lower bound. Test-only fix `d1d44c75`
selects a free numeric-loopback port in the existing allowed range, with a bounded
retry. Guards, runtime ACLs and application behavior remain unchanged. The second
call passed all 79 tests. This correction must accompany any reproduced Linux run.

Commands, each inside the authorized test tool:
- Unit: the documented explicit test environment, SQLite memory URL and
  `venv/bin/python -m pytest tests/unit -q`.
- Phase 0: `bash scripts/run_phase0_postgres_tests.sh`.
- Other PostgreSQL: explicit test environment and
  `bash scripts/run_product_analytics_postgres_tests.sh tests/postgres`.
- HTTP: `ENVIRONMENT=test RELEASE_SHA=$(git rev-parse HEAD) venv/bin/python -m tests.phase0.http_runner`.
- Monitor: the same unit environment and SQLite URL, then
  `venv/bin/python -m pytest tests/monitor -q -rs`.

Local streamed log SHA-256 (all contain completion markers):
- Unit: `c3e6c968b44a4ce7450353d787a2ba66d9b79cc529a244339da71cf72802f64c`.
- Phase 0: `b5cbb90e5d4d9a17610cf03deebf76b294a0d06edf2361960941988287ef4032`.
- PostgreSQL: `8f25d99e97d98babf08245137839140a9b33976de6d3be3359872468cca7e2cc`.
- HTTP first attempt: `56a8800cb23dcfb8b4c8825cc89fb99dfb49302fb5fe6f7e6d04754a7fc402cd`.
- HTTP passing retry: `326045f8be27dc78377c5b7ddb2a29ebec4bb74d37822398adaa5e2fb2b8a385`.
- Monitor: `de62b1b8fe315ef8ee78fb893e1c6abb67f84dd02c03c8ae5bc0f6028129bfa2`.

## Current eleven-category status

| Category | Status and evidence | Remaining owner/gate |
|---|---|---|
| Inventory | Real host catalog attested through the cutover tool chain (62/62 tables) | None for Phase 0; Rust uses `56d75770` |
| Schema | Union live at `p0_20261016_union`; drift checks passed | None; promotion complete |
| Copy | Live cutover copy: 26,242 orders / 127 trades / 44 users / 50 orgs equal | None; frozen source preserved until cleanup |
| ACL | Live `verdaxis_prod` accepted by `validate_roles.sql` after the window's default-privilege grants | Appendix amendment keeps the grants in the block |
| Products | Eight IDs and scalar/typed contracts retained | 265 PostgreSQL and 79 HTTP cases passed |
| Auth | P0-2 input/output separation; M3 profile matrix complete | Live logins verified by the reopen probes |
| Privacy | Notification and typed snapshot cases complete | Live egress isolation was the fence; retained evidence host-private |
| Timezone | UTC reconnect/role proofs retained | Live backend runs UTC (window readiness evidence) |
| Vault | M3 browser fault cases complete; live host runs the empty placeholder (storage 503) | Founder/PM: B11 real key ring and restart |
| Frontend | `5f3cd698` promoted (run `38948394269`); smokes passed; banner/refresh note sent | Owner watches the accepted cached-tab window |
| Rollback | Pre-write rollback was not needed; old tuple/DB preserved | PM: quiet-period cleanup with founder approval |

## Current resume point

The cutover is complete: `56d75770` is the live, final accepted oracle (record
above). The post-cutover branch folds the window's three deviations and the demo
fix into source and the runbook; its commits are tooling/documentation only and
add no application change. Remaining owners:

- Founder/PM: B11 key ring replacement (empty placeholder is live now), then KYC
  storage verification through the approved restart.
- PM: quiet-period cleanup of `prod/be.phase0-old`, old `verdaxis` database and
  the `17/p0rehearsal` scratch cluster, only with founder approval.
- Rust/Phase 1: regenerate catalog policy, affected goldens and SQLx metadata
  from `56d75770` per the boundaries below; keep the explicit legacy
  `created_at DESC, id DESC` delta.

Do not treat the post-cutover commits as a release; promote them through the
normal staging→prod review if they are to reach the live host. No host operation
is authorized from this document.

Integration evidence recorder: Task 9 integration owner (AI assistant), with the
2026-10-17 cutover record above taken from the PM's window evidence. This is not
another person's signature or deployment approval.

## Historical evidence — 2026-10-16 — superseded checkpoints

The following is the preserved record before `e61400f3`. Status, candidates and
resume instructions inside it applied only at those checkpoints. Use the current
tables and single resume point above instead.

**Current decision: local fixture blockers closed; independent sign-off and Linux
proofs pending. No host operation or deployment approval.**
P0-2 and the final rerun below supersede the earlier blocked counts and candidate
identity. Earlier evidence is retained as history, not current failure status.
All runs finished; owned containers were removed with their volumes.

### Historical — 2026-10-16 — Sign-off M1 correction — current application candidate

**Candidate: `48c2cc305dd5c6085ae18352b9e16f8f4e1c915e`.** Independent sign-off
found that P0-2's key-share read needs UPDATE on ports. That regression is now
fixed with a SELECT-only existence read and exact port-FK error mapping after
transaction rollback. No port UPDATE grant or schema change was made.

New real constrained-role PostgreSQL/ASGI HTTP cases: **3 failed before fix,
3 passed after** (existing port, missing port, deletion after the read). They
assert SELECT-only port authority and no inventory/outbox record on refusals.
Focused unit tests: **18 passed**. Full HTTP through committed
`tests/phase0/http_runner.py`: **79 passed**, no failures/skips. The harness applies
and validates canonical runtime ACLs and uses `phase0_app_test` for the app,
not `phase0_owner`. Owner/admin connections remain setup-only. Both disposable
migration drift checks passed. All runs finished and containers/volumes were removed.

M1 is locally resolved; B1/B2/M2/M3 and independent re-review remain gates.
B1/B2-owned bootstrap/copy/verify files were not touched. Previous full-suite
counts below are historical; only the affected unit, real-role PostgreSQL and
complete HTTP suites were rerun for M1. Rust inventory goldens need the candidate
above; schema/revision digests remain unchanged. No host or Rust run was authorized.

### Historical — 2026-10-16 — P0-2 acceptance — historical local rerun (superseded for M1)

Candidate backend/Python oracle: **`82f414847c2c8c2203a7ca42edd6be87a9830ba4`**.
Frontend identity and schema/policy hashes below are unchanged. This candidate
replaces `4be3fbb5`; Rust must rerun affected profile/inventory goldens. P0-2 is
recorded in `docs/phase0/progress-task9.md` and changes no migration or data.

| Completed check | Passed | Failed | Errors | Skipped |
|---|---:|---:|---:|---:|
| P0-2 failing tests before implementation | 0 | 2 | 0 | 0 |
| P0-2 plus behavior tests after implementation | 40 | 0 | 0 | 0 |
| Full unit | 2158 | 19 known Bash 3.2 | 0 | 0 |
| Full monitor | 290 | 0 | 0 | 1 known platform skip |
| Full Phase 0 runner | 153 | 0 | 0 | 0 |
| Every remaining PostgreSQL suite | 261 | 0 | 1 Linux `/proc` fixture | 0 |
| Complete guarded HTTP integration | 79 | 0 | 0 | 0 |
| Ordinary migration drift checks | Both passed | 0 | 0 | 0 |

The initial baseline comparison remains exact: staging had the same 49 HTTP
failures and six errors, and passed all five migration cases. Those Phase 0
migration regressions are now closed, and HTTP has no exceptions. No additional
product defect was exposed after P0-2. The remaining unit/SSE/monitor limitations
are the same platform limitations previously recorded, not new regressions.
The entire suite was not rerun on staging; do not overstate the baseline sample.

P0-2 fixes: nonexistent inventory port is **404 `Port not found`**, matching the
port handler, before INSERT; a key-share lock protects the checked reference.
Output `UserResponse.email` is a stored string, with no input revalidation or
normalization. All email input validation remains strict. Both had failing tests
before implementation. No data rewrite, filtering, auth relaxation or KYC change.

HTTP tests now use current catalog IDs/labels, complete synthetic port and
publication metadata, and the cookie refresh contract. The test client explicitly
forwards Secure cookies over its attested HTTP-only loopback transport with the
required `https://test` Origin. Production Secure flags are not changed; this
is not browser TLS evidence. Per-test limiter reset remains confined to the
secret-protected disposable wrapper. The current cookie/ID tests pass all 79 cases.

Historical migration tests now build the predecessor independently. They retain
specific lock/refusal/data-preservation and successful downgrade assertions.
Union head/refusal expectations are separate. One intermediate run exposed five
other historical tests using the new head; those were corrected and the entire
PostgreSQL suite reran to completion. No assertion was replaced by a generic
nonzero exit check.

Evidence: `work/task9-p02-{unit,monitor}.log`,
`work/task9-p02-postgres-final.log`, `work/task9-p02-http-final.log`, and
`work/phase0-fixtures/6d8a0e36513b4d48b80a05b8a3b6c469/runner-evidence.json`.
JUnit SHA-256:
- Unit: `d68b1dd0c2e5bf8c7b610caed0b0bf349caf3aa76113a5d6a7b5762d8ab8a358`.
- Monitor: `2e08151f4095f4ebc44bec9537e1a225e08a0a4a2cafd8abe82b4fd1d7afced3`.
- PostgreSQL: `113da3e786c6f1a8e3c0a634c760af62075a3a97253cdfe3cc217a48146670a9`.
- HTTP: `40bf2350068486d09a2cd93d6864736a8f4ee86c7ba03394d1992ca7d5827d80`.

Unit/monitor/Phase 0 ran at `cc4c9245`; subsequent `82f41484` changes only one
PostgreSQL test file's historical revision boundaries. The final PostgreSQL run
used those bytes and the final HTTP run used `82f41484`. No application change
occurred after the full unit run.

Checklist update: **auth and products now pass the full HTTP gate; schema's five
local migration blockers are closed.** The eleven other category scopes below
remain unchanged. The known `/proc` SSE error, 19 Bash failures and monitor skip
are owned by the Task 10 Linux operator. Host-mode authority, loaded systemd,
backup/key recovery and full switch-back remain pending. Frontend evidence is
still the prior 53/54 PM-exception gate: P0-2 preserves valid-email and existing-port
contracts tested there, but its new error cases are unit/HTTP, not browser proof.
Broader frontend cases already assigned below remain unmeasured.

Integration evidence reviewer: **Task 9 integration owner (AI assistant)**.
Current disposition: **local regressions resolved; independent PM/reviewer
acceptance pending**. No signature from another person is implied.

### Historical — 2026-10-16 — PM-authorized fixture follow-up — stopped on product error path

Baseline worktree at `b218ab264b92f99007bacc05d4871413e024b72a` was created
under `work/`, tested with the same synthetic setup and pinned container, then
removed. Evidence was copied out before removal. No host or live DB was used.

| Follow-up run | Passed | Failed | Errors |
|---|---:|---:|---:|
| Unchanged HTTP suite on staging baseline | 24 | 49 | 6 |
| Five migration tests on staging baseline | 5 | 0 | 0 |
| HTTP after first fixture repairs | 46 | 28 | 5 |
| Focused reset/runner unit checks | 9 | 0 | 0 |

All 55 failing/error HTTP node IDs match the prior Phase 0 run exactly: these
are pre-existing staging failures, not Phase 0 regressions. All five migration
expectation failures are new with union; staging passes them. Raw evidence:
`work/task9-baseline-{http,postgres}.{log,xml}` and
`work/task9-baseline-http-server.log`. The baseline drift checks also passed.

Partial fix commit `4ee95ef1` adds configured issuer/audience and access-token
type to legacy token helpers, uses `example.com` synthetic addresses, and resets
the existing limiter between tests through the disposable ASGI wrapper only.
The reset requires the disposable secret and POST; its positive and denial
checks pass. Product routes and limiter enforcement are unchanged. The temporary
seed script uses matching addresses. No application code changed.

**Stop required by PM instruction:** valid authentication exposed inventory's
missing-reference error path. `POST /api/inventory` with `port_id=sg-sin` when
that port does not exist returns HTTP 500 after `inventory_items_port_id_fkey`.
This test setup lacks the port. Independently, the product accepts that reference
through request validation, then maps the FK error to 500 rather than a client
error. The same DBAPIError-to-500 handler exists at pinned staging lines 235–243
and pinned production lines 209–217. This is inherited, not a new Phase 0 change.
No handler or test expectation was changed to hide it. PM must decide whether to
track that product defect separately and permit fixture repair to continue.

Other exposed failures still need test repairs: five auth setup errors expect a
refresh token in JSON rather than the current cookie contract; 27 trade failures
use stale product/delivery IDs. These are not yet fixed or accepted.

#### Historical — 2026-10-16 — Reserved-email 500 facts

- Pinned staging reproduced the same two profile/login HTTP 500s as Phase 0.
  Its log identifies `UserResponse.email` validation of
  `itest-admin@disposable.invalid`. Registration of a reserved-domain address
  returns 422 instead, so ordinary current registration does not create it.
- Both pinned staging and production declare `UserBase.email: EmailStr`, and
  `UserResponse` inherits it. Staging `_user_profile` calls
  `UserResponse.model_validate(user)`; production does so too (line 1980).
  A stored invalid email can therefore raise output validation in both. Production
  behavior is established by source inspection, not a live or pinned-prod HTTP run.
- The database stores `users.email` as a string with uniqueness, not an EmailStr
  constraint. SQL/legacy fixtures can persist reserved domains. Copy retains all
  user rows, including synthetic provenance; it does not validate emails or filter
  canaries. Such a source row would survive and could hit this error after copy.
- The inspected pinned production signup canary uses
  `canary+<name>-<stamp>@<name>-<stamp>.canary.verdaxis.exchange`, not a reserved
  domain. No reserved-domain user seed was found in the inspected production
  `app`, `scripts` or `deploy` source. This is not proof of actual production rows.
  Their existence is unknown without separately authorized host-local inspection.
- Do not silently rewrite/drop user emails during copy or weaken validation.
  PM owns the disposition; an authorized host inventory must report only bounded
  counts externally and retain any row details privately on-host.

Resume after PM decision: add the missing synthetic port and required publication
metadata; update cookie assertions and catalog IDs; fix independently migrated
predecessor lock fixtures and union expectations; then rerun every acceptance
suite. Full unit/monitor/PostgreSQL/Phase 0 reruns were **not** done after this
partial repair because the explicit product-defect stop rule took precedence.
Prior counts below are historical, not validation of `4ee95ef1`. All started runs
finished and their owned containers/volumes were removed; no background run remains.

### Historical — 2026-10-16 — Frozen candidate identities

- Backend / candidate Python oracle: `4be3fbb5ac39558fbacba6d3b6ae1f5057a2525c`.
  Application bytes equal integrated base `4105a15a2d417d74abafc01297df21beda103cdf`;
  the candidate adds runner evidence checks only. Later documentation commits do
  not change this tested identity. This is a candidate, not an accepted oracle.
- Frontend candidate: `7b7d91b33db60ce3cb2d4408ca90a7a7b2562272`.
  The measured bundle source is `d079122e1aac8599fdb505db2fe1349052e64db3`;
  only `docs/phase0/progress-task8.md` differs. Old bundle:
  `3f16cabba6424ef08250b32f8f8e4b9a621c4379`.
- Frontend gate backend: `53e1960c42a3e2373db84802a5ecfca553e0244f`.
  Subsequent Task 7 changes concern deployment identity, CORS, seed safety and
  monitoring, not the measured HTTP contracts. Reuse is limited to those
  contracts; this gate does not prove deployed CORS or systemd.
- Revision: `p0_20261016_union`, parent `kycd_20261015_staging`.
- Permanent production database: `verdaxis_prod`; old `verdaxis` stays untouched.
  Both pilot environments require UTC and `KYC_EXECUTION_GATE_ENABLED=false`.
  Production receives the encrypted vault, initially empty; no invented files.
- Founder decisions 2–6 are confirmed by the task instruction on 16 October.
  This confirmation does not authorize a host connection or operation.

### Historical — 2026-10-16 — Completed local runs

| Command / evidence | Passed | Failed | Errors | Skipped |
|---|---:|---:|---:|---:|
| Runner tests, before implementation | 0 | 8 | 0 | 0 |
| `pytest tests/unit/test_phase0_runner.py -q` after implementation | 8 | 0 | 0 | 0 |
| `bash scripts/run_phase0_postgres_tests.sh` | 153 | 0 | 0 | 0 |
| Required SQLite `pytest tests/unit -q` | 2155 | 19 | 0 | 0 |
| `pytest tests/monitor -q -rs` | 290 | 0 | 0 | 1 |
| Complete existing PostgreSQL suites (Phase 0 files covered above) | 256 | 5 | 1 | 0 |
| Guarded `pytest tests/integration -q -rs` | 24 | 49 | 6 | 0 |
| Reused actual old/new frontend bundle assertions | 53 | 1 accepted exception | 0 | 0 |

Unit/monitor commands used `ENVIRONMENT=test RELEASE_SHA=test`, explicit test
JWT secret and SQLite memory URL. HTTP server and client used explicit distinct
`JWT_ISSUER=verdaxis-test-api`, `JWT_AUDIENCE=verdaxis-test-web`, the same test
secret, the full candidate SHA and an ephemeral numeric-loopback endpoint with
the required disposable token handshake. Socket guards allowed only the owned
DB for the backend and only the owned HTTP server for the client. No providers.

The PostgreSQL command was the existing `run_product_analytics_postgres_tests.sh`
through `work/task9-postgres.sh`, with only absolute root, 240-second readiness
and `docker rm -f -v` cleanup adjustments. It ran every `tests/postgres` file
except the three `test_phase0_*` files already run by the Phase 0 gate. Both
initial database `alembic check` commands reported **no new upgrade operations**.
The Phase 0 union fresh/upgraded-schema and drift checks also passed.

Raw evidence: `work/task9-{unit,monitor,postgres,http}.{log,xml}` and
`work/task9-phase0.log`. Phase 0 commands, suite coverage and hashes:
`work/phase0-fixtures/ed38c87f860e49ce906263f0f2c3365b/runner-evidence.json`.
Temporary evidence is deliberately not committed. HTTP reproduction scripts:
`work/task9-http.py` and `work/task9-http-seed.py`; synthetic rows only.

### Historical — 2026-10-16 — Eleven-category checklist

Signed below as an integration evidence review, **not** independent approval.

| Category | Local evidence / result | Remaining gate and owner |
|---|---|---|
| Inventory | PASS: exact 61-source/62-target classification, unknown table/column refusal | Task 10 operator: compare actual host catalog |
| Schema | PASS: supplied connection, real transaction, union, fresh/upgraded equality and drift | Integration owner: repair five stale historical migration tests without weakening guards |
| Copy | PASS: all 55 copy tables, both references, four skips, JSON/null/float/date/EWKB, cycles, sequences, six interruption boundaries | Task 10 operator: real-data blockers, restored dump, trusted host authority |
| ACL | PASS: copied-union app/backup denial and existing role policy tests | Task 10 operator: exact production roles/Settings in separate cluster |
| Products | PASS: eight IDs, scalar/typed separation, qualification and union direct takes | Existing HTTP suite remains blocked; integration owner |
| Auth | BLOCKED: unit/PostgreSQL contracts pass; legacy HTTP fixtures do not | Integration owner: explicit token claims, valid synthetic emails, login-rate isolation |
| Privacy | PASS: copied notification payload retention and response redaction; reminder concurrency | Task 10 operator: private logging and provider egress proof |
| Timezone | PASS: fresh app/migrator/backup sessions, reconnect and role override case | Task 10 operator: scratch proof; Task 11: recycle every pool/CLI |
| Vault | PASS for existing unit and PostgreSQL vault suites only | Task 10 operator: four-worker credential loading, backup/key recovery, 503/500 and proxy 504 ambiguity; frontend owner: missing browser fault coverage |
| Frontend | MEASURED GATE: 53/54, new 27/27; PM accepts old unknown ETS shown as EUR 0 | Task 11 operator/founder: verify new entry HTML and send refresh note; extra cases below not covered |
| Rollback | PASS for executable synthetic tuple/locks and outbox drain | Task 10 operator: actual Linux systemd/Caddy, full old→new→old switch and source hashes |

Integration evidence reviewer: **Task 9 integration owner (AI assistant), 2026-10-16**.
Disposition signed: **BLOCKED**. Independent reviewer / PM acceptance: **pending**.
No signature or approval from another person is implied.

### Historical — 2026-10-16 — Failed checks and explicit owners

1. **Integration owner, before Task 9 acceptance:** five PostgreSQL expectations
   predate union. Three `test_cutoff_downgrade_fails_fast_behind_concurrent_user_write`
   cases encounter union's unconditional downgrade refusal before the historical
   lock guard. Preserve the old lock proof in an independently migrated predecessor
   fixture; do not replace it with a weaker generic refusal assertion. The fresh
   migration test still expects `kycd_20261015_staging` as head. The quarantine
   test expects the old downgrade message rather than union's refusal.
2. **Task 10 Linux operator:** `test_four_worker_durable_sse_dispatch` errors because
   macOS has no `/proc`. It reached server readiness but did not prove dispatch.
   The 19 unit failures are exclusively `test_deploy_health.py` and the known Bash
   3.2 `mapfile`/lowercase-expansion limitation. Monitor's single skip is
   `test_runtime_identity.py:212`: passwordless nobody execution unavailable.
   Run all three checks on supported Linux before acceptance; none is a pass here.
3. **Integration owner, before Task 9 acceptance:** HTTP suite has 49 failures and
   six setup errors. Legacy token helpers omit issuer/audience and are rejected
   with 401. Reserved `disposable.invalid` emails produce registration 422 and
   profile serialization 500. Subsequent real logins hit 5/minute and return 429.
   These causes prevent meaningful execution of many downstream assertions;
   fixing fixtures may expose additional failures. Do not disable JWT validation,
   email validation or rate limiting. No claim of HTTP acceptance is made.
4. **Frontend owner / PM, before expanding acceptance claims:** reused Task 8
   evidence excludes copied RFQ history, the full auth-state matrix, all supplier
   FAME/LNG snapshots, KYC browser fault cases and every order-entry UI. SSE is
   blocked and external Mapbox/CORS are untested. Backend unit proofs are not
   substitutes for browser proofs. The PM exception covers only old ETS display.

### Historical — 2026-10-16 — Review finding ledger

Read every backend `progress-task*.md`, frontend Task 8 progress, and all six
`phase0-task*-review-2026-10-16.md` reports in workspace `outputs/research/`.
No review files for Tasks 4b or 6 were present; no independent review is invented.

| Review | Disposition and evidence |
|---|---|
| Task 2 M1 | Closed: `745143ed`, negative inventory/assisted routes and exact successful scalar match; current unit suite passes |
| Task 2 M2 | Closed: `a92dc44d`, literal production seed bands, CI/LCV and catalog tuples; current unit suite passes |
| Task 3 N1 | Closed: non-BUYER/SUPPLIER profile branch and gate-on/off role-null vectors |
| Task 3 N2 | Closed: `test_order_expiry_reminders_concurrency.py`, three PostgreSQL tests pass in complete run |
| Task 4a M1 | Closed: `4c29220`, physical asyncpg transaction gate; four AUTOCOMMIT vectors, mid-DDL rollback and lock lifetime pass |
| Task 4a additional suggestions | Deferred to integration owner: broader literal/stamp, disconnect and all-engine connection-count vectors; Task 10 owns production-mode exact-identity proof |
| Task 5 M1/M2/M3/N1 | Closed: `964c9cb`, `972bbf2`, `fad8a50`, `97ac741`; formatting/RLS/reference completeness/empty-history regressions pass on union |
| Task 5 final integration | Union hashes, populated scalar history, multibatch cycles and copied-union ACLs pass. Host authority, isolation and restored-data proof deferred to Task 10 operator |
| Task 7 M1/M2 | Closed: asyncpg assertion CLI and every-other-session drain tests pass |
| Task 7 M3/M4 | Local fixes closed: explicit daemon reload/loaded-manager instructions and executable tuple interruptions. Actual manager/proxy proof deferred to Task 10 operator |
| Task 8 B1 | Measured two-bundle blocker closed by union rerun, 53/54 with one PM exception; broader unmeasured cases remain explicitly assigned above |
| Task 8 M1 | Not repaired in cached old JS. Explicit PM acceptance; Task 11 operator/founder owns entry-HTML check and refresh note |
| Task 8 M2/N1 | Closed: `cc2bc5d6`, scalar map fallback/forecast guard and public demo ticker; no LNG/UCOME demo prices |
| Task 8 N2 | Correct fixture/type closed by `a20b48af`/`2c62a24b`; full browser auth-state matrix deferred to frontend owner |
| Task 8 release-test suggestions | Deferred to frontend owner: broader stubbed release execution and CI wiring verification; no release run authorized |

### Historical — 2026-10-16 — Frozen digests and Rust instructions

SHA-256 values (file hashes unless labelled semantic):

| Artifact | Digest |
|---|---|
| Structural schema (semantic) | `54a40050c8af02f0132fe3c8516089212cf0e7494c9b1e24eee6b42885d85ffa` |
| Migration graph (policy algorithm) | `748dbcdfba75e5b9b2b563c814a5d42808c05a7c4139218fee23402cc7742df9` |
| Reference values (semantic) | `5b522ebaed9732c8576f00bebf9100d7c36c7f67079add01f0234e3445bf1c58` |
| Copy manifest | `a5e27570803d5d00c43f83ebbee734acf97bea9891cf51ce8347b596ba958b1c` |
| Bootstrap policy | `e794312c20819ad342a4bbd083a1bbe073b29ec96aefe804e154899c035472c5` |
| Revision checkpoint allowlist | `b840a03ce47add08fe3e89e40aeb7fd92ceb17985a15aa43c7eb16984d0b4713` |
| Synthetic history corpus (`tests/phase0/synthetic.py`) | `33998481d3ad3c8a63e1fa7d390035590a67c5143240b1d0f8c5b5bcbeecf619` |
| Exact readiness corpus | `07172b9315750135599c7bae16d05754e718b09d1cb4bc87281e4a7fa526f50e` |
| Product/delivery policy (Task 2 algorithm) | `fbe6fb695a983267d0f7db973e3aac8b8b0ba803543ef7327756bde7e953bb20` |
| Phase 0 JUnit | `152c17ce8faeb370005cbe5022f5bedaa5a165d9b90bd5fa27c5f2e1c4e289cc` |
| Unit JUnit | `341729c5006621a84e8f7afaab6d1c74103b0558d82bf2e3e85f8497267dbe68` |
| Monitor JUnit | `5200883e7e6390c8bae29361113c70aca9f2521852319bc22e1c59354482c7d5` |
| PostgreSQL JUnit | `2edf50829dd8ff5037568efa918c4e2ff50b5d3a4486a10b8f0d5a0a799e8afd` |
| HTTP JUnit | `f731596aa70024cd295d0c7f767691918879b0355f1b037f350e9c78d67e5207` |
| Frontend raw two-bundle result | `287f546bfc9622c5c240c42735a0266bf124483a36ff64604482b6d798e3a70c` |

Rust owner: use the candidate SHA above for investigation, but **do not label it
an accepted final oracle yet**. After the blocked gates pass, regenerate catalog
policy, affected goldens and SQLx metadata from the accepted SHA. Target union,
UTC (`DB_SESSION_TIME_ZONE` too), pilot KYC off, all eight distinct product IDs,
absent-liquidity null prices, historical snapshots, notification redaction and
copied reference IDs. Replace the Phase 1 plan's old staging revision and
Singapore-session assumptions after compatibility proof. Keep migrator-backed
prestart and the compiled revision allowlist; no runtime SELECT on
`alembic_version`. Phase 0 does not authorize a Rust production rollout.

Rust plan decision 23 carries `created_at DESC, id DESC` on two legacy list
routes. That Python tie-break fix is not in this Phase 0 candidate. It does not
change Phase 0's schema, copy or economic contracts and does not explain these
acceptance failures. Rust must record that intentional tie-order delta when
comparing goldens; do not quietly treat its branch as this canonical oracle.

### Historical — 2026-10-16 — Task 10 requirements and resume point

First close the local failures above and obtain independent acceptance. Then
obtain separate host approval and complete reviewed host commands. Host apply is
still disabled in bootstrap/copy; editing policy JSON is not authorization or
implementation. The integration owner must resolve that source gate before any
host apply attempt. No host mode was implemented in Task 9.

Linux rehearsal must prove exact production Settings/roles/names in a separate
cluster; live-target collision refusal; trusted OID/cluster bindings; blocked
provider/live-DB egress; complete restored-backup hashes; effective cached systemd
fence/drop-in loading; four workers; ordinary deploy two-lock and exact readiness
checks including union-to-union; stopped timers/cron and all writers; outbox drain;
fresh UTC sessions; encrypted key-ring recovery and vault fault semantics; and
full old→new→old tuple restoration with unchanged frozen source hashes. Keep
public mutations closed. After reopening, rollback needs reconciliation, never
automatic loss of new writes. Host backup producer and monitor wiring remain
operator-owned. Preserve Python feature freeze and do not port Rust work here.

No packages, skills, subagents, pushes, host operations or real data were used.
No frontend files were changed. Reused frontend build evidence: both environment
builds/artifact checks pass, 918 Vitest tests, 14 Node checks, 16 helper tests;
synthetic-token bundles are not publishable. See frontend progress for exact
build digests. Any later code change invalidates its affected evidence.

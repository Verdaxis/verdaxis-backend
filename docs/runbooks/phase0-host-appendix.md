# Phase 0 host appendix — PM answers applied, 16 October 2026

## Open items

No remaining PM design decision. Final SHA/configuration evidence is an explicit
window checklist, not a design gap. B1/B2 independent review is complete; R1–R3
fixes and any PM-supplied CLI changes must land before execution approval.
One execution prerequisite remains: the bounded scratch business-smoke adapter in
A3 must be built and reviewed. Never point the disposable runner directly at 8100
or weaken its target gate. **M2 ready for review**; review is not authorization
to execute.

## PM answers — final sequencing decisions

1. Maintenance permits exactly GET `/health/ready` and GET `/health/live` to
   localhost:8000; all other requests receive 503. Normal public `/health/ready`
   deploy checks remain unchanged. No HEALTH_URL override.
2. Capture full SHAs, before-file digests, exact restore/removal delta and
   root:root 0644 Caddy readability at rehearsal/window start; stop on mismatch.
3. Task 9 business cases run only on scratch 8100 with scratch identities;
   owner fixture SQL is scratch-target-only. Bootstrap defines schema/catalog
   preparation. Pending dump events mean STOP and a fresh dump, never a scratch
   drain. Live drain checks both dispatched_at and stream_seq before/after stop.
4. Static backup order: independent verify → target ACL acceptance → local
   backup with backend stopped → scratch restore → every table count checked
   against verification evidence → only then deploy/start.
5. Backend acceptance under maintenance → API reopening → ONE frontend workflow
   with candidate_only=false → frontend smoke → founder refresh note. No earlier
   candidate-only run. The accepted cached-bundle interval starts at API reopen.
   Include the workflow's 10–15 minutes in the 60-minute hard limit;
   after API reopening, never automatically return to the old database.

Everything below is for the PM to run, not an agent. No live operation is
performed by editing or testing this appendix.

## Accepted answers and scope

P0-5 (founder may override) rehearses the data path and new release **on the prod
host**, with scratch cluster `17/p0rehearsal`, port 5434 and socket
`/var/run/postgresql`. There is no second OS/systemd instance. Transient units
use `IPAddressDeny=any`, `IPAddressAllow=localhost`, `PrivateTmp=yes` and
`ProtectHome=read-only`. This denies external providers, **not live loopback
ports**; exact endpoint/cluster/OID authority and scratch-only credentials are
therefore essential. Do not describe this as network isolation from 5432/8000.
No provider keys enter rehearsal Settings. Caddy never routes to 127.0.0.1:8100.

**Explicitly not rehearsed:** fixed production path unit/drop-in swaps and actual
old→new→old restoration under systemd. P0-5 accepts that pilot risk. The exact
rollback block below gets syntax and dry-run review only, not an execution claim.
The old database and complete old tuple remain intact. No HBA edit; target-only
CONNECT fence; existing live `verdaxis_migrator` and `verdaxis_backup` roles.

The PM logs into prod as `verdaxis-admin`, uses sudo, and logs into staging as
`jons-openclaw@194.233.68.86` with `sudo -n`. Rehearsal changes no staging/live
services. Cutover pauses staging monitor and backup pull. Schedule outside
02:30–04:30 SGT. **30-minute target; 60-minute hard limit.** At 60 minutes without
signed acceptance **while API access is still closed**, use pre-write rollback.
Reserve 15 minutes for frontend validation/promotion: do not reopen after minute
45 without a new PM decision. After reopening, an overrun means re-close access
and preserve/reconcile new writes, not silent rollback.

Founder announces the window, receives the PM's start notice, gives an explicit
OK before reopening and sends "please refresh" at reopening. KYC key creation,
encrypted Drive archive and separate founder-held recovery secret occur **after
reopening**, with founder present; absence disables storage (503), not readiness.
No forced refresh or compatibility release. No live business-command smoke.

All secrets, real data, reports and logs remain host-local. Only the founder's
encrypted key archive leaves through its approved channel. Never print secrets,
source rows or raw SQL errors. Any failure stays closed for manual inspection;
no EXIT trap reopens traffic, clears deploy state or retries copy.

## Common setup and input contract

Commands below run in a PM root shell (`sudo -n -i`), with `set +x`,
`set -euo pipefail`, `umask 077`. No raw URLs/passwords on argv. Expected exit 0
unless explicitly testing denial; otherwise STOP. Inspect destinations first.
Do not overwrite an existing authority/report or reuse a failed populated target.

One tool installation: `/opt/verdaxis-phase0`; Python
`/opt/verdaxis-phase0/venv/bin/python`. One private workspace:
`/root/verdaxis-phase0` (0700), files 0600. Rehearsal uses a private subdirectory
`rehearsal`; cutover uses `cutover`. The authority anchor is always
`/root/verdaxis-phase0/authority.json`, root-owned 0600. Preserve the rehearsal
anchor before explicitly installing the separately approved live anchor.

```bash
set +x
set -euo pipefail
umask 077
test "$(hostname -s)" = vmi3623757
sudo -n true
systemctl --machine=verdaxis-test@ --user list-units 'verdaxis-prod-test-*'
df -B1 --output=avail /
```

Expected no test units and at least 10737418240 bytes available. PM waits for
lanes to finish; do not interrupt tests. Read authority files on staging:
`/home/jons-openclaw/VPS_MAP.md`,
`/home/verdaxis-prod/verdaxis/PRODUCTION_HOST.md`, and
`/home/jons-openclaw/artifacts/verdaxis-migration-20261001/OPERATIONS.md`.
There is no prod-host file at that old authority path.

### Archive and packages (before isolation/window; PM)

PM records approved full `RELEASE_SHA`, pinned `OLD_SHA=c08266bd6969143812dec7c5033338428cd5f159`
and frontend PR #82 head `FRONTEND_SHA` in the journal. On the PM workstation,
`git archive --format=tar "$RELEASE_SHA" > "release-$RELEASE_SHA.tar"`; transfer
by the PM's scp route to the private host workspace and compare SHA-256 locally
and on host. This is source packaging, not a local build/test. Inspect archive
paths before extraction; refuse absolute paths, `..`, links or a digest mismatch.

```bash
test ! -e /opt/verdaxis-phase0
install -d -o root -g root -m 0755 /opt/verdaxis-phase0
install -d -o root -g root -m 0700 /root/verdaxis-phase0/rehearsal /root/verdaxis-phase0/cutover
sha256sum "/root/verdaxis-phase0/release-$RELEASE_SHA.tar"
tar -tf "/root/verdaxis-phase0/release-$RELEASE_SHA.tar"
tar --no-same-owner -xf "/root/verdaxis-phase0/release-$RELEASE_SHA.tar" -C /opt/verdaxis-phase0
chmod -R go-w /opt/verdaxis-phase0
cd /opt/verdaxis-phase0
/usr/bin/python3.12 -m pip download -r requirements.txt -c constraints.txt -d /root/verdaxis-phase0/wheels
/usr/bin/python3.12 -m venv /opt/verdaxis-phase0/venv
venv/bin/pip install --no-index --find-links /root/verdaxis-phase0/wheels -r requirements.txt -c constraints.txt
venv/bin/pip check
```

Expected Python 3.12.3 and complete constrained packages. No mirror is required:
rehearsal uses the archive; live deploy retains its normal GitHub origin. Keep
wheel hashes and pip-check evidence. No candidate gets live credentials here.

### Tool files and commands (FINAL host interface)

Set `RUN=/root/verdaxis-phase0/rehearsal` for A; use
`RUN=/root/verdaxis-phase0/cutover` for B. These are different evidence sets.

| Tool | Manifest | Source secret | Target secret | New report |
|---|---|---|---|---|
| bootstrap | bootstrap.json | bootstrap-source.env | bootstrap-target.env | bootstrap-receipt.json |
| copy | copy.json | copy-source.env | copy-target.env | copy-receipt.json |
| verify | verify.json | verify-source.env | verify-target.env | verified.json |

Every tool secret contains exactly one `DATABASE_URL=postgresql+asyncpg://...`
line, exact role/endpoint/name, percent-encoded password, no query parameters.
Do not use the historical synthetic `MIGRATOR_DATABASE_URL` format. Separate
source/target files for each tool; passwords differ. Settings are a separate
root-private EnvironmentFile (`rehearsal-settings.env` or `cutover-settings.env`)
with `ENVIRONMENT=production`, exact SHA, both app/migrator URLs, consistent
component settings, UTC, KYC gate false. Rehearsal contains **no provider keys**.

PM writes authority by hand from independent observations, not a target
recapture. Exact fields are in `docs/phase0/progress-hosttools.md:46–71`:
COMMON_FIELDS plus each tool's `tool`, `report`, `bootstrap_receipt_sha256`.
The bootstrap digest is null; later manifests hash its actual successful receipt.
All report paths are new. Copy receipt loss means verify, never repeat copy.

The following reusable transient-unit shell function applies the P0-5 boundary.
Run it in the root shell; do not enable persistent units. `RUN` and `RELEASE_SHA`
must already be approved. `--wait --pipe` returns only after completion.

```bash
p0run() {
  systemd-run --wait --pipe --collect \
    -p IPAddressDeny=any -p IPAddressAllow=localhost \
    -p PrivateTmp=yes -p ProtectHome=read-only \
    -p WorkingDirectory=/opt/verdaxis-phase0 \
    -p "EnvironmentFile=$RUN/settings.env" \
    "$@"
}
```

Copy the approved per-run Settings to `$RUN/settings.env` (0600). Under the
same function, validate no provider keys and outbound denial before real data:

```bash
p0run /usr/bin/python3.12 -c 'import os; assert not any(os.environ.get(k) for k in ("RESEND_API_KEY","GEMINI_API_KEY","WEBZ_API_TOKEN","TELEGRAM_BOT_TOKEN","TELEGRAM_CHAT_ID"))'
if p0run timeout 3 bash -c '</dev/tcp/1.1.1.1/443'; then exit 1; fi
p0run getent hosts api.resend.com || true
```

Expected connection failure; DNS may resolve. Record the transient unit's loaded
IPAddress properties and lack of BPF warnings. Missing filtering is a STOP.
All data-bearing rehearsal Python, psql, restore, smoke and backend commands use
this boundary. Administrative scratch creation/start commands below do not carry
customer rows. Live tools may use the same wrapper but live Settings can contain
provider keys; never use live Settings for rehearsal.

```bash
p0run /opt/verdaxis-phase0/venv/bin/python -m scripts.phase0.bootstrap \
  --manifest "$RUN/bootstrap.json" \
  --source-environment-file "$RUN/bootstrap-source.env" \
  --target-environment-file "$RUN/bootstrap-target.env" --apply
p0run /opt/verdaxis-phase0/venv/bin/python -m scripts.phase0.copy \
  --manifest "$RUN/copy.json" \
  --source-environment-file "$RUN/copy-source.env" \
  --target-environment-file "$RUN/copy-target.env" \
  --source-sha "$OLD_SHA" --target-sha "$RELEASE_SHA" --apply
p0run /opt/verdaxis-phase0/venv/bin/python -m scripts.phase0.verify \
  --manifest "$RUN/verify.json" \
  --source-environment-file "$RUN/verify-source.env" \
  --target-environment-file "$RUN/verify-target.env" \
  --report "$RUN/verified.json"
```

Expected all three exit 0, private reports, exact source preservation. Run each
separately; prepare copy/verify manifests only after bootstrap report exists.
No stdout containing data; private errors remain on host. Record elapsed times.

### Independent observations and source fingerprint

For `PORT=5434` (A) or `PORT=5432` (B), query as postgres without passwords:

```bash
p0run sudo -u postgres psql -X -h /var/run/postgresql -p "$PORT" -d postgres -v ON_ERROR_STOP=1 -Atc "SELECT system_identifier FROM pg_control_system(); SELECT oid,datname,datacl FROM pg_database WHERE datname IN ('verdaxis','verdaxis_prod'); SHOW port;"
p0run sudo -u postgres psql -X -h /var/run/postgresql -p "$PORT" -d verdaxis -v ON_ERROR_STOP=1 -Atc "SELECT version_num FROM alembic_version; SHOW timezone; SELECT has_function_privilege('verdaxis_backup','pg_control_system()','EXECUTE'),has_function_privilege('verdaxis_migrator','pg_control_system()','EXECUTE');"
```

Expected old revision `kycg_20261014_prod`, UTC, both privileges true. Q5 verified
live grants already: **no grant changes on old `verdaxis`**. Scratch privileges
may be prepared independently. Record exact server-host text (including CIDR)
and system ID through the connected endpoint in the authority, as the tools do.

This read-only command writes only a new private observation file. Run after
source drain; it is evidence for PM review, not self-approval:

```bash
p0run /opt/verdaxis-phase0/venv/bin/python - "$RUN" <<'PY'
import asyncio, json, os, sys
from pathlib import Path
from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine
from scripts.phase0.host_authority import tree_digest
from scripts.phase0.manifest import load_manifest, digest, inspect_catalog, require_complete_inventory
from scripts.phase0.verify import fingerprint_database, publish_report
root = Path(sys.argv[1])
async def observe():
    line = (root / 'bootstrap-source.env').read_text().strip()
    assert line.startswith('DATABASE_URL=') and '\n' not in line
    engine = create_async_engine(line.split('=', 1)[1], hide_parameters=True)
    try:
        async with engine.connect() as connection:
            async with connection.begin():
                await connection.execute(text('SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY'))
                assert (await connection.execute(text('SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid()'))).scalar_one() == 0
                catalog = load_manifest()['source_catalog']
                require_complete_inventory(await connection.run_sync(inspect_catalog), catalog)
                fingerprint = await fingerprint_database(connection, catalog)
                identity = dict((await connection.execute(text('SELECT current_database() AS database,current_user AS role,d.oid,inet_server_addr()::text AS server_host,inet_server_port() AS server_port,(SELECT system_identifier::text FROM pg_control_system()) AS cluster_system_identifier FROM pg_database d WHERE datname=current_database()'))).mappings().one())
        publish_report(root / 'source-observation.json', dict(source=identity,
            source_catalog_sha256=digest(catalog), frozen_source=fingerprint,
            tool_tree_sha256=tree_digest(Path('/opt/verdaxis-phase0'))))
    finally:
        await engine.dispose()
asyncio.run(observe())
PY
```

Expected new root-private report and complete inventory equality. PM compares
independent cluster/OID observations and installs approved `authority.json` only
after review. A missing source relation or changed fingerprint stops execution.

## A. Task 10 — P0-5 data-path rehearsal

### A1. Scratch cluster and private credentials (PM)

```bash
pg_lsclusters
test ! -d /etc/postgresql/17/p0rehearsal
pg_createcluster 17 p0rehearsal --port 5434 --start-conf manual
pg_ctlcluster 17 p0rehearsal start
pg_lsclusters
RUN=/root/verdaxis-phase0/rehearsal
PORT=5434
```

Before the scratch credentials exist, create `$RUN/settings.env` with only
`ENVIRONMENT=production` and the approved `RELEASE_SHA`; the administrative
psql commands do not import Settings. After password setup, replace it with the
complete reviewed scratch Settings before any Python tool/application command.

Expected new `17 p0rehearsal 5434`, socket `/var/run/postgresql`, system ID different
from `7691585279836774230`. Stop on existing cluster/port/name collision; never
operate on `17/main` or `17/test`. No staging timer changes in A.

PM generates scratch-only random passwords privately and creates only scratch
role objects. Use a root-owned scratch SQL file, never password argv. The SQL
must create `verdaxis_app`, `verdaxis_backup`, `verdaxis_migrator` as LOGIN,
NOINHERIT, NOSUPERUSER, NOCREATEDB, NOCREATEROLE, NOREPLICATION, NOBYPASSRLS;
no memberships. Use PostgreSQL `\password` interactively inside the following
bounded session (hidden input), with three distinct random password-manager
values. Record them only in private Settings/tool/passfiles:

```bash
p0run sudo -u postgres psql -X -h /var/run/postgresql -p 5434 -d postgres -v ON_ERROR_STOP=1 <<'SQL'
CREATE ROLE verdaxis_app LOGIN NOINHERIT NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
CREATE ROLE verdaxis_backup LOGIN NOINHERIT NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
CREATE ROLE verdaxis_migrator LOGIN NOINHERIT NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
CREATE DATABASE verdaxis OWNER verdaxis_migrator;
SQL
```

`--pipe` is noninteractive: for hidden `\password` use the same properties with
`systemd-run --pty --wait --collect`, run the same psql endpoint, enter
`\password verdaxis_app`, `\password verdaxis_backup`, `\password verdaxis_migrator`,
then `\q`. Do not paste passwords into a SQL file/terminal transcript.
Populate root-private tool secrets and Settings from those values. Verify the
Settings parser before restore; no source password is copied from live.

### A2. Restore newest dump and prepare target (PM)

Select newest local custom dump without deleting or renaming anything. Source
has 65 MB on disk; dumps about 4 MB are a sanity check, not proof:

```bash
find /var/backups/verdaxis -maxdepth 1 -type f -name 'verdaxis-*.dump' -printf '%T@ %p\n' | sort -nr
```

PM records the exact newest `DUMP` path and digest. Then:

```bash
p0run /usr/bin/pg_restore --list "$DUMP"
p0run sudo -u postgres /usr/bin/pg_restore --exit-on-error -h /var/run/postgresql -p 5434 -d verdaxis < "$DUMP"
```

Expected old ownership/grants restored, no errors. Do not use `--no-owner` or
`--no-acl`. Database-level owner/CONNECT settings are not necessarily in a dump
without CREATE DATABASE. Set the independently recorded old database ACL on
scratch **only**:

```bash
p0run sudo -u postgres psql -X -h /var/run/postgresql -p 5434 -d verdaxis -v ON_ERROR_STOP=1 <<'SQL'
REVOKE CONNECT ON DATABASE verdaxis FROM PUBLIC;
GRANT CONNECT ON DATABASE verdaxis TO verdaxis_app,verdaxis_backup;
GRANT EXECUTE ON FUNCTION pg_control_system() TO verdaxis_backup,verdaxis_migrator,verdaxis_app;
SQL
```

This reproduces checked Q5 authority on new scratch objects; do not run it on
5432. If restored relation grants differ, stop rather than invent privileges.
Inspect old revision, catalog, source backup SELECT and function EXECUTE in
scratch. Set scratch-only role/database settings with the SQL below, replacing
`verdaxis_prod` with `verdaxis` only in scratch. Source live settings never change.

Create target and provision extension as postgres **on selected PORT**:

```bash
p0run sudo -u postgres createdb -h /var/run/postgresql -p "$PORT" --owner=verdaxis_migrator verdaxis_prod
p0run sudo -u postgres psql -X -h /var/run/postgresql -p "$PORT" -d verdaxis_prod -v ON_ERROR_STOP=1 <<'SQL'
CREATE EXTENSION postgis;
REVOKE CONNECT ON DATABASE verdaxis_prod FROM PUBLIC, verdaxis_app, verdaxis_backup;
GRANT CONNECT ON DATABASE verdaxis_prod TO verdaxis_migrator;
ALTER DATABASE verdaxis_prod SET timezone TO 'UTC';
ALTER ROLE verdaxis_app IN DATABASE verdaxis_prod SET statement_timeout TO '30s';
ALTER ROLE verdaxis_app IN DATABASE verdaxis_prod SET lock_timeout TO '3s';
ALTER ROLE verdaxis_app IN DATABASE verdaxis_prod SET idle_in_transaction_session_timeout TO '60s';
ALTER ROLE verdaxis_migrator IN DATABASE verdaxis_prod SET statement_timeout TO '300s';
ALTER ROLE verdaxis_migrator IN DATABASE verdaxis_prod SET lock_timeout TO '30s';
ALTER ROLE verdaxis_migrator IN DATABASE verdaxis_prod SET idle_in_transaction_session_timeout TO '300s';
ALTER ROLE verdaxis_backup IN DATABASE verdaxis_prod SET statement_timeout TO '120s';
ALTER ROLE verdaxis_backup IN DATABASE verdaxis_prod SET lock_timeout TO '5s';
ALTER ROLE verdaxis_backup IN DATABASE verdaxis_prod SET idle_in_transaction_session_timeout TO '60s';
SELECT extname,extversion FROM pg_extension;
SELECT oid,datacl::text FROM pg_database WHERE datname=current_database();
SQL
```

Expected installed PostGIS 3.6.4, plpgsql, migrator owner, only migrator CONNECT,
no user tables. Tool policy may reject a changed extension/catalog; no bypass.
No HBA edits. CONNECT does not evict sessions or restrict superusers.

Restored source must have zero undispatched and zero unsequenced outbox rows:

```bash
p0run sudo -u postgres psql -X -h /var/run/postgresql -p 5434 -d verdaxis -Atc "SELECT count(*) FILTER (WHERE dispatched_at IS NULL),count(*) FILTER (WHERE stream_seq IS NULL) FROM market_event_outbox;"
```

Expected `0|0`. Otherwise STOP and take a fresh production dump; no scratch drain
or ad hoc UPDATE. The PM observed 1823 rows and both zero at 03:20 UTC; that is
history, not permission to skip the current check. Capture frozen observations/
authority, run the three tool commands above and retain reports/timings.
Schema/catalog preparation is A2's PostGIS-only target followed by
`scripts.phase0.bootstrap.bootstrap_connection`: its exact committed policy
checks the empty checked connection, runs Alembic, verifies schema and captures
the baseline receipt. Never manually recreate application tables or stamp head.

### A3. New backend, read smoke and business evidence (PM)

After tool verification, use the reviewed target ACL convergence with target-only
runtime EnvironmentFile, then grant runtime/backup CONNECT as the reviewed ACL
policy requires. Do not apply new ACLs to old source:

```bash
p0run /opt/verdaxis-phase0/venv/bin/python -m scripts.converge_runtime_acls \
  --environment production --environment-file "$RUN/settings.env" --bundle-root /opt/verdaxis-phase0
```

The object ACL helper does not grant database CONNECT. After independent verify,
open only the new target to the two exact runtime/backup roles:

```bash
p0run sudo -u postgres psql -X -h /var/run/postgresql -p "$PORT" -d verdaxis_prod -v ON_ERROR_STOP=1 <<'SQL'
GRANT CONNECT ON DATABASE verdaxis_prod TO verdaxis_app,verdaxis_backup;
SQL
```

Verify schema USAGE/default privileges against the approved policy before runtime
preflight. Do not grant app reference-table UPDATE or metadata SELECT. The same
explicit target CONNECT transition is required after B4 verification, before B7
preflight; the tool chain intentionally refuses after this transition.

Run backend as a transient unit, not installed prod units:

```bash
systemd-run --unit=verdaxis-phase0-rehearsal --collect \
  -p User=verdaxis-prod -p WorkingDirectory=/opt/verdaxis-phase0 \
  -p EnvironmentFile=/root/verdaxis-phase0/rehearsal/settings.env \
  -p IPAddressDeny=any -p IPAddressAllow=localhost -p PrivateTmp=yes -p ProtectHome=read-only \
  /opt/verdaxis-phase0/venv/bin/uvicorn app.main:app --host 127.0.0.1 --port 8100 --workers 4 --no-access-log
p0run curl --fail --silent http://127.0.0.1:8100/health/ready
systemctl show verdaxis-phase0-rehearsal.service -p ActiveState -p IPAddressDeny -p IPAddressAllow
```

Expected exact SHA readiness on 127.0.0.1:8100, four workers, KYC gate false,
missing vault key/503 storage. No Caddy route, no provider credentials. Log only
bounded facts. Use the Task 9 HTTP business cases from `tests/integration`
with the fixture identity construction in `tests/phase0/http_seed.py`: unique
scratch organizations/users, approved organization-bound KYC, hashed scratch
passwords and fixture provenance. Owner fixture SQL is allowed only in scratch
`verdaxis_prod` on 5434, after copy verification; never in old scratch source or
live. Do not run the entire seed file on copied data: its catalog seed, fixed
Singapore port and fixed user IDs can collide.

The existing `tests/phase0/http_runner.py` starts its own disposable container
and `tests.disposable_server`; its integration gate requires ENVIRONMENT=test
and an ephemeral port, not production-mode 8100. Thus select/review a bounded
adapter of the business cases before A3; do not disable that gate or pretend the
runner accepts `--target-url`. Record its exact SHA, scratch-only identities and
case list with the rehearsal inputs. This is the remaining executable test
artifact, not an unresolved PM policy. Run it through `p0run` with
`http://127.0.0.1:8100`, assert app-role business success/denial, UTC reconnects
and missing-key storage 503. No customer account or provider call is permitted.
Do not claim readiness proves KYC submission. Finish and retain the cluster:

```bash
systemctl stop verdaxis-phase0-rehearsal.service
systemctl show verdaxis-phase0-rehearsal.service -p ActiveState -p Job
```

Expected inactive/no job. Record restore/bootstrap/copy/verify timings and smoke
results. Old→new→old fixed-path/systemd restoration is not rehearsed under P0-5.
Review B8 with `bash -n` and deploy/installer dry-run only; do not execute live
rollback as a drill. Keep scratch until post-cutover restored-backup acceptance.

## B. Task 11 — production cutover

### B1. Window approval and capture (PM; founder announces)

Outside 02:30–04:30 SGT, start monotonic 60-minute deadline and journal. Pause test
lanes, check at least 10 GB free, founder start notice, clean old SHA and DNS:

```bash
WINDOW_START=$SECONDS
getent ahostsv4 api.verdaxis.exchange
sudo -u verdaxis-prod git -C /home/verdaxis-prod/verdaxis/prod/be status --porcelain
sudo -u verdaxis-prod git -C /home/verdaxis-prod/verdaxis/prod/be rev-parse HEAD
sudo -u postgres psql -X -d postgres -Atc "SELECT system_identifier FROM pg_control_system(); SELECT oid,datname FROM pg_database WHERE datname IN ('verdaxis','verdaxis_prod');"
```

Expected `169.58.37.164`, old pinned SHA, clean tree, old OID 21404/system ID
`7691585279836774230`, target absent. Stop on drift; no drop/reuse.
Capture files privately without printing contents:

```bash
test ! -e /root/verdaxis-phase0/before
install -d -m 0700 /root/verdaxis-phase0/before /root/verdaxis-phase0/backups-before
cp -a /var/backups/verdaxis/. /root/verdaxis-phase0/backups-before/
for path in /etc/caddy/Caddyfile /etc/verdaxis-backup-export.conf /home/verdaxis-backup-export/.pgpass /usr/local/sbin/verdaxis-local-db-backup /etc/verdaxis-monitor/demo/production.env /home/verdaxis-prod/verdaxis/prod/be/.env /home/verdaxis-prod/verdaxis/prod/be/.env.analytics /home/verdaxis-prod/verdaxis/prod/be/.runtime-release.env; do
  install -D -m 0600 "$path" "/root/verdaxis-phase0/before$path"
done
cp -a /etc/systemd/system/verdaxis-* /root/verdaxis-phase0/before/
find /root/verdaxis-phase0/before -type f -exec sha256sum {} + > /root/verdaxis-phase0/before.sha256
```

Root-private parent protects captured units. Preserve existing monitor/runtime
file if present, else record absence. Keep old dumps outside recursive retention.
PM checklist before any mutation: journal approved OLD_SHA/RELEASE_SHA/FRONTEND_SHA,
archive digest, every before-file hash, exact candidate-only file removal list and
old restore list. Compare installed drop-ins to that list; copying cannot delete
added files. Install Caddy bytes root:root 0644 and prove readability:

```bash
stat -c '%U:%G %a' /etc/caddy/Caddyfile
sudo -u caddy test -r /etc/caddy/Caddyfile
sha256sum -c /root/verdaxis-phase0/before.sha256
```

Expected root:root 644, successful read and all hashes OK. Missing SHA, unknown
delta, changed file or unreadable Caddyfile stops the window. Repeat this input
check at rehearsal start with its separate evidence set.

### B2. Pause timers and close all public traffic (PM)

On prod capture all verdaxis timer states, then stop the seven listed timers:

```bash
systemctl show 'verdaxis-*.timer' -p Id -p ActiveState -p UnitFileState > /root/verdaxis-phase0/timers-before.txt
systemctl stop verdaxis-auth-maintenance.timer verdaxis-demo-activity.timer verdaxis-onboarding-attention.timer verdaxis-order-expiry-reminders.timer verdaxis-news-refresh.timer verdaxis-product-analytics-prune.timer verdaxis-local-db-backup.timer
```

On staging, PM as `jons-openclaw`, sudo root shell, create a private same-named
state file on that host and run:

```bash
install -d -m 0700 /root/verdaxis-phase0
systemctl show 'verdaxis-*.timer' -p Id -p ActiveState -p UnitFileState > /root/verdaxis-phase0/timers-before.txt
systemctl stop verdaxis-monitor.timer verdaxis-backup.timer
systemctl show verdaxis-monitor.service verdaxis-backup.service -p ActiveState -p SubState -p Job
systemctl list-jobs --no-pager
```

Expected no active/pending pull or monitor; wait naturally, no kill. Recovery
remains masked. No staging changes during A. Onboarding/local backup explicitly
stop despite ignoring deploy state. Do not repair failed analytics relay.

PM prepares `/root/verdaxis-phase0/Caddyfile.maintenance`: original file, replacing
only the whole site's route block with the following exact PM-approved content:

```caddyfile
@phase0_health {
    method GET
    path /health/ready /health/live
}
route {
    handle @phase0_health {
        reverse_proxy localhost:8000
    }
    handle {
        header Retry-After "600"
        header Content-Type "application/json"
        respond `{"detail":"Verdaxis is under maintenance. Please retry in a few minutes."}` 503
    }
}
```

The named matcher belongs inside the existing site block, beside this replacement
route. Retain original TLS/global/trusted-proxy bytes. Only exact GET health paths
pass; every other method/path, including `/api/send`, SQLAdmin, authenticated GET,
HEAD health, `/health` alias and SSE receives 503. No stale staging Caddy edits.

```bash
caddy validate --config /root/verdaxis-phase0/Caddyfile.maintenance --adapter caddyfile
install -m 0644 /root/verdaxis-phase0/Caddyfile.maintenance /etc/caddy/Caddyfile
systemctl reload caddy
for path in /health /api/auth/me /api/send /admin /api/orderbook /api/stream; do
  test "$(curl --silent --output /dev/null --write-out '%{http_code}' "https://api.verdaxis.exchange$path")" = 503
done
```

Expected all listed probes 503; GET `/health/ready` and `/health/live` still
reach the running old backend. Check POST/HEAD health refuse too. Reload need
not terminate established SSE; stop backend below
closes it. Wait up to 120 seconds for in-flight requests/provider oneshots and
check all seven service peers inactive/no job:

```bash
for unit in verdaxis-auth-maintenance.service verdaxis-demo-activity.service verdaxis-onboarding-attention.service verdaxis-order-expiry-reminders.service verdaxis-news-refresh.service verdaxis-product-analytics-prune.service verdaxis-local-db-backup.service; do
  systemctl show "$unit" -p ActiveState -p SubState -p Job
done
systemctl list-jobs --no-pager
```

Expected inactive/dead (or recorded failed with no running process), empty/0 Job.
Any running/pending writer after 120 seconds stops the cutover. Keep old backend solely to drain
its existing sequencer. Check pending outbox as postgres without changing rows:

```bash
DRAIN_START=$SECONDS
while test "$(sudo -u postgres psql -X -d verdaxis -Atc 'SELECT count(*) FROM market_event_outbox WHERE dispatched_at IS NULL OR stream_seq IS NULL')" != 0; do
  test "$((SECONDS-DRAIN_START))" -lt 120
  sleep 2
done
systemctl stop verdaxis-backend.service
sudo -u postgres psql -X -d verdaxis -Atc "SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid(); SELECT count(*) FILTER (WHERE dispatched_at IS NULL),count(*) FILTER (WHERE stream_seq IS NULL) FROM market_event_outbox;"
```

Expected zero other sessions and `0|0` outbox counts after ALL producers drain;
late commit means STOP/drain again,
not a snapshot. No session termination/source grants/settings. Preserve frozen
source; take a new custom backup with the existing read-only role/passfile:

```bash
test ! -e /root/verdaxis-phase0/cutover/final.dump
sudo -u verdaxis-backup-export env PGPASSFILE=/home/verdaxis-backup-export/.pgpass pg_dump -h 127.0.0.1 -p 5432 -U verdaxis_backup -d verdaxis --format=custom > /root/verdaxis-phase0/cutover/final.dump
sha256sum /root/verdaxis-phase0/cutover/final.dump
```

Root opens the private output; pg_dump receives no password argv. A nonzero exit
leaves a partial dump: preserve it, do not accept or overwrite it. Restore into a
new scratch verification database, not the rehearsal source already used.

### B3. External fence and final source authority (PM)

Install the exact guard/drop-in from the main runbook, outside checkout, root
owned. Inspect parent ownership first. Publish fence durably:

```bash
install -d -o root -g root -m 0700 /var/lib/verdaxis/phase0
install -o root -g root -m 0755 /opt/verdaxis-phase0/scripts/phase0/start_fence.py /opt/verdaxis-phase0/start_fence.py
python3 -I - <<'PY'
import os
root='/var/lib/verdaxis/phase0'
fd=os.open(root+'/fence',os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
with os.fdopen(fd,'wb') as f:
    f.write(b'phase0\n'); f.flush(); os.fsync(f.fileno())
fd=os.open(root,os.O_RDONLY|os.O_DIRECTORY); os.fsync(fd); os.close(fd)
PY
install -d -m 0755 /etc/systemd/system/verdaxis-backend.service.d
test ! -e /etc/systemd/system/verdaxis-backend.service.d/90-phase0-fence.conf
printf '%s\n' '[Service]' 'ExecStartPre=+/usr/bin/python3 -I /opt/verdaxis-phase0/start_fence.py --environment production --expected-release-sha ${RELEASE_SHA} --database verdaxis_prod' > /etc/systemd/system/verdaxis-backend.service.d/90-phase0-fence.conf
chmod 0644 /etc/systemd/system/verdaxis-backend.service.d/90-phase0-fence.conf
systemctl daemon-reload
systemctl show verdaxis-backend.service -p LoadState -p NeedDaemonReload -p DropInPaths -p ExecStartPre
```

Expected exact external guard plus existing two guards, loaded/no pending reload.
No source replacement until this check. Precondition: main runbook drop-in bytes
are installed, not just guard script. Missing/mismatched drop-in is a STOP.

```bash
/opt/verdaxis-phase0/venv/bin/python -m scripts.phase0.assert_host \
  --manifest /root/verdaxis-phase0/cutover/old.json --check outbox-empty
```

Expected zero jobs/sessions/pending events. Assertion manifests use raw backup
password files and are **not** production tool manifests. PM signs frozen source
observation, new target identity and authority; no blind baseline recapture.

### B4. New live target and tool chain (PM)

Set `PORT=5432`, `RUN=/root/verdaxis-phase0/cutover`. Recheck target absence, then
execute A2's exact target creation/PostGIS/CONNECT/settings block against this
port. It changes only `verdaxis_prod`; existing live role attributes, old DB,
HBA and credentials remain unchanged. Recheck UTC and target ownership/datacl.
Install approved live authority, separate tool secrets and Settings, then run
bootstrap→copy→verify commands in Common setup. Each new report must match the
approved authority and frozen source. Stop on any refusal; never replay copy.

### B4a. Static backup and restored-count acceptance (PM)

**Do this before B5–B7, with new backend stopped and every writer paused.**
Order: independent verify report → target schema/default/object ACL acceptance
(A3, including app/backup CONNECT) → B9 backup configuration switch → manual
backup → scratch restore → count comparison → only then runtime deploy.
B9 is a callable configuration block, not a later post-start step. Do not run
demo or business smoke against the live target. Any other target session stops
the boundary. The object ACL helper is
`scripts.converge_runtime_acls` with the reviewed target Settings/bundle; database
and schema/default privileges must pass `deploy/postgres/validate_roles.sql`
against this target. Never run global role bootstrap against live roles.

Execute B9 through restore into scratch `verdaxis_restorecheck`. Then compare
**every target table** to the independent verification evidence, including extra
reference rows, derived grace notices, empty new tables and Alembic's one row.
Keep the table-name/count-only CSV private:

```bash
p0run sudo -u postgres psql -X -h /var/run/postgresql -p 5434 -d verdaxis_restorecheck -v ON_ERROR_STOP=1 -qAt <<'SQL' > /root/verdaxis-phase0/cutover/restored-counts.csv
SELECT format('SELECT %L || '','' || count(*) FROM %I.%I;',n.nspname||'.'||c.relname,n.nspname,c.relname)
FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
WHERE n.nspname='public' AND c.relkind IN ('r','p')
AND NOT EXISTS (SELECT 1 FROM pg_depend d WHERE d.classid='pg_class'::regclass AND d.objid=c.oid AND d.deptype='e')
ORDER BY c.relname
\gexec
SQL
p0run /opt/verdaxis-phase0/venv/bin/python - <<'PY'
import csv,json
from pathlib import Path
from scripts.phase0.manifest import load_manifest
from scripts.phase0.verify import publish_report
root=Path('/root/verdaxis-phase0/cutover')
report=json.loads((root/'verified.json').read_text())['verification']
expected={name:int(value['count']) for name,value in report['tables'].items()}
for name,value in report['new_reference_rows'].items():
    expected[name]+=int(value['count'])
expected.update(refresh_sessions=0,supplier_offers=0,kyc_documents=0,
                kyc_grace_notices=int(report['notice_count']),alembic_version=1)
observed={}
for name,count in csv.reader((root/'restored-counts.csv').open()):
    assert name.startswith('public.')
    name=name.removeprefix('public.')
    assert name not in observed
    observed[name]=int(count)
# Any unclassified table fails closed, including later schema additions.
assert set(observed)==set(expected), 'Restore table inventory differs'
assert observed==expected, 'Restored counts differ from independent verify'
publish_report(root/'restored-counts-accepted.json',dict(counts=observed,verified=True))
PY
```

Expected exit 0 and new private accepted report. An unclassified relation or
count mismatch is STOP, never an ignored table. No concurrent writes or new
runtime before this finishes. The verified report is the fixed comparison, not
a fresh query of changing live data. Preserve backup/report evidence; scratch
cleanup remains after this successful check and PM acceptance.

### B5. Prepare new runtime, preserve old complete tuple (PM)

Prepare a clean canonical staging checkout before the window with its normal
origin and approved SHA. PM supplies it at the new work-product path
`/home/verdaxis-prod/verdaxis/prod/be.phase0-new`; this is not claimed preexisting.
Verify clean SHA and no symlink. Preserve old entire directory, not just Git:

```bash
root=/home/verdaxis-prod/verdaxis/prod/be
test ! -e "${root}.phase0-old"
test ! -L "$root"
test ! -L "${root}.phase0-new"
test "$(sudo -u verdaxis-prod git -C "${root}.phase0-new" rev-parse HEAD)" = "$RELEASE_SHA"
test -z "$(sudo -u verdaxis-prod git -C "${root}.phase0-new" status --porcelain)"
mv "$root" "${root}.phase0-old"
mv "${root}.phase0-new" "$root"
```

```bash
install -o verdaxis-prod -g verdaxis-prod -m 0600 "${root}.phase0-old/.env" "$root/.env"
install -o verdaxis-prod -g verdaxis-prod -m 0600 "${root}.phase0-old/.env.analytics" "$root/.env.analytics"
sudo -u verdaxis-prod /usr/bin/python3.12 -m venv "$root/venv"
```

The service user cannot traverse root's private wheelhouse. Copy its reviewed
non-secret package files to a service-readable new cache under
`/opt/verdaxis-phase0/wheels` before installation; keep root ownership:

```bash
install -d -m 0755 /opt/verdaxis-phase0/wheels
cp /root/verdaxis-phase0/wheels/* /opt/verdaxis-phase0/wheels/
chmod 0644 /opt/verdaxis-phase0/wheels/*
sudo -u verdaxis-prod "$root/venv/bin/pip" install --no-index --find-links /opt/verdaxis-phase0/wheels -r "$root/requirements.txt" -c "$root/constraints.txt"
sudo -u verdaxis-prod "$root/venv/bin/pip" check
```

Then change only the three
name sources `DATABASE_NAME`, `DATABASE_URL`, `MIGRATOR_DATABASE_URL` to new DB
using the exact script in B9 (on runtime .env). Preserve other secrets, gate off,
user split and URLs. Build final-path venv from cached constrained packages;
never reuse relocated shebangs. Publish `.runtime-release.env` with exact
ENVIRONMENT/SHA/union revision and credential-free `.runtime-monitor.env` before
new code gets credentials:

```bash
printf 'ENVIRONMENT=production\nRELEASE_SHA=%s\nMIGRATION_REVISION=p0_20261016_union\n' "$RELEASE_SHA" > "$root/.runtime-release.env"
printf 'ENVIRONMENT=production\nRELEASE_SHA=%s\n' "$RELEASE_SHA" > "$root/.runtime-monitor.env"
chown verdaxis-prod:verdaxis-prod "$root/.runtime-release.env" "$root/.runtime-monitor.env"
chmod 0600 "$root/.runtime-release.env"
chmod 0644 "$root/.runtime-monitor.env"
```

The external fence remains closed during both writes. Fsync files/directory
before approving final tuple. Do not issue a permit yet.

Use `new-preinstall.json` with current installed-unit hashes, then immutable
installer and `new-final.json` with actual final unit/drop-in/venv/env hashes:

```bash
/opt/verdaxis-phase0/venv/bin/python -m scripts.phase0.assert_host \
  --manifest /root/verdaxis-phase0/cutover/new-preinstall.json --check tuple
sudo -n -u verdaxis-prod -H /home/verdaxis-prod/verdaxis/prod/be/scripts/deploy.sh --dry-run
sudo /home/verdaxis-prod/verdaxis/prod/be/scripts/install_systemd_units.sh \
  --dry-run --environment production --source-ref "$RELEASE_SHA"
sudo /home/verdaxis-prod/verdaxis/prod/be/scripts/install_systemd_units.sh \
  --apply --environment production --source-ref "$RELEASE_SHA"
systemctl daemon-reload
systemctl show verdaxis-backend.service -p LoadState -p NeedDaemonReload -p DropInPaths -p ExecStartPre
/opt/verdaxis-phase0/venv/bin/python -m scripts.phase0.assert_host \
  --manifest /root/verdaxis-phase0/cutover/new-final.json --check tuple
```

Expected approved SHA, matching hashes, four workers, unchanged guards. No key
is required during this window; storage 503 is expected. Stop on stale unit cache.

### B6. Fsync permit (PM root shell)

No stale permit or previous lock owner may remain. Inspect stale files before
removal. Never reuse old lock inode/permit. This script creates a fresh lock;
root holds fd 9 until deploy finishes. It does not hold the normal deploy locks.

```bash
test ! -e /var/lib/verdaxis/phase0/permit.json
test ! -L /var/lib/verdaxis/phase0/permit.json
python3 -I - <<'PY'
import os,tempfile
root='/var/lib/verdaxis/phase0'
fd,path=tempfile.mkstemp(dir=root,prefix='.operator.')
os.fsync(fd); os.close(fd); os.replace(path,root+'/operator.lock')
fd=os.open(root,os.O_RDONLY|os.O_DIRECTORY); os.fsync(fd); os.close(fd)
PY
exec 9<>/var/lib/verdaxis/phase0/operator.lock
flock -n 9
export RELEASE_SHA
python3 -I - <<'PY'
import os,json,re,tempfile
root='/var/lib/verdaxis/phase0'; sha=os.environ['RELEASE_SHA']
assert re.fullmatch('[0-9a-f]{40}',sha)
s=os.stat(root+'/operator.lock')
fd,path=tempfile.mkstemp(dir=root,prefix='.permit.')
with os.fdopen(fd,'w') as f:
    json.dump(dict(environment='production',release_sha=sha,database='verdaxis_prod',lock_device=s.st_dev,lock_inode=s.st_ino),f)
    f.flush(); os.fsync(f.fileno())
os.replace(path,root+'/permit.json')
fd=os.open(root,os.O_RDONLY|os.O_DIRECTORY); os.fsync(fd); os.close(fd)
PY
/opt/verdaxis-phase0/venv/bin/python -m scripts.phase0.start_fence \
  --environment production --expected-release-sha "$RELEASE_SHA" --database verdaxis_prod
```

Expected guard passes only while root lock held. Failure leaves fence intact.

### B7. Normal deploy and live read-only smoke under maintenance (PM)

B4a static backup/count acceptance must already be signed. The approved exact
GET health exceptions let the helper's public gate work; all other traffic is
still closed. Use normal four-variable same-revision deploy, no URL bypass:

```bash
sudo -n -u verdaxis-prod -H env APPROVED_RELEASE_SHA="$RELEASE_SHA" \
  MIGRATION_APPROVED_SOURCE_SHA="$RELEASE_SHA" \
  MIGRATION_EXPECTED_CURRENT_REVISION=p0_20261016_union \
  MIGRATION_TARGET_REVISION=p0_20261016_union \
  /home/verdaxis-prod/verdaxis/prod/be/scripts/deploy.sh
```

Expected helper owns both locks, exact readiness, state absent. Revoke permit
with unlink+directory fsync before releasing fd 9. On failure revoke manually,
leave state/fence/maintenance and inspect. Never open customer traffic in a trap.

```bash
python3 -I - <<'PY'
import os
root='/var/lib/verdaxis/phase0'
os.unlink(root+'/permit.json')
fd=os.open(root,os.O_RDONLY|os.O_DIRECTORY); os.fsync(fd); os.close(fd)
PY
exec 9>&-
curl --fail --silent http://127.0.0.1:8000/health/ready > /root/verdaxis-phase0/cutover/ready.json
/opt/verdaxis-phase0/venv/bin/python -m scripts.validate_health_response \
  --environment production --release-sha "$RELEASE_SHA" < /root/verdaxis-phase0/cutover/ready.json
for path in /health/live /api/prices /api/orderbook /api/orderbook/bids /api/orderbook/regions /api/curves/forward; do
  curl --fail --silent --output /dev/null "http://127.0.0.1:8000$path"
done
```

Expected exact SHA and successful public reads. No login, customer writes,
authenticated GETs or private exports. Account for internal outbox writes before
signed pre-write acceptance. Production verify requires original target CONNECT
fence; it refuses after app CONNECT. For a post-start comparison stop new backend,
restore target-only approved datacl, verify with a new report/manifest, then
reapply target ACLs via normal helper. Never alter source grants.

Do not dispatch a frontend workflow here or before the window. Continue through
signed backend acceptance and B10 API reopening, then run exactly B10a. Until
then all non-health public traffic remains 503.

### B8. Exact pre-write rollback block (PM; syntax/dry-run only in A)

If 60 minutes expires without acceptance, or acceptance fails, stay closed.
First stop new workers, revoke permit, preserve failed evidence. Signed
reconciliation must show no customer writes; otherwise preserve both DBs and
request reconciliation, not rollback. No `git reset` or downgrade.

```bash
set -euo pipefail
umask 077
root=/home/verdaxis-prod/verdaxis/prod/be
systemctl stop verdaxis-backend.service
rm -f /var/lib/verdaxis/phase0/permit.json
test -d "${root}.phase0-old"
test ! -e "${root}.phase0-failed"
mv "$root" "${root}.phase0-failed"
mv "${root}.phase0-old" "$root"
test "$(sudo -u verdaxis-prod git -C "$root" rev-parse HEAD)" = "$OLD_SHA"
# Restore only the exact before/ unit list reviewed at B1, including old drop-ins.
cp -a /root/verdaxis-phase0/before/verdaxis-* /etc/systemd/system/
# The old directory already includes the old .env, analytics file and original venv.
install -m 0644 /root/verdaxis-phase0/before/etc/caddy/Caddyfile /root/verdaxis-phase0/Caddyfile.reopen
```

Do not reopen Caddy yet. Remove candidate-only drop-ins from the reviewed delta
list captured and signed at B1 before restoring; copying cannot remove added files. Restore
backup config/script/demo files if B9 ran; preserve ownership and modes from
before-state. Change only external fence `--database verdaxis_prod` to
`--database verdaxis`, reload and check all guards. Publish a fresh B6 permit
using `OLD_SHA` and literal `verdaxis`; do not recycle the new permit. Then, as
service owner, hold both normal locks throughout restart/readiness:

```bash
sudo -n -u verdaxis-prod -H bash -s -- "$OLD_SHA" <<'SH'
set -euo pipefail
umask 077
root=/home/verdaxis-prod/verdaxis/prod/be
guard="$root/.runtime-deploy"
exec 7>>"$guard/production.lock"
flock -n 7
exec 8>>"$guard/production.restart.lock"
flock -n 8
state=$(mktemp "$guard/.production.state.XXXXXXXX")
printf 'DEPLOYMENT_STATE=restart-authorized\nENVIRONMENT=production\nRELEASE_SHA=%s\n' "$1" > "$state"
chmod 600 "$state"
mv "$state" "$guard/production.state"
sudo systemctl restart verdaxis-backend.service
curl --fail --silent http://127.0.0.1:8000/health/ready | "$root/venv/bin/python" -m scripts.validate_health_response --environment production --release-sha "$1"
rm "$guard/production.state"
SH
```

Expected exact old local readiness; no migration, ACL mutation or writer restart
on old DB. Root revokes/fsyncs permit before releasing root lock. Restore prior
frontend if promoted. Only after PM accepts recovered tuple may Caddy reopen
and recorded timers restart. Full old→new→old execution is not claimed in A.

### B9. Backup configuration subroutine for B4a; demo binding before rearm (PM)

Execute backup edits/manual dump/restore from B4a **before any new backend start**.
Execute the demo name patch before timer rearm. Still pause both backup timers. Old dumps already reside outside retention.
Change config, password entry and local artifact names with exact checks:

```bash
python3 -I - <<'PY'
import os
from pathlib import Path
config=Path('/etc/verdaxis-backup-export.conf')
lines=config.read_text().splitlines()
assert sum(line.startswith('PGDATABASE=') for line in lines)==1
config.write_text('\n'.join("PGDATABASE='verdaxis_prod'" if line.startswith('PGDATABASE=') else line for line in lines)+'\n')
password=Path('/home/verdaxis-backup-export/.pgpass')
lines=password.read_text().splitlines()
old=[line for line in lines if line.startswith('127.0.0.1:5432:verdaxis:verdaxis_backup:')]
assert len(old)==1 and not any(line.startswith('127.0.0.1:5432:verdaxis_prod:') for line in lines)
lines.append(old[0].replace(':verdaxis:',':verdaxis_prod:',1))
password.write_text('\n'.join(lines)+'\n'); password.chmod(0o600)
script=Path('/usr/local/sbin/verdaxis-local-db-backup')
value=script.read_text()
for old,new in [('.verdaxis-${TIMESTAMP}.dump.partial','.verdaxis-prod-${TIMESTAMP}.dump.partial'),('verdaxis-${TIMESTAMP}.dump','verdaxis-prod-${TIMESTAMP}.dump')]:
    assert value.count(old)==1
    value=value.replace(old,new)
script.write_text(value)
for path in (config,password,script):
    fd=os.open(path,os.O_RDONLY); os.fsync(fd); os.close(fd)
PY
```

Expected exactly named edits, existing file ownership/mode retained, no printed
password. On any assert failure restore before copies, inspect bytes; no global
substitution. Both exporters use the shared config. Staging `backup-db.sh` stays
unchanged per Q6; do not claim its historical prefix changed. No new monitor
reader or backup timer installed. Preserve old configs and artifacts.

For both runtime .env (B5) and demo file, use this no-output URL-aware patch,
with `ENV_PATH` set to only the reviewed destination:

```bash
/opt/verdaxis-phase0/venv/bin/python - "$ENV_PATH" <<'PY'
import os,sys
from pathlib import Path
from sqlalchemy.engine import make_url
path=Path(sys.argv[1]); lines=path.read_text().splitlines(); seen=set(); result=[]
for line in lines:
    key,sep,value=line.partition('=')
    if key in {'DATABASE_NAME','DATABASE_URL','MIGRATOR_DATABASE_URL'}:
        assert key not in seen and sep
        seen.add(key)
        quote=value[0] if value[:1] in {"'",'"'} else ''
        if quote:
            assert value.endswith(quote); value=value[1:-1]
        if key=='DATABASE_NAME':
            assert value=='verdaxis'; value='verdaxis_prod'
        else:
            url=make_url(value); assert url.database=='verdaxis'
            value=url.set(database='verdaxis_prod').render_as_string(hide_password=False)
        line=key+'='+quote+value+quote
    result.append(line)
assert seen=={'DATABASE_NAME','DATABASE_URL','MIGRATOR_DATABASE_URL'}
path.write_text('\n'.join(result)+'\n')
fd=os.open(path,os.O_RDONLY); os.fsync(fd); os.close(fd)
PY
```

Only allowed destinations: new runtime `.env` and
`/etc/verdaxis-monitor/demo/production.env`. Onboarding reads new runtime .env.
No reader consumes `.runtime-monitor.env` in Phase 0; validate file identity only.

```bash
systemctl start verdaxis-local-db-backup.service
systemctl show verdaxis-local-db-backup.service -p Result -p ExecMainStatus
find /var/backups/verdaxis -maxdepth 1 -name 'verdaxis-prod-*.dump' -type f -printf '%T@ %p\n' | sort -nr
```

Expected successful new custom dump. Select exact new `RESTORE_DUMP`, record
digest. Restore into scratch only, never live:

```bash
p0run sudo -u postgres createdb -h /var/run/postgresql -p 5434 --owner=verdaxis_migrator verdaxis_restorecheck
p0run sudo -u postgres pg_restore --exit-on-error -h /var/run/postgresql -p 5434 -d verdaxis_restorecheck < "$RESTORE_DUMP"
```

Return to B4a's exact per-table comparison against the verify report, while the
new backend is still stopped. `pg_restore --list` is not proof. A mismatched or
unclassified table stops deployment. Preserve table-name/count-only evidence,
no row values. After full acceptance and approved cleanup:
`pg_dropcluster 17 p0rehearsal --stop` (PM only).

### B10. Sign and reopen the API (PM/founder)

PM journal acceptance plus founder OK required. Confirm no writes on old DB,
new read-only smoke, backup result, runtime tuple and 60-minute limit. KYC
key-ring recovery is **not** a pre-reopen gate under Q7.

```bash
test ! -e /home/verdaxis-prod/verdaxis/prod/be/.runtime-deploy/production.state
test ! -L /home/verdaxis-prod/verdaxis/prod/be/.runtime-deploy/production.state
test ! -e /var/lib/verdaxis/phase0/permit.json
test ! -L /var/lib/verdaxis/phase0/permit.json
rm /var/lib/verdaxis/phase0/fence
rm /etc/systemd/system/verdaxis-backend.service.d/90-phase0-fence.conf
systemctl daemon-reload
systemctl show verdaxis-backend.service -p NeedDaemonReload -p DropInPaths -p ExecStartPre
caddy validate --config /root/verdaxis-phase0/before/etc/caddy/Caddyfile --adapter caddyfile
install -m 0644 /root/verdaxis-phase0/before/etc/caddy/Caddyfile /etc/caddy/Caddyfile
systemctl reload caddy
```

Expected original guards retained, no pending reload, original Caddy routing.
Fsync modified directories and append actual write-open UTC to private journal.
No rollback after this without reconciliation. Old bundles now use the accepted
new backend; this is the PM-accepted cached-bundle interval. Record exact UTC
reopen time before continuing; all frontend workflow time counts toward 60 minutes.

### B10a. ONE frontend workflow, smoke, then refresh note (PM/founder)

From the PM workstation, dispatch exactly one approved promotion workflow. It
validates the candidate against the now-live new backend, then promotes the same
candidate. No candidate-only pre-run and no old-SHA substitution:

```bash
gh workflow run release-vercel.yml --repo Verdaxis/verdaxis-frontend --ref staging \
  -f source_sha="$FRONTEND_SHA" -f backend_release_sha="$RELEASE_SHA" \
  -f candidate_only=false -f notify_release=true
gh run watch "$PROMOTION_RUN_ID" --repo Verdaxis/verdaxis-frontend --exit-status
```

Bind PROMOTION_RUN_ID to this dispatch's SHA/time in GitHub; do not select another
operator's run. Expected successful candidate/canary checks, promotion and bounded
report containing current/prior deployment IDs. PM verifies production entry
HTML, hashed asset and the workflow's frontend smoke; only then founder sends
"please refresh". Existing banner remains, no forced reload. On repeated critical
failure, workflow uses captured prior frontend deployment. Manual frontend
rollback uses founder's Vercel login and exact recorded prior deployment, then
checks its aliases/asset. It does **not** roll back the backend or discard writes.

If promotion/smoke fails or the 60-minute limit is reached after API reopening,
re-close non-health API traffic with the same maintenance file, preserve new
data and reports, and request PM forward-repair/reconciliation. Do not run B8
against a database that may contain accepted customer writes. Never interrupt a
running test or release call; wait for its completion while access stays closed.

### B10b. Restore only recorded-active timers (PM)

After successful frontend smoke/refresh communication, on each host derive restore candidates from its private before-state; review
that list, then start only recorded-active timers. Do not enable inactive/masked
units or restore demo before its new identity is checked:

```bash
python3 -I - <<'PY'
from pathlib import Path
text=Path('/root/verdaxis-phase0/timers-before.txt').read_text()
names=[]
for block in text.strip().split('\n\n'):
    fields=dict(line.split('=',1) for line in block.splitlines())
    if fields.get('ActiveState')=='active':
        name=fields['Id']; assert name.startswith('verdaxis-') and name.endswith('.timer')
        assert fields.get('UnitFileState') not in {'masked','masked-runtime'}
        names.append(name)
Path('/root/verdaxis-phase0/timers-restore.txt').write_text('\n'.join(names)+'\n')
PY
while IFS= read -r unit; do
  test -n "$unit" || continue
  systemctl start "$unit"
done < /root/verdaxis-phase0/timers-restore.txt
```

Expected exact original active/enabled/masked states (this procedure never changes
enablement), no unknown jobs. Staging backup/monitor resume only after prod
acceptance. Keep grace sender off and failed analytics relay untouched.

### B11. KYC key ring after reopening (PM and founder)

Follow `docs/runbooks/security-v2-operator-review.md`, KYC key operations, adapting
only to the approved production key path `/etc/verdaxis/kyc-document-keys.json`.
Inspect first; never overwrite an existing ring. Founder creates/holds encrypted
complete offline copy, uploads encrypted archive to Drive, keeps recovery secret
separate and proves restore. Then PM runs unit-installer preflight and separately
approved restart. No raw keys/documents in chat, CI or this report. Until then
storage 503 is expected, readiness remains available.

## Acceptance limits

Parser/syntax tests do not execute host commands, validate secrets, prove BPF,
or establish systemd/Caddy/backup correctness. P0-5 deliberately narrows Task 10;
no fixed-path rollback proof is claimed. The PM sequencing decisions above resolve
the four design items. R1–R3 host-tool fixes, final input attestation and the
scratch business-smoke adapter still require evidence before execution approval.
A failed pre-write step remains closed; after reopening preserve all new writes.

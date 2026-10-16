# Phase 0 host appendix — 16 October 2026

**Status: command-contract preparation; M2 is NOT closed for execution.**
This is the single host appendix to [phase0-cutover.md](phase0-cutover.md).
The PM must resolve the questions below, review the resulting exact bytes and
rehearse them before authorizing Task 11. An unanswered question is a STOP, not
permission to choose a convenient path, role, port or bypass during the window.
No command in this document was run on a live service by its author.

## PM questions — blocking facts, not implementation choices

1. **Scratch boundary:** which installed facility supplies a separate Linux
   systemd instance, private mounts, hostname and network namespace? Supply its
   exact create/enter/stop commands, image/rootfs digest, scratch cluster name,
   unused port, data path and socket `/var/run/postgresql`. Neither `17/main`
   (5432) nor PM-owned `17/test` (5499) may be used. The inventory does not
   establish that nspawn, a suitable container image or a VM exists. Supply the
   exact egress deny and verification commands, including live DB/API, DNS and
   provider denial. This blocks Part A at A1.
2. **Staging authority:** confirm the PM's authorized staging SSH login and sudo
   route. The inventory names staging files and services, not a `verdaxis-admin`
   login there. Do not assume that account exists. Supply the full effective
   `verdaxis-backup.service`, monitor and verify units, their pending-job state,
   and any service dependencies. We choose to pause BOTH staging timers during
   rehearsal/cutover; the 03:00 SGT pull may not overlap either window.
3. **Configuration bytes:** supply root-private copies and digests of prod's
   complete Caddyfile, effective backend unit and every drop-in (including the
   `.env.analytics` loader), backup producer scripts, staging `backup-db.sh` and
   `verify-backup-restore.sh`. The inventory gives locations, not replacement
   anchors or restore CLI flags. Supply reviewed maintenance and smoke-only
   Caddyfiles, and exact backup patches/restore invocation before B1. Do not use
   global search/replace on scripts or print their secret-bearing contents.
4. **Release/install inputs:** approve full backend/frontend SHAs, archive and
   package hashes, Linux Python executable/version, local Git mirror/wheel-cache
   locations and permissions, and the exact empty-target PostGIS preparation.
   All paths introduced below under `/root/verdaxis-phase0` and
   `/opt/verdaxis-phase0` are proposed new work products, not claimed existing
   host facts. Approve them after inspecting destinations. Supply the private
   manifest builder/observations and Settings launcher described below; no
   generic manifest-generation CLI exists in the release.
5. **Source read authority:** production tools call `pg_control_system()` as
   `verdaxis_backup` and `verdaxis_migrator`. The inventory lists role attributes,
   not function EXECUTE ACLs. Prove both existing roles can execute that read,
   and prove backup SELECT on every source object, without modifying old DB
   grants. If absent, stop for a separate authority decision, not a grant here.
6. **Backups and monitor:** approve exact script patches for both artifact
   producers and the staging restore/status inventory. Choose new prefix
   `verdaxis-prod`, preserve all `verdaxis-` artifacts and disable retention
   deletion during the window. Supply the exact manual restored-backup command
   and isolated restore destination. There is **no installed reader** of
   `.runtime-monitor.env`; accept no new reader in Phase 0, or identify and
   separately approve one. Supply the demo identity file's exact keys and safe
   binding change before rearming demo. Do not assume it reads the new file.
7. **Frontend/key recovery:** supply the protected Vercel rollback execution
   context (CLI 50.37.2, existing private auth directory, scope, prior deployment
   URL/ID and asset), approved smoke route allowlist, key creation/recovery tool
   and exact encrypted-archive commands. The founder must hold the recovery
   secret separately. A raw key or real customer capture may never enter this
   repository, the Mac, GitHub Actions or a release report.
8. **Window inputs:** approve deadline, drain timeout, disk reserve, source
   revision/identity evidence, signed pre-write rollback acceptance, and pause
   all PM test lanes. The inventory's free disk and idle sessions are dated
   observations, not current acceptance. Supply actual timer active/enabled/
   masked states; do not infer them from schedules.

The missing items above prevent a truthful start-to-end host execution today.
Resolve them in this file before sign-off. Sections below fix the known command
interfaces and prescribe the order and exact commands where facts are known.
A `STOP` row has no substitute command; never improvise one.

## Roles, fixed inputs and failure rule

- **PM:** runs every host command, prod SSH login `verdaxis-admin`, sudo for root
  actions, service account `verdaxis-prod` only where stated. The AI never SSHs.
- **Founder:** announces the window; owns the encrypted key-ring recovery secret;
  approves recovery evidence; sends the refresh note at reopening.
- **Every failure:** leave Caddy maintenance, target CONNECT fence and external
  start fence closed. Preserve both databases, reports, code and journal. Do not
  trap EXIT to reopen, retry copy, clear state, kill unknown sessions or revert
  only a SHA. After customer writes, no return to the old database without a new
  reconciliation decision. No `pg_hba.conf` changes and no new live roles.

All root steps run in a clean root shell after the PM logs in, with `set +x`,
`set -euo pipefail`, `umask 077`. Commands below are not a single script: stop at
each expected-result check. A second operator reviews the private evidence.
All new paths must be absent or already reviewed; no silent overwrite.

| Fixed item | Value / authority |
|---|---|
| Prod host | `vmi3623757`, `169.58.37.164` |
| Prod database | `17/main`, 5432, system ID `7691585279836774230` |
| Old source | `verdaxis`, OID 21404 at inventory; recheck, never recapture silently |
| Old source SHA | `c08266bd6969143812dec7c5033338428cd5f159` |
| New target | `verdaxis_prod`, new observed OID; `p0_20261016_union` |
| Roles | source `verdaxis_backup`; target owner/tool `verdaxis_migrator`; app `verdaxis_app` |
| Runtime root | `/home/verdaxis-prod/verdaxis/prod/be` |
| Tool install | `/opt/verdaxis-phase0` only; venv below it |
| Private inputs | `/root/verdaxis-phase0`, root 0700, files 0600 |
| Durable start fence/journal | `/var/lib/verdaxis/phase0`, root 0700 |
| Real authority files | staging `/home/jons-openclaw/VPS_MAP.md`, `/home/verdaxis-prod/verdaxis/PRODUCTION_HOST.md`, `/home/jons-openclaw/artifacts/verdaxis-migration-20261001/OPERATIONS.md` |

There is no production-host `PRODUCTION_HOST.md` at the old documented path.
The PM reads the three staging files through the approved staging login; keep a
private approved copy with the window record. Never treat a missing prod file
as authorization to ignore host identity.

## Input contract and installation (used by A and B)

**PM / expected:** one reviewed immutable archive installed root-owned, not
writable by group/other; packages attested independently; exact tool tree digest
matches the authority. **STOP:** unresolved Q4, missing packages, dirty archive,
unknown digest or destination collision. Prepare before maintenance, with no
real data in the package/cache boundary.

Use `git archive` of the approved full SHA, not a working directory copy. The
PM-signed input list must name the archive, local bare mirror and wheelhouse.
Mirror the exact `staging` ref and retain the old SHA; check both full SHAs with
`git --git-dir=... rev-parse`. The offline runtime's `origin` must be the approved
read-only local mirror. Copy complete cached dependency bytes before isolation.
Install packages with `--no-index --find-links` from that wheelhouse and run
`pip check` inside the rehearsal. Do not rely on exported PIP variables: the
normal deploy helper clears its environment. Its later install must find every
pinned requirement already installed in the final-path venv. An offline fetch
or pip failure is a STOP, never permission to enable outbound networking.

The exact mirror/cache paths and preparation commands remain Q4. Do not publish
a guessed SSH remote or silently change the normal deploy helper.

### Separate files: no shared manifest and no synthetic secrets

These are **production-mode** inputs, also in the isolated scratch instance:

| Tool | Manifest | Source secret | Target secret | New output |
|---|---|---|---|---|
| bootstrap | `bootstrap.json` | `bootstrap-source.env` | `bootstrap-target.env` | `bootstrap-receipt.json` |
| copy | `copy.json` | `copy-source.env` | `copy-target.env` | `copy-receipt.json` plus attempt/failure evidence |
| verify | `verify.json` | `verify-source.env` | `verify-target.env` | `verified.json` |

Every filename in the table is below `/root/verdaxis-phase0`. Each secret is
exactly one line `DATABASE_URL=postgresql+asyncpg://...`, with the exact role,
percent-encoded nonempty password, numeric loopback endpoint and exact database.
No URL query. Different source/target passwords; private distinct files per tool.
**Do not use `MIGRATOR_DATABASE_URL` in these six tool secret files.** That is the
historical synthetic bootstrap format, not the production interface. The runtime
Settings file separately needs BOTH distinct app `DATABASE_URL` and
`MIGRATOR_DATABASE_URL`, plus consistent `DATABASE_NAME`, user and endpoint parts.

The fixed `/root/verdaxis-phase0/authority.json` has exactly the COMMON_FIELDS
listed in `docs/phase0/progress-hosttools.md:46–65`: approved production mode,
host nodename, release/tree/source identity, exact source/target endpoints/OIDs/
roles/system identifier, frozen source catalog/fingerprint, target datacl and
bootstrap receipt path. Derive the tree digest from reviewed archive bytes and
freeze the source only after drain. Do not self-approve arbitrary target data.

Each tool manifest repeats the authority exactly and adds `tool`, `report`,
`bootstrap_receipt_sha256`. Bootstrap uses JSON null and its report equals the
authority's bootstrap receipt. After bootstrap, hash its actual report bytes and
prepare copy/verify manifests with that hash. Each report destination is new.
Verify does not require the copy receipt; lost copy receipt means verify with a
new approved verify manifest/report, never replay copy.

Assertion manifests are a **different format** (see main runbook). Use:
`old.json`, `new-preinstall.json`, `new-final.json`, `rollback.json`. They contain
raw backup `password_file` paths, not the tool URLs. Pre-install hashes include
currently installed units/drop-ins; final hashes include the newly installed
units/drop-ins. Only final tuple approval can issue a new start permit. Do not
reuse bootstrap/copy manifests for `assert_host`.

The PM must prepare a root-private Settings launcher with the exact production
Settings and release, using file input rather than passwords in argv. From the
tool installation working directory its environment is `ENVIRONMENT=production`,
`RELEASE_SHA=<approved full SHA>`, with matching target app/migrator Settings,
UTC and `KYC_EXECUTION_GATE_ENABLED=false`. The interpreter is always
`/opt/verdaxis-phase0/venv/bin/python`. Q4 blocks execution until that launcher,
authority-builder and private input set have exact reviewed commands. The tools
must not be run from the live checkout with ambient `.env` routing.

### Parser-checked production tool sequence

**PM / expected:** bootstrap then copy then independent verify exit 0, publish
new root-private reports and leave frozen source unchanged. **STOP:** any refusal,
missing report, changed source, extra session, receipt mismatch or existing output.
Do not interpret a dry run as connection proof. Run these only after A/B fence
steps and authority approval; create copy/verify manifests after bootstrap.

```bash
/opt/verdaxis-phase0/venv/bin/python -m scripts.phase0.bootstrap \
  --manifest /root/verdaxis-phase0/bootstrap.json \
  --source-environment-file /root/verdaxis-phase0/bootstrap-source.env \
  --target-environment-file /root/verdaxis-phase0/bootstrap-target.env --apply
/opt/verdaxis-phase0/venv/bin/python -m scripts.phase0.copy \
  --manifest /root/verdaxis-phase0/copy.json \
  --source-environment-file /root/verdaxis-phase0/copy-source.env \
  --target-environment-file /root/verdaxis-phase0/copy-target.env \
  --source-sha "$OLD_SHA" --target-sha "$RELEASE_SHA" --apply
/opt/verdaxis-phase0/venv/bin/python -m scripts.phase0.verify \
  --manifest /root/verdaxis-phase0/verify.json \
  --source-environment-file /root/verdaxis-phase0/verify-source.env \
  --target-environment-file /root/verdaxis-phase0/verify-target.env \
  --report /root/verdaxis-phase0/verified.json
```

## A. Task 10 — isolated host-local rehearsal

### A1. Establish isolation before real data (PM; Q1 STOP)

Pause every test lane first. Do not change the PM test host setup or use its
`17/test` cluster. Q1 must supply a complete separately approved Linux boundary
with systemd, private fixed paths and a separate PostgreSQL 17/PostGIS cluster.
Its private socket is `/var/run/postgresql`; choose no port from guesswork.
Inside it use `verdaxis` and `verdaxis_prod`, existing production role *names*
but distinct scratch role objects/passwords. No live credentials in scratch.

**Expected:** separate cluster ID and OIDs, no route/mount to `17/main`, live API,
Caddy admin, public DNS or providers (Resend, Gemini, Webz, Telegram, Umami).
Git mirror/cache are read-only local inputs. No SSH agent, home-secret mounts,
remote logging or public listener. **STOP:** any reachable provider/live endpoint,
unknown namespace or public DNS fallback. Default-deny all non-loopback egress
inside the isolated boundary is required; a list of provider IPs is insufficient.
The inventory supplies no safe command to create this facility. Do not replace
this gate with `unshare` alone or claim the capped test user is the rehearsal.

### A2. Prepare and restore (PM)

Read the real authority files, check prod identity without stopping live workers,
and obtain an existing backup-role dump through the approved private backup
mechanism. Restore only into scratch old `verdaxis`, with old pinned schema,
ownership and grants. Preserve real dump/logs only on the host. Q3/Q4/Q6 must
supply the exact dump/restore command, service/passfile and owner preparation.
`pg_restore --list` alone is not acceptance. Expected complete restore and
independent counts/hashes; any missing relation, ACL or extension is a STOP.

Use the old pinned tuple at the private fixed runtime path. Block all producers
and let only the existing sequencer drain. Apply B2–B4 inside scratch, with the
scratch host/endpoint assertions, never live authority. Freeze after the post-stop
zero. Create and fence scratch target as B5 with scratch observations. Install
scratch-only authority and run the parser-checked sequence above.

### A3. Run the actual switch and rollback (PM)

Execute B6–B8 in the isolated boundary with scratch Caddy and local-only DNS
for `api.verdaxis.exchange`. Never let scratch DNS fall through publicly. Execute
the main runbook's complete old-tuple restart block, holding both deploy locks
and the separate fresh operator lock. Restore final paths so venv shebangs work.
Do not actually dispatch a Vercel promotion from this boundary; use the captured
previous-deployment procedure in a separately authorized frontend test context.

**Expected:** real cached systemd unit initially lacks the new fence; after
reload it contains it. No-permit and stale-permit starts deny. Released locks
and reboot deny. Kill at each directory/identity/unit/restart/readiness boundary;
no old code executes on new data or vice versa, and public writes remain closed.
Restore old directory, venv, units/drop-ins, environment, backup settings and
runtime/monitor files; exact old readiness and frozen source hashes must match.
**STOP:** any unexplained internal write, mismatched tuple, surviving provider
route or failure that reopens automatically. Do not perform these fault
injections against live production.

### A4. Required Linux evidence (PM and founder)

Run the separately approved Linux acceptance commands through the isolated test
facility, not on the production app database. Record completed counts for the
19 Bash deploy cases, `/proc` four-worker/SSE case, nobody monitor case, app-role
business commands and ACL denials. Prove UTC on fresh app/migrator/backup/CLI
connections and preserve stored epochs/Singapore FAME dates. Prove four-worker
KYC credentials, gate off, unavailable 503, decrypt 500, proxy-504 uncertainty,
two-row retention, audited no-store download and RESTRICT. No real customer
screenshots leave the host. Founder proves encrypted complete-ring recovery
with secret held separately; Q7 supplies exact commands. A missing item blocks
Task 11. Keep all scratch data until separately approved cleanup.

## B. Task 11 — one production window

### B1. Approval, real host checks and capture (PM; founder announces)

Founder announces the bounded maintenance window before closure. PM pauses test
lanes and resolves all questions. From the authorized prod login:

```bash
sudo -n true
test "$(hostname -s)" = vmi3623757
getent ahostsv4 api.verdaxis.exchange
sudo systemctl show verdaxis-backend.service -p User -p WorkingDirectory -p ExecStart
sudo -u postgres psql -X -d postgres -v ON_ERROR_STOP=1 -Atc "SELECT system_identifier FROM pg_control_system(); SELECT datname,oid FROM pg_database WHERE datname IN ('verdaxis','verdaxis_prod');"
```

Expected DNS `169.58.37.164`, user `verdaxis-prod`, fixed `prod/be`, source OID
21404/system ID as recorded, no `verdaxis_prod`. STOP on drift or collision; do
not drop/reuse target. Preserve old complete checkout/venv/env/drop-ins, backup
config/scripts/.pgpass and previous frontend deployment before modifying them.
Q3 supplies exact reviewed private capture commands; never print `.env` diffs.

### B2. Record and pause timers (PM on both hosts)

On prod, root shell, with private evidence directory already approved/created:

```bash
timers=(verdaxis-auth-maintenance.timer verdaxis-demo-activity.timer verdaxis-onboarding-attention.timer verdaxis-order-expiry-reminders.timer verdaxis-news-refresh.timer verdaxis-product-analytics-prune.timer verdaxis-local-db-backup.timer)
test ! -e /root/verdaxis-phase0/prod-timers.before
for unit in "${timers[@]}"; do
  printf '%s\n' "$unit"
  systemctl show "$unit" -p ActiveState -p UnitFileState -p LoadState
done > /root/verdaxis-phase0/prod-timers.before
systemctl stop "${timers[@]}"
systemctl list-jobs --no-pager
```

Expected every listed timer stopped; record active/enabled/masked states first.
Onboarding attention and local backup ignore deploy state: stopping those timers
is mandatory. This list plus the backend covers all eight recorded writers/jobs.
The failed analytics relay is not repaired or restarted by this procedure.

On staging, **only through Q2's approved PM login**, record/pause:

```bash
timers=(verdaxis-monitor.timer verdaxis-backup.timer)
for unit in "${timers[@]}"; do
  printf '%s\n' "$unit"
  sudo systemctl show "$unit" -p ActiveState -p UnitFileState -p LoadState
done
sudo systemctl stop "${timers[@]}"
sudo systemctl list-jobs --no-pager
sudo systemctl show verdaxis-recover.service -p LoadState -p UnitFileState -p ActiveState
```

Keep that output in the approved staging private evidence destination (Q2).
Expected recovery remains masked, monitor and 03:00 SGT backup pull paused. Wait
for existing pulls/backups/provider calls to finish naturally; do not kill them.
No manual monitor-verify while maintenance is closed. STOP on unknown job or
active writer beyond the approved drain deadline. Never change global role
properties, terminate sessions or disable an unrecorded schedule.

### B3. Close production Caddy, load start fence, then drain (PM)

**Maintenance point is prod `/etc/caddy/Caddyfile` only.** Staging's stale prod
blocks do not control direct DNS. Q3 must provide a reviewed full replacement
that returns 503 for **every route/method** except exact GET `/health/ready`.
This includes old cached tabs, authenticated GETs, SSE, SQLAdmin, `/script.js`
and `/api/send`; analytics must not bypass maintenance. Preserve TLS and trusted
proxy configuration. No wildcard `/api/*` allow, and no customer-authenticated
smoke. Existing SSE sockets need explicit closure: reload alone is not proof.

Once Q3's private `Caddyfile.maintenance` and original backup are approved:

```bash
sudo caddy validate --config /root/verdaxis-phase0/Caddyfile.maintenance --adapter caddyfile
sudo install -o root -g root -m 0644 /root/verdaxis-phase0/Caddyfile.maintenance /etc/caddy/Caddyfile
sudo systemctl restart caddy
```

Restart deliberately closes old proxy/SSE connections. Expected maintenance is
active after the brief TLS interruption. Q3 must verify the effective Caddy unit
loads this file. Check anonymous status codes only, without credentials:

```bash
for path in /api/orderbook /api/auth/me /api/stream /admin /script.js /api/send; do
  test "$(curl --silent --output /dev/null --write-out '%{http_code}' "https://api.verdaxis.exchange$path")" = 503
done
test "$(curl --silent --output /dev/null --write-out '%{http_code}' -X POST https://api.verdaxis.exchange/api/send)" = 503
```

These are probes, not a complete route proof; inspect the deny-all config and
prove old SSE closure in rehearsal. STOP if any forbidden request reaches a
runtime/relay. Keep Caddy closed on every subsequent failure.

Install the main runbook's exact root-owned external guard/drop-in, before any
source replacement. Use its reload/show checks. No permit exists yet. The root
operator publishes the durable fence with exclusive create and fsync:

```bash
python3 -I - <<'PY'
import os
from pathlib import Path
root = Path('/var/lib/verdaxis/phase0')
assert root.is_dir() and not root.is_symlink()
assert root.stat().st_uid == 0 and root.stat().st_mode & 0o777 == 0o700
fd = os.open(root / 'fence', os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
with os.fdopen(fd, 'wb') as output:
    output.write(b'phase0\n'); output.flush(); os.fsync(output.fileno())
fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
os.fsync(fd); os.close(fd)
PY
sudo systemctl daemon-reload
sudo systemctl show verdaxis-backend.service -p LoadState -p NeedDaemonReload -p DropInPaths -p ExecStartPre
```

Expected loaded fence plus original two pre-start guards; no pending reload.
Allow in-flight requests/oneshots to finish. Observe these service units until
inactive/dead and Job empty/0; no new timer firing can start them:

```bash
for unit in verdaxis-auth-maintenance.service verdaxis-demo-activity.service verdaxis-onboarding-attention.service verdaxis-order-expiry-reminders.service verdaxis-news-refresh.service verdaxis-product-analytics-prune.service verdaxis-local-db-backup.service; do
  sudo systemctl show "$unit" -p ActiveState -p SubState -p Job
done
```

Expected drained providers/jobs. Keep API only for its existing sequencer. Q4's
backup connection reads pending NULL `stream_seq` to zero after producer drain;
then stop backend and recheck all sessions and zero with the assertion:

```bash
sudo systemctl stop verdaxis-backend.service
/opt/verdaxis-phase0/venv/bin/python -m scripts.phase0.assert_host \
  --manifest /root/verdaxis-phase0/old.json --check outbox-empty
```

Expected no other source session, pending job or pending outbox. Backup/psql
sessions count too. STOP on a late commit; drain again under the same closure,
never sequence by ad hoc SQL. Snapshot only after this final zero.

### B4. Final source snapshot and frozen authority (PM)

Take the final dump with the read-only backup credentials, then perform the
Q6-approved isolated restore and full comparison. Keep old dumps and original
backup files; do not rely on archive-list success. Record source catalog,
table/column hashes, sequence state and frozen authority while no writer can
reconnect. Q4 must supply the exact signed observation/builder commands. Source
`verdaxis` rows, schema, ACLs, settings and name remain untouched thereafter.

### B5. Create and fence only the new database (PM)

After rechecking absence, use the existing migrator, no new live roles:

```bash
sudo -u postgres createdb --owner=verdaxis_migrator verdaxis_prod
sudo -u postgres psql -X -d postgres -v ON_ERROR_STOP=1 <<'SQL'
REVOKE CONNECT ON DATABASE verdaxis_prod FROM PUBLIC;
REVOKE CONNECT ON DATABASE verdaxis_prod FROM verdaxis_app, verdaxis_backup;
GRANT CONNECT ON DATABASE verdaxis_prod TO verdaxis_migrator;
ALTER DATABASE verdaxis_prod SET timezone TO 'UTC';
SELECT datname,oid,datacl::text FROM pg_database WHERE datname='verdaxis_prod';
SQL
```

Expected fresh target OID, exact non-null ACL with only migrator CONNECT and no
grant option. Existing sessions/superusers bypass CONNECT: mandatory session
checks still apply. Q4 must provide target-only PostGIS setup and prove no role
per-database override defeats UTC. No global ALTER ROLE and no old DB settings.
Record exact target ACL and OID independently; install approved authority.
Run bootstrap/copy/verify sequence above. Verify bootstrap receipt before making
the later two manifests. All reports must exist privately and match authority.

### B6. Install new tuple and backup bindings (PM)

Preserve old complete tuple, then use the reviewed rename procedure at the fixed
path. Build venv there offline. Change **all three** production names:
`DATABASE_NAME`, app `DATABASE_URL`, `MIGRATOR_DATABASE_URL`. Keep exact roles,
secrets, `.env.analytics`, gate off and approved vault credential source. Publish
matching release/monitor identity before credentials reach new code.

```bash
/opt/verdaxis-phase0/venv/bin/python -m scripts.phase0.assert_host \
  --manifest /root/verdaxis-phase0/new-preinstall.json --check tuple
sudo -n -u verdaxis-prod -H /home/verdaxis-prod/verdaxis/prod/be/scripts/deploy.sh --dry-run
sudo /home/verdaxis-prod/verdaxis/prod/be/scripts/install_systemd_units.sh \
  --dry-run --environment production --source-ref "$RELEASE_SHA"
sudo /home/verdaxis-prod/verdaxis/prod/be/scripts/install_systemd_units.sh \
  --apply --environment production --source-ref "$RELEASE_SHA"
sudo systemctl daemon-reload
sudo systemctl show verdaxis-backend.service -p LoadState -p NeedDaemonReload -p DropInPaths -p ExecStartPre
/opt/verdaxis-phase0/venv/bin/python -m scripts.phase0.assert_host \
  --manifest /root/verdaxis-phase0/new-final.json --check tuple
```

Expected exact approved SHA printed by dry-run, final installed unit hashes,
four workers and independent fence composition. STOP on drift. Pre-install
manifest is not authority for a final start.

**Backup switch before acceptance:** with both local and staging backup timers
still stopped, preserve `/etc/verdaxis-backup-export.conf` and
`/home/verdaxis-backup-export/.pgpass` privately. Q3/Q6 exact patch changes
`PGDATABASE=verdaxis_prod` and duplicates the old database-specific .pgpass entry
for the new database, retaining old credentials for rollback. Keep file owner
and 0600 mode. Never print password lines. Both live producers read that config:
`/usr/local/sbin/verdaxis-local-db-backup` and
`/usr/local/libexec/verdaxis-backup-export`. Q6 must patch local custom dump prefix
to `verdaxis-prod-<UTC>.dump` and staging's plain/gzip artifact/status/restore
inventory to `verdaxis-prod`, database `verdaxis_prod`. Preserve old `verdaxis-`
dumps, B2 objects, scripts and config; suspend deletion, do not rename old dumps.

Run both approved producer/restore checks manually only after target backup
CONNECT/SELECT is accepted. Never pretend `pg_restore --list` is a full restore.
A restored backup goes only into the approved isolated scratch destination.
Q6 blocks rearming until full restore evidence and expected inventories pass.
Do not install the uninstalled monitor backup timer as a competing producer.

**Monitor binding:** no `.runtime-monitor.env` reader is installed on prod.
Verify the generated file against full runtime SHA via `assert_host`; do not
claim monitoring consumes it. Preserve/update only the Q6-reviewed existing demo
identity binding at `/etc/verdaxis-monitor/demo/production.env`; keep onboarding
`/etc/verdaxis-monitor.env`. Staging's monitor continues its existing public
checks after reopening. New reader activation is separate work, not implicit.

### B7. Publish one-use permit and normal deploy (PM)

As root, only after tuple approval; inspect stale entries and confirm no previous
operator owns the lock. No normal deploy lock is held by this root shell. Remove
a stale permit only after that inspection. The following refuses if it exists:

```bash
test ! -e /var/lib/verdaxis/phase0/permit.json
test ! -L /var/lib/verdaxis/phase0/permit.json
python3 -I - <<'PY'
import os, tempfile
root = '/var/lib/verdaxis/phase0'
fd, name = tempfile.mkstemp(prefix='.operator.', dir=root)
os.fsync(fd); os.close(fd)
os.replace(name, root + '/operator.lock')
fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
os.fsync(fd); os.close(fd)
PY
exec 9<>/var/lib/verdaxis/phase0/operator.lock
flock -n 9
export RELEASE_SHA
python3 -I - <<'PY'
import json, os, re, tempfile
root = '/var/lib/verdaxis/phase0'
sha = os.environ['RELEASE_SHA']
assert re.fullmatch('[0-9a-f]{40}', sha)
info = os.stat(root + '/operator.lock')
permit = dict(environment='production', release_sha=sha, database='verdaxis_prod',
              lock_device=info.st_dev, lock_inode=info.st_ino)
fd, name = tempfile.mkstemp(prefix='.permit.', dir=root)
with os.fdopen(fd, 'w') as output:
    json.dump(permit, output); output.flush(); os.fsync(output.fileno())
os.replace(name, root + '/permit.json')
fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
os.fsync(fd); os.close(fd)
PY
/opt/verdaxis-phase0/venv/bin/python -m scripts.phase0.start_fence \
  --environment production --expected-release-sha "$RELEASE_SHA" --database verdaxis_prod
sudo -n -u verdaxis-prod -H env APPROVED_RELEASE_SHA="$RELEASE_SHA" \
  MIGRATION_APPROVED_SOURCE_SHA="$RELEASE_SHA" \
  MIGRATION_EXPECTED_CURRENT_REVISION=p0_20261016_union \
  MIGRATION_TARGET_REVISION=p0_20261016_union \
  /home/verdaxis-prod/verdaxis/prod/be/scripts/deploy.sh
```

For a separately captured readiness response saved privately as `ready.json`,
validate its exact environment/full SHA with the current parser:

```bash
/opt/verdaxis-phase0/venv/bin/python -m scripts.validate_health_response \
  --environment production --release-sha "$RELEASE_SHA" < /root/verdaxis-phase0/ready.json
```

The captured file supplies stdin; a missing file or empty response refuses.
Do not type a fabricated response. The assertion below also fetches readiness
itself and verifies the full tuple.

Expected helper acquires both normal locks, converges target ACLs and reaches exact
readiness before clearing state. No checkpoint/installer override. Keep fd 9 open
through helper exit. On success revoke permit durably then close fd 9. On failure
keep maintenance and fence, revoke permit manually and inspect actual state.

```bash
python3 -I - <<'PY'
import os
root = '/var/lib/verdaxis/phase0'
os.unlink(root + '/permit.json')
fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
os.fsync(fd); os.close(fd)
PY
exec 9>&-
/opt/verdaxis-phase0/venv/bin/python -m scripts.phase0.assert_host \
  --manifest /root/verdaxis-phase0/new-final.json --check readiness
```

**Verification transition:** after runtime CONNECT is granted, the independent
production verify tool deliberately refuses its earlier datacl boundary. For
post-start comparison, stop new workers, close all target sessions and restore
only the previously approved target CONNECT ACL, then use a newly approved verify
report/manifest. Do not rewrite old source authority or blindly recapture baseline.
Q4 must supply these exact target refence/regrant commands before execution.
Repeat copy/source hashes around starts; unexplained internal writes STOP.

### B8. Pre-write emergency rollback (PM; never routine live drill)

Only before reopening and after signed internal-write reconciliation. Stop new
workers; revoke permit; retain failed target and tuple. Use `rollback-safe` with
the approved target identity/evidence and then restore complete old tuple:

```bash
/opt/verdaxis-phase0/venv/bin/python -m scripts.phase0.assert_host \
  --manifest /root/verdaxis-phase0/rollback.json --check rollback-safe
```

Q3/Q4 supply exact preserved directory and unit restore commands. Restore original
backup scripts/config/.pgpass and runtime/monitor environment, not metadata only.
Change fence literal to `verdaxis`, reload, inspect loaded commands, then use the
main runbook's old two-lock restart block with a fresh root permit bound to old
SHA/database. No Alembic downgrade or new ACL helper on old DB. Validate old local
and public readiness against the pinned SHA; keep maintenance until acceptance.

### B9. Frontend promotion and previous deployment restoration (PM)

Read frontend `.github/workflows/release-vercel.yml` and
`docs/vercel-production-release.md` from the approved frontend SHA. Workflow
requires source equal to canonical staging tip and exact backend readiness.
Before dispatch, Q3/Q7 must provide a **smoke-only** Caddy config: allow only the
reviewed anonymous GET routes needed by rendered smoke, keep every authenticated
GET, mutation, SSE, SQLAdmin and analytics route denied. No general reopen for
CI. If the smoke requires customer data or writes, STOP, do not weaken the fence.

```bash
gh workflow run release-vercel.yml --repo Verdaxis/verdaxis-frontend --ref staging \
  -f source_sha="$FRONTEND_SHA" -f backend_release_sha="$RELEASE_SHA" \
  -f candidate_only=false -f notify_release=true
gh run watch "$FRONTEND_RUN_ID" --repo Verdaxis/verdaxis-frontend --exit-status
```

Q7 binds the exact run ID to this dispatch. Expected one immutable build, canary,
exact backend SHA, captured prior deployment URL/ID/asset, same-candidate promotion
and all production aliases verified. Preserve bounded report. No rebuild or
manual production deploy. Failure leaves API fenced; no automatic backend rollback.

If pre-write emergency rollback needs frontend restoration, PM uses the existing
protected Vercel auth context (never token argv), CLI 50.37.2:

```bash
vercel --scope "$VERCEL_ORG_ID" rollback "$PRIOR_URL" --yes --timeout=3m
```

Expected aliases at the exact captured prior deployment and prior asset, then
rendered smoke. Q7 must supply actual auth-directory/inspection/smoke commands;
this line alone is not restore acceptance. Do not infer prior identity from a
human-friendly deployment name. Keep full maintenance during backend restoration.

### B10. Signed acceptance, fence cleanup and reopening (PM and founder)

PM checks source preservation, backup restore, exact readiness, UTC fresh
connections, vault recovery, no sessions/jobs on old DB and no unexplained writes.
Founder confirms encrypted complete key ring is in Drive and recovery secret is
held separately. No customer document archive. No key contents in logs.

Under still-closed Caddy, after successful signed rollback gate, inspect and remove
only the one-use permit/fence/drop-in; retain the control directory and evidence:

```bash
test ! -e /home/verdaxis-prod/verdaxis/prod/be/.runtime-deploy/production.state
test ! -L /home/verdaxis-prod/verdaxis/prod/be/.runtime-deploy/production.state
test ! -e /var/lib/verdaxis/phase0/permit.json
test ! -L /var/lib/verdaxis/phase0/permit.json
rm /var/lib/verdaxis/phase0/fence
rm /etc/systemd/system/verdaxis-backend.service.d/90-phase0-fence.conf
python3 -I - <<'PY'
import os
for path in ('/var/lib/verdaxis/phase0', '/etc/systemd/system/verdaxis-backend.service.d'):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    os.fsync(fd); os.close(fd)
PY
systemctl daemon-reload
systemctl show verdaxis-backend.service -p NeedDaemonReload -p DropInPaths -p ExecStartPre
```

Expected fence gone, both normal guards retained, no pending reload. Restore
Q3's approved production Caddyfile and validate before reload. Append/fsync the
signed `customer-writes-open` journal entry at the actual reopening. After this
point preserve new writes; rollback needs a separate reconciliation decision.

At reopening PM verifies the **new frontend entry HTML** and hashed asset at
`app.verdaxis.exchange`; existing update banner handles old tabs. Founder sends
**"please refresh"** to active pilot users. No forced reload or per-user gate.

Restore **only timers recorded active**, without enabling inactive/masked units,
on both hosts. Q2/Q8 must bind the reviewed before-state to exact restore list;
inspect each state first, then `systemctl start` only those named timers. Do not
restore a timer whose new database/backup/demo identity has not passed acceptance.
Keep grace sender off; do not repair failed analytics relay. Observe first jobs
and verify single scheduling. Preserve old dumps/config/databases for approved
retention; no cleanup is authorized by this appendix.

## Sign-off boundary

Parser tests prove option spelling and required arguments only. They do not
prove private manifests, loaded units, egress, backup scripts, frontend smoke or
host permission. Until Q1–Q8 have exact reviewed answers and Part A passes without
improvisation, M2 remains open and Part B must not begin.

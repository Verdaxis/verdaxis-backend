# Phase 0: one-use fenced cutover checklist

## Current state — 2026-10-16

**Source preparation only. No host operation is authorized.** This checkout is
based on `e61400f3`, with Task 4b's `p0_20261016_union` and the production host
tools merged. Use the FINAL interface in `docs/phase0/progress-hosttools.md`,
not its historical checkpoints. Task 9 acceptance, independent host-tool review,
founder approval and separate host-operation approval remain gates.

The single operator appendix is [phase0-host-appendix.md](phase0-host-appendix.md).
Its PM questions are hard stops: the supplied inventory does not yet establish
all executable host commands, so sign-off M2 remains open. Task 10 rehearses the
complete switch-back in an isolated host-local cluster. Task 11 permits rollback
only for a pre-write emergency. The appendix is the current host-specific record;
the sections below retain the invariant and guard contracts.

## Hard stops and required private evidence

Read the three real staging-host authority files named in the appendix; the
old production-host `PRODUCTION_HOST.md` path does not exist. Confirm DNS,
hostname, service User/WorkingDirectory, mounted filesystems, proxy ownership and
all active/enabled/masked timer states. The production root stays
`/home/verdaxis-prod/verdaxis/prod/be`; its canonical branch becomes `staging`.
The old complete release still requires `verdaxis`. The new release requires
`verdaxis_prod`, with the existing exact app, migrator and backup roles.

Do not proceed with placeholders. The operator must add the actual commands for:

- Proxy maintenance and proof that public mutations, including old browser tabs,
  cannot reach either runtime. Block provider egress in the rehearsal boundary.
- Every external monitor, recovery, canary, demo, email and host cron writer.
  Include processes and pending systemd jobs, not only timer names. The assertion
  helper's fixed unit list is a minimum, not a host inventory.
- The host-owned backup producer, its exact new database inventory, artifact
  prefixes and restored-backup verification. Canonical backup verification expects
  `verdaxis_prod` / `verdaxis-prod`, not the old `verdaxis` / `verdaxis` pair.
  Preserve old backups and old producer configuration for rollback. No producer
  exists in this repository; do not install a competing one.
- Monitor identity: the inventory found no installed `.runtime-monitor.env`
  reader on prod. The helper publishes that credential-free file beside the
  runtime artifact; verify it, but do not claim a reader consumes it. Preserve
  the existing demo/onboarding bindings and resolve appendix Q6 before rearming.
  Do not install the source-only dual-host local monitor unchanged.
- Separate scratch-cluster endpoint/socket, system identifier and exact database
  OIDs. The assertion uses `/var/run/postgresql` for the operator's cluster query;
  the scratch cluster must expose its approved port there. Do not substitute the
  live cluster. No arbitrary database-name override is supported.

All real records, password files, manifests, hashes and reports stay on-host.
Use `/root/verdaxis-phase0` (0700) for root-owned 0600 manifests and secret files.
Use `/var/lib/verdaxis/phase0` (0700) for the fence and private journal. Never put
secrets in shell arguments, chat, CI or Git. Keep KYC execution off. Copy no
refresh sessions; preserve KYC reviews and start the encrypted document store
empty. Verify the encrypted complete key-ring backup and recovery separately.

## Read-only assertions

Install the reviewed exact-SHA tools at `/opt/verdaxis-phase0`, outside both
checkout directories. Run with a clean environment and the approved tool venv:

```sh
/opt/verdaxis-phase0/venv/bin/python -m scripts.phase0.assert_host \
  --manifest /root/verdaxis-phase0/new.json --check identity
```

Fixed checks: `identity`, `quiesced`, `outbox-empty`, `tuple`, `readiness`,
`rollback-safe`, `timers`. They do not mutate services, grant permission, execute
manifest command strings or append to a journal. Refusal output is bounded.

The manifest contains `hostname`, `release_sha`, `password_file` (a private raw
backup-role password, not a URL), and `identity` with exactly `database`,
`database_oid`, `cluster_id` (decimal string), `host` (`127.0.0.1`), `port`,
`role` (`verdaxis_backup`) and `revision`. `artifacts` maps absolute paths to
SHA-256 digests, including `.env`, `.runtime-release.env`, the installed backend
unit and `monitor_identity_file`. Include every installed drop-in and unit,
key-source metadata digest, venv receipt and backup configuration digest too.
`rollback-safe` also requires `reconciliation_file` and `reconciliation_sha256`.
A matching report digest is not proof of no customer writes: the operator must
keep maintenance closed and sign the report after inspecting current data.

The database check opens a read-only backup connection, compares database/OID,
role, endpoint and revision, and queries the cluster identifier independently
through local PostgreSQL administrator peer authentication. No control privilege
is granted to the app or backup role. `outbox-empty` requires stopped writers,
no pending jobs, no other database sessions and a fresh zero count of NULL
`stream_seq`. The gate excludes only its own backend PID. Idle psql sessions,
CLIs, timers and backup connections also block it; let their owners close them.
Do not change old database grants or terminate unknown sessions to pass the gate.
`timers` proves quiescence; it does not restore timers.

## Install the separate fence before any source replacement

As root, inspect destinations and preserve any existing files before installing.
Copy `scripts/phase0/start_fence.py` to
`/opt/verdaxis-phase0/start_fence.py`, root-owned 0755. Record its SHA-256. All
parent directories must be root-controlled. The control directory is root-owned
0700; its regular files are root-owned 0600, single-link and never symlinks.
Write `phase0\n` to `fence`, fsync it and the directory. Keep it across reboot.

Install this root-owned drop-in at
`/etc/systemd/system/verdaxis-backend.service.d/90-phase0-fence.conf`:

```ini
[Service]
ExecStartPre=+/usr/bin/python3 -I /opt/verdaxis-phase0/start_fence.py --environment production --expected-release-sha ${RELEASE_SHA} --database verdaxis_prod
```

Do not clear existing `ExecStartPre` entries. The normal deploy-start guard and
migration revision check remain independent. The `+` prefix lets this one guard
read the root-only control directory; it does not run the backend as root.
Run `systemd-analyze verify` on the installed unit, then load the drop-in into
systemd **before any source replacement**. File inspection alone is not enough:

```bash
sudo systemd-analyze verify /etc/systemd/system/verdaxis-backend.service
sudo systemctl daemon-reload
sudo systemctl show verdaxis-backend.service \
  -p LoadState -p NeedDaemonReload -p DropInPaths -p ExecStartPre
```

Stop unless `LoadState=loaded`, `NeedDaemonReload=no`, `DropInPaths` includes the
exact `90-phase0-fence.conf` path, and the manager's `ExecStartPre` includes the
external guard with `--database verdaxis_prod`, the normal two-lock guard and
migration check. Record the loaded command list and the fence file digest.
The fence is active as a loaded start condition, not a separate running service.
Keep the old workers only for the drain below; do not restart them for this check.
In the isolated Linux rehearsal, prove a no-permit start is denied with the unit
already cached before installation. Missing control paths, malformed/dangling
fence files, stale permits and released locks deny.
After final acceptance remove the permit and fence deliberately; retain the
valid control directory while the drop-in remains installed.

For each supervised start, use a **new operator-lock inode**. Never reopen and
relock an old inode with an old permit. With writers stopped and no prior owner,
remove the inspected stale permit, create a fresh private `operator.lock` by
atomic replacement, and hold its exclusive flock in the operator shell. Record
`stat -Lc '%d %i' /proc/$$/fd/9` after `exec 9<>.../operator.lock; flock -n 9`.
Publish `permit.json` by private temporary file + fsync + rename + directory
fsync. Its exact JSON fields are:

```json
{"environment":"production","release_sha":"<approved-40-hex>","database":"verdaxis_prod","lock_device":123,"lock_inode":456}
```

Use observed device/inode integers, never this example. Keep fd 9 open until the
helper exits, then revoke the permit before closing it. The permit authorizes
only a start condition, not DDL. The operator must not hold either normal deploy
lock during the normal helper. A crash releases the operator lock; the durable
fence and stale permit then deny starts. Do not start recovery/timers.

## Drain before the final old-database snapshot

1. Close public mutations and stop external producers. Let active requests,
   oneshots and in-flight provider requests drain naturally. Confirm pending jobs
   are absent. A zero outbox count while a producer is still running is invalid.
2. Allow only the existing outbox sequencer to drain committed events. Wait for
   zero NULL `stream_seq` after producers have finished. Do not add a sequencer.
3. Stop all API workers (which own the sequencer), then inspect sessions and take
   another zero count. Run `--check outbox-empty`. A producer that committed after
   the first zero forces another drain. Do not snapshot until the post-stop check.
4. Freeze the old DB as read-only input **by stopping/isolation of writers**, not
   by ALTER DATABASE, ACL changes, rename or schema edits. Record source hashes,
   outbox count and snapshot identity. Never run new ACL convergence on it.

## Complete tuple switch, not metadata rollback

1. Prepare a clean exact-SHA canonical tree and approved cached dependencies.
   Verify the final `p0_20261016_union` policy from Task 4b. Before creation, prove
   `verdaxis_prod` does not exist; a collision stops the procedure. Bootstrap/copy
   use their own exact approval gates. This checklist grants no import authority.
2. Preserve the complete old tree, venv, environment, installed units/drop-ins,
   runtime/monitor identity, timer state and backup configuration. Record the
   rollback directory and digests. Do not use `git reset`, symlink roots or stamp.
3. Under maintenance and the external fence, rename old `be` to the approved
   rollback directory, then rename the new clean tree into `prod/be`. Build its
   venv at this final path from approved cached packages. Check all shebangs.
   The old venv is usable only after restoring its original directory path.
4. Publish matching runtime and monitor identity before any new code receives
   credentials. Verify `.env` targets `verdaxis_prod`, exact role split, UTC,
   KYC gate off and the approved vault credential source. Compare complete tuple
   digests with `--check tuple`. Repeat exact identity and copy verification.
5. Run the existing deploy helper dry-run, then exact-SHA unit installer dry-run
   and approved installation. Do not remove checkpoint, clean-tree, digest,
   migration, ACL, two-lock, four-worker or readiness gates. Recheck effective
   drop-in composition after installation. No external monitor is auto-installed.
6. In the supervised operator-lock window issue the permit for the exact new
   SHA/database. Run the normal same-revision deploy with all four approvals:
   `APPROVED_RELEASE_SHA`, `MIGRATION_APPROVED_SOURCE_SHA`,
   `MIGRATION_EXPECTED_CURRENT_REVISION=p0_20261016_union`, and
   `MIGRATION_TARGET_REVISION=p0_20261016_union`. Remove the permit after exit.
7. Verify exact four-key readiness, UTC fresh connections, source/data hashes,
   vault recovery and backup restore. Keep public mutations and all unapproved
   jobs closed. Any failure leaves the fence in place; inspect actual state before
   retry. Never treat a journal line as permission to resume automatically.

## Emergency pre-write rollback block

This block is manual and must be rehearsed in Task 10. After any customer write,
stop: preserve both databases and obtain a reconciliation decision. No automatic
post-write return is allowed.

Stop new workers, revoke the permit and inspect all internal writes/outbox rows.
Run the approved reconciliation and `rollback-safe` checks. Preserve new DB and
new tuple as evidence. Restore the **complete** old directory to `prod/be`, its
attested units and old `.env` (`verdaxis`), runtime and monitor identity. Restore
the old backup configuration. Change only the external drop-in's literal database
to `verdaxis`; its SHA still comes from the restored runtime release file.
Do not run the old SHA through the new migration graph or new ACL helper.
Before issuing an old-tuple permit or starting workers, load the restored units:

```bash
sudo systemd-analyze verify /etc/systemd/system/verdaxis-backend.service
sudo systemctl daemon-reload
sudo systemctl show verdaxis-backend.service \
  -p LoadState -p NeedDaemonReload -p DropInPaths -p ExecStartPre
```

Stop unless `LoadState=loaded`, `NeedDaemonReload=no`, the exact fence is in
`DropInPaths`, and `ExecStartPre` contains the external guard with `--database
verdaxis` plus both original guards. Check restored tuple digests again. The
fence must be loaded and active before the restart block; an old file on disk
with the new command cached in the manager is not acceptable.

The reviewed one-use restart block uses the existing ABI, as the service owner:

```bash
# Variables come only from the signed old-tuple manifest. Maintenance stays closed.
set -euo pipefail
umask 077
root=/home/verdaxis-prod/verdaxis/prod/be
guard="$root/.runtime-deploy"
# Inspect owner, mode, single links and absence of symlinks before these opens.
exec 7>>"$guard/production.lock"
flock -n 7
exec 8>>"$guard/production.restart.lock"
flock -n 8
# Both locks are now held. Use a new temporary file, never truncate existing state.
state=$(mktemp "$guard/.production.state.XXXXXXXX")
printf 'DEPLOYMENT_STATE=restart-authorized\nENVIRONMENT=production\nRELEASE_SHA=%s\n' \
  "$OLD_SHA" > "$state"
chmod 600 "$state"
mv -- "$state" "$guard/production.state"
# Root operator separately holds the fresh external lock/permit for OLD_SHA + verdaxis.
sudo systemctl restart verdaxis-backend.service
# Use the old release validator and the exact approved old SHA; no alias/head.
curl --fail --silent --show-error http://127.0.0.1:8000/health/ready \
  | "$root/venv/bin/python" "$root/scripts/validate_health_response.py" \
      --environment production --release-sha "$OLD_SHA"
# Only after exact readiness and old tuple/DB checks: remove the inspected state.
rm -- "$guard/production.state"
# Keep descriptors open through readiness; shell exit releases them.
```

Any failed command retains state. Root revokes the external permit after this
block exits. Keep the fence and maintenance until old-tuple acceptance. The block
must be adapted to the *attested old validator CLI* before approval; no guessed
host command is authority. Do not restore timers except those previously active,
only after deploy state is absent, and only after operator acceptance.

## Append-only operator journal (not a resume engine)

Create the private root-owned journal before the window. Append one line only
after the named manual check passes; fsync after each append. Never rewrite past
lines. Record UTC time, operator identity, operation approval, old/new cluster
ID, endpoint, database OID/name, SHA/revision, env/unit/monitor/backup digests,
rollback directory, timer active/enabled/masked state, outbox counts and table
hash report digests. Example (synthetic):

```text
2026-10-16T12:00:00Z operator=reviewer check=post-stop-outbox old-db=verdaxis old-oid=123 cluster=456 count=0 evidence-sha256=<64hex> decision=pass
```

Before reopening, record the signed lossless rollback boundary. After reopening,
write an explicit `customer-writes-open` entry: pre-write rollback is no longer
available. No tool here interprets the journal or executes its contents.

## Reopen public access and refresh cached pilot tabs

At reopening, the operator checks that the **new frontend entry HTML** is live
at the production frontend URL, with the approved hashed entry asset. This lets
the **existing update banner** appear in old tabs on its next check (on mount,
visibility return or five-minute polling). The founder sends a short **"please refresh"** note to **active pilot users** when public access reopens.

PM accepts the pilot's short old-tab window: an old tab can display EUR 0 for an
unknown ETS cost until the user refreshes. Maintenance and login do not reload
cached JavaScript. Do not delete old assets, add forced reloads, require per-user
confirmation or add a compatibility release. The founder can override this
choice before cutover; revisit it after public launch. Decision source: frontend
`docs/phase0/cached-bundle-options.md`, 2026-10-16 03:22 SGT.

## Current resume point

Resolve the appendix's PM questions, then obtain independent host-tool and
appendix review before Task 10. Recheck bootstrap/copy/verify, exact production
Settings/roles/ACLs, union-to-union deploy and monitor identity at the approved
combined full SHA. Use `new-preinstall.json` for current installed-unit bytes and
`new-final.json` after immutable installation; only the final tuple can authorize
a permit. Do not edit acceptance/oracle documents from this runbook lane.

## Historical note — 2026-10-16, before integration at e61400f3

The original Task 7 lane deferred Task 4b merge and production host-tool authority.
Those source changes are now merged. That earlier deferral is historical, not
the current resume instruction; Linux host approval and rehearsal remain pending.

## Current production tool interface

The production bootstrap parser now uses `--source-environment-file` and
`--target-environment-file`, as do copy and verify. Do not change bootstrap to
`--environment-file` based on the earlier sign-off reproduction: that referred
to the superseded synthetic interface. All six production tool secret files
contain exactly one `DATABASE_URL` line. The historical synthetic bootstrap
`MIGRATOR_DATABASE_URL` file is not valid for this host path. Runtime Settings
still need their separate app and migrator URLs.

Use distinct `bootstrap.json`, `copy.json`, `verify.json` and separate source/
target secret files for each tool, under `/root/verdaxis-phase0`. Each manifest
repeats the fixed approved `authority.json` plus its exact tool/output fields;
copy/verify bind the successful bootstrap receipt digest. Assertion tuple
manifests are a different format. The appendix contains the sole executable
production command sequence and the parser regression test checks it against
the current source. Never reuse one manifest or secret file across tools.

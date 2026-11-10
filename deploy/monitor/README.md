# Verdaxis Local Monitor v2

## Current Rust release boundary

Rust is the sole application backend in this source release. Current service
units run the verified `verdaxis-read` binary and use its projected runtime/job
files. Production Rust listens on loopback 8010; staging listens on 8011.
The demo units invoke `verdaxis-read job demo`, not the retired Python launcher.
Native migration uses the sqlx migrations and `sqlx_2` admission.

The older sections below are historical monitor integration records. They do
not authorize installation, revival of old API ports, or execution of removed
application paths. Retained Python monitor sources and operator compatibility
helpers remain for system-interpreter use where the approved host procedure
calls them. Read the current units, policy and approved live-window runbook
before host work.


This directory is a source-only monitor contract. It does not install, promote,
load, start, enable, disable, restart, or deploy any live unit. The deleted
local installer and pilot-runbook patch are not replaced here. Activation is an
outstanding integration owned by the canonical immutable runtime installer.

## Deployment status

The live environments are split across two hosts:

| Environment or function | Host | Live local endpoint |
| --- | --- | --- |
| Production API and PostgreSQL | `169.58.37.164` (`vmi3623757`) | `127.0.0.1:8000` |
| Staging API, shared Umami, B2 backup owner, external canary | `194.233.68.86` (`vmi1840561.contaboserver.net`) | `127.0.0.1:8001` for staging |

The shared host's production port 8000 is retired, and its production backend
and recovery units are masked. This source tree is not the installed shared-host
monitor. Do not copy it over the installed monitor or activate its units. Use
only the reviewed selective installed patch described by
`/home/verdaxis-prod/verdaxis/PRODUCTION_HOST.md`. Determine current runtime
identity from each public `/health/ready` response, not from a dated source SHA.

## Ownership boundaries

| Responsibility | Owner |
| --- | --- |
| Loopback readiness, filesystem checks, backup verification, sanitized status | Local monitor |
| Telegram/Healthchecks delivery and per-destination receipts | Local alert consumer |
| Application settings, readiness production, database identity, Alembic | Runtime |
| Release checkout, deployment state, identity publication, restart/rollback | Runtime deploy |
| Database dump, B2 upload, local/remote retention, restore drills | Existing backup system |
| Local-versus-remote deletion authority | Existing backup system |
| Public Vercel/Caddy/DNS/TLS/rendered checks | External monitor |
| Event-outbox probe source and attested artifact | Local monitor contract |
| Event-outbox probe invocation, status, and alerting | External monitor |
| Signup and analytics ingestion canaries | Legacy monitor until separately replaced |
| Immutable artifact promotion and activation | Canonical runtime installer/operator |

The deployed databases use PostgreSQL 17 with PostGIS 3.6. Production and
staging are not colocated. The production frontend remains on Vercel.

## Readiness and release identity seam

The source `local_health_check.py` currently contacts both fixed
numeric-loopback endpoints:

- production: http://127.0.0.1:8000/health/ready
- staging: http://127.0.0.1:8001/health/ready

No current host has both live loopback endpoints. This dual-loopback reader is
an unresolved integration seam and must not be installed unchanged. A future
installer must split or route environment checks without reviving the retired
shared-host production listener.

A successful response has exactly four keys and values:

~~~json
{
  "status": "ok",
  "db": "ok",
  "environment": "production",
  "release_sha": "<nonzero lowercase 40-hex Git SHA>"
}
~~~

Missing keys, duplicate keys, extra keys of any type, db=connected, wrong types, redirects,
oversized input, a mismatched environment/SHA, and work beyond the single
monotonic headers-plus-body deadline all fail closed. The shared positive and
negative corpus is runtime-v2-readiness-corpus.json. Runtime integration must
run that same corpus against its readiness producer; this branch does not
modify app/config.py, app/main.py, app/database.py, scripts/deploy.sh, or
scripts/preflight_runtime.py.

The checker reads two dedicated identity files. Each file is a bounded regular
non-symlink with exactly these two keys, one per line and no comments, blanks,
duplicates, or extras:

~~~dotenv
ENVIRONMENT=production
RELEASE_SHA=<current-clean-full-40-hex-sha>
~~~

The runtime deploy owner must atomically publish its own release env and the
matching monitor identity before restart, keep identity aligned with the code
that actually remains checked out after failure, run the environment-bound
database preflight and Alembic, and validate the exact readiness contract. This
branch supplies only the reader contract. The identity files must be owned by
the runtime owner, not writable by monitor identities, and readable by
verdaxis-health without exposing any credential.

## Backup truth and producer seam

backup_verify.py is credential-free and read-only. It validates exact
verdaxis, verdaxis_staging, and umami inventory; couples completed_at, the
timestamp-bearing backup_id, and finalized artifact mtimes within five seconds;
rejects stale/non-newest generations; and streams gzip validation under
compressed, expanded, entry-count, and total-deadline limits. Each gzip must
expand to at least 1 KiB and its bounded prefix must contain a PostgreSQL
plain-dump marker (`PostgreSQL database dump`). A valid but empty,
tiny, or unrelated gzip is not plausible backup evidence and fails closed.

A successful old generation cannot hide a newer failed or crashed attempt.
Activation therefore requires the existing producer owner to maintain
backups/attempts/ with one durable JSON record per attempt:

~~~json
{
  "schema_version": 1,
  "attempt_id": "bounded-unique-id",
  "state": "started",
  "started_at": "2026-07-20T03:00:00Z",
  "finished_at": null,
  "backup_id": null
}
~~~

The producer writes started before dump work. It atomically replaces that
record with either:

- failed, with a bounded UTC finished_at and backup_id=null; or
- succeeded, with finished_at and the exact finalized backup_id.

Records from prior attempts remain available long enough for the verifier to
select the latest started_at. A latest started or failed record is non-green
regardless of older valid artifacts. Recovery requires a newer succeeded record
whose ID and finish time match verified status/artifacts. The checker never
creates, edits, quarantines, or fakes producer attempts.

There is no producer implementation, service, or timer in this tree. The
external backup owner must preserve its complete dump, compression, B2 upload,
local and remote retention, restore verification, failure publication, and
split deletion responsibility graph while adopting the attempt-record seam.

The producer integration must make the backup directory and finalized status,
attempt records, and artifacts traversable/readable by verdaxis-backup-reader
without broadening partial or backup-environment permissions. The monitor
tmpfiles contract deliberately does not mutate the external backup directory,
.backup-env, application .env, or runtime release files.

## Status and alert semantics

Readers atomically publish mode 0600 exact-schema status. Persisted status,
metadata, attempt, alert, and retirement JSON is bounded and typed. Monitor
status requires a nonempty check list and a top-level category equal to the
deterministic worst child category. Unsafe reader state is quarantined and
replaced with a sanitized failure; unsafe producer evidence is read-only and
fails closed. All hostile/external JSON readers reject duplicate object keys.

Readers never load Telegram, Healthchecks, application, database, or backup
credentials. Static OnFailure/OnSuccess events are handled by the separate
verdaxis-monitor-alert user, whose service alone loads alert.env.

Warning behavior is explicit: a reader warning exits nonzero, follows the same
failure dispatch path as a failure, pages each configured destination on the
transition, and repeats no more than hourly while unresolved. The next healthy
run emits recovery. Warning never becomes green.

Alert state schema v3 stores independent receipt IDs and UTC timestamps for
each destination and event state. Each successful channel receipt is persisted
before the next channel attempt, so one channel failure cannot make the other
channel repeat. Both Healthchecks and Telegram use a fixed one-hour
transition/reminder gate; there is no force bypass or configurable interval.
Malformed state is quarantined and cannot suppress paging.

## Native alert handoff (Phase 10, `verdaxis-read monitor alert`)

This section describes the Rust replacement that the Phase 10 unit installer
activates. Until then the Python units above stay authoritative. Its units and
tmpfiles policy are the files in this directory.

**Journal.** Each monitor admits its observation into its own ordered journal
before it publishes its status: `/var/lib/verdaxis-health/alert-handoff/` and
`/var/lib/verdaxis-backup-verify/alert-handoff/`, one immutable
`<sequence>.json` per transition, with the cursor `<check>.sequence.json`, the
stable journal lock `.<check>.journal.lock` and the producer lock
`.<check>.producer.lock` beside it. One producer runs at a time: a manual run
beside the timer exits 4 without publishing. A full journal (256 entries or
1 MiB) or a 20 s journal-lock wait also exits 4 with the status unchanged.

**Delivery.** `--event <check>` is a wakeup; the dispatcher delivers only the
oldest unacknowledged entry. A head gets three attempts, at least 60 s and then
300 s apart; a wakeup before the next attempt is due exits 4 and the unit
restarts a minute later. With no destination configured nothing is
acknowledged (exit 2) and the journal keeps the transition.

**Exit codes and logs.** One JSON line on stderr per run, never a token, chat id
or URL: 0 done, 2 no destination configured, 3 blocked (exhausted, invalid or
reused head), 4 retry later or lock wait, 5 uncertain send. Exits 2, 3 and 5 are
logged at error priority, exit 4 at warning priority. Python printed
`Verdaxis monitor alert: ok|failed` on stdout and used only 0 and 2.

**Failed handoff status.** `/var/lib/verdaxis-monitor-alert-status/<check>.json`
(directory 0755, file 0644) has the monitor status schema with one check,
`alert_handoff`: `healthy`/`ok`, or `failure` with `handoff_exhausted`,
`handoff_uncertain`, `handoff_invalid_entry`, `handoff_sequence_reused` or
`handoff_unconfigured`. It names no sequence, path or destination. The Phase 10
external monitor (Task 29) is to read it together with the failed state of the
alert units (`verdaxis-monitor-alert@health`, `@backup` and the reminder).

**Operator procedures** (run as `verdaxis-monitor-alert`, with
`/usr/local/libexec/verdaxis/verdaxis-read monitor alert`):

1. Find the blocked head: the status code above, then the `head` of
   `/var/lib/verdaxis-monitor-alert/<check>.delivery.json` (sequence, attempts,
   per-destination status).
2. `handoff_uncertain`: check the provider (Telegram chat, Healthchecks log),
   then for each uncertain destination run
   `--reconcile <check>:<sequence>:<healthchecks|telegram>:<delivered|not-delivered>`.
3. `handoff_exhausted`: fix the destination, then `--rearm <check>:<sequence>`
   (fresh attempts, due at once).
4. `handoff_invalid_entry` or `handoff_sequence_reused`: move the bad journal
   name aside as root (the next producer run quarantines links and temp files
   itself), then `--repair <check>:<sequence> --reason "<what was done>"`. The
   repair and its reason are logged; the acknowledged sequence is kept.
5. Revert to Python: stop the native timers and units, drain the journals, then
   `--release <check>`; it refuses while a head or entry remains. The native
   dispatcher keeps the stable `.<check>.lock` at mode 0400, which stops a
   direct Python run until `--release`.

**tmpfiles.** Apply `verdaxis-monitor.conf` and
`verdaxis-monitor-handoff-acl.conf` in one `systemd-tmpfiles --create` call and
check `getfacl` afterwards. The base file creates the producer state
directories with mode 0710, the ACL mask mode, so applying it alone keeps the
dispatcher's traversal entry; the handoff paths are created beside their ACLs.

**Outbox probe.** `verdaxis-read monitor outbox --dsn <dsn>` keeps the Python
probe's stdout JSON and exits (0 ok, 1 breach, 2 error), adds one JSON line on
stderr, and accepts `--query-timeout` from 1 to 20 seconds only. A DSN without
`host` uses `PGHOST` or the local socket, as libpq does.

## Demo activity isolation

Production and staging have independent service/timer pairs. Neither unit
requires, orders, names, schedules, or mutates the other environment. Each
timer is the sole scheduler owner for its matching service.

- production uses verdaxis-demo-production,
  /etc/verdaxis-monitor/demo/production.env, and the production backend;
- staging uses verdaxis-demo-staging,
  /etc/verdaxis-monitor/demo/staging.env, and the staging backend.

Credential files are mode 0600 and owned by the matching non-login user. Each
service loads only its own runtime release env and invokes one launcher that
forces the exact ENVIRONMENT, verifies the expected full SHA against the exact
repository root, archives that commit into a private read-only runtime
snapshot, and executes `scripts/run_demo_activity.py` from that snapshot. Dirty
or concurrently changed checkout bytes therefore cannot become the executed
source after attestation. Git runs with one fixed exact safe.directory; ambient,
global, and system Git configuration are disabled. ProtectHome=tmpfs exposes
only the matching backend as read-only and masks its application .env.

Each service has a negative condition on its own runtime deployment-state file:

~~~text
<backend>/.runtime-deploy/<environment>.state
~~~

The runtime deploy owner must preserve that guard contract. A staging failure
cannot block production, and production cannot schedule or alter staging.

## Static artifact inventory

artifact-manifest.json is data, not an installer. It lists the complete
monitor-owned promotable source set with exact source path, absolute
destination, mode, and SHA-256. It excludes:

- every backup producer implementation and unit;
- application/runtime/deploy files;
- a local root installer or transaction helper; and
- any command that reloads or activates systemd.

The canonical immutable installer may consume this inventory only after it
attests the exact approved clean Git object, privately stages and verifies all
listed bytes and units, applies its own rollback-safe promotion policy, and
receives separate operator authorization. This branch has no standalone
installation or activation procedure.

Sysusers/tmpfiles declarations are idempotent source definitions for
monitor-owned identities, state directories, and per-environment credential
ownership. Merely having those files in source does not create users or change
permissions.

The manifest includes the read-only outbox backlog probe. It has no separate
service or timer. Its source contract pins a local database host and port,
strips ambient libpq routing and password variables, and passes only an
explicit `PGPASSFILE`. Production and staging databases now live on different
hosts, so one shared-host process cannot invoke both probes through loopback.
Source wiring and matching filenames do not prove release. Do not replace or
enable the installed monitor until a reviewed integration separates the two
database locations and the canonical installer promotes the exact
manifest-attested artifacts. This contract installs no prerequisite.

## Dual-run and retirement gate

legacy_retirement.py is retired off-host. It is not in artifact-manifest.json, no unit
or timer runs it, and no host installs it. Its source and the retirement evidence
example stay in this directory for audit only; the reviewed copy is in the off-host
tools record (`tests/rust_contract/phase10/offhost-tools.json`).

legacy_retirement.py defaults to validation only. It derives the required
delivery matrix from the presence of these key names without retaining or
reporting values:

- TELEGRAM_BOT_TOKEN plus TELEGRAM_CHAT_ID;
- HEALTHCHECKS_HEALTH_URL;
- HEALTHCHECKS_BACKUP_URL.

Every configured destination must have matching durable failure and recovery
receipt IDs/timestamps, the durable current state must be healthy, and each
recovery must be strictly newer than that destination's latest failure. Absence
of destination history never proves health. Both health and backup require at
least one destination.
The gate also requires:

- continuous healthy local-health, backup, and legacy samples for an actually
  elapsed 30 hours, with gaps no larger than seven minutes and a recent end;
- fresh, currently healthy local status;
- complete Vercel/Caddy/DNS/TLS/rendered/public-alert and remaining-canary
  external-monitor signoff;
- one scheduler owner per demo environment and proof the legacy monitor stayed
  active throughout observation; and
- every v2 timer plus verdaxis-monitor.timer active immediately before
  retirement.

Execution requires the exact confirmation
RETIRE_LEGACY_AFTER_PROVEN_DUAL_RUN. The command re-reads config, evidence,
receipt state, and local status against newly sampled current UTC, then
revalidates every required timer as its final operation before invoking the one
fixed legacy disable command. A future/stale history, changed config, missing
destination, receipt drift, stale status, inactive timer, or missing external
signoff refuses retirement.

No evidence is supplied by this branch. The legacy monitor must remain active
through dual-run. This source does not authorize its retirement.

## Disposable integration target producer

`tests/disposable_server.py` wraps the normal FastAPI application and produces
`/.well-known/verdaxis-disposable-test` only when `ENVIRONMENT=test` and a
bounded `DISPOSABLE_TEST_TOKEN` are explicit. It binds only numeric loopback and
refuses ports outside the ephemeral range, including 8000/8001. Start it from
this tree with:

~~~bash
ENVIRONMENT=test \
DISPOSABLE_TEST_TOKEN=disposable-test-token-0123456789abcdef \
DATABASE_URL=sqlite+aiosqlite:///:memory: \
JWT_SECRET=test-secret-key-that-is-at-least-32-characters-long \
venv/bin/python -m tests.disposable_server --port 59123
~~~

Then run integration pytest with the same token and
`--disposable-target-url http://127.0.0.1:59123`.

## Safe source verification

Every pytest run declares a disposable numeric-loopback target even though the
unit/monitor suites do not contact it:

~~~bash
env \
  VERDAXIS_TEST_TARGET=disposable \
  TEST_API_URL=http://127.0.0.1:59123 \
  VERDAXIS_DISPOSABLE_TARGET_TOKEN=disposable-test-token-0123456789abcdef \
  ENVIRONMENT=test \
  RELEASE_SHA=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
  JWT_SECRET=disposable-test-jwt-secret-at-least-32-chars \
  DATABASE_URL=sqlite+aiosqlite:///:memory: \
  PYTEST_DISABLE_PLUGIN_AUTOLOAD=1 \
  /usr/bin/python3.12 -m pytest -p pytest_asyncio.plugin \
  tests/monitor tests/unit tests/test_disposable_target.py -q
~~~

Safe static checks include Python compilation, JSON parsing, shell syntax for
remaining shell files, manifest digest comparison, and systemd-analyze verify
against source units. Loaded-unit security scoring is an operator check after a
separately reviewed installation. Do not install or start units to obtain a
score.

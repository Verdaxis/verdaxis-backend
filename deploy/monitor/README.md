# Verdaxis Local Monitor v2

This directory is a source-only monitor contract. It does not install, promote,
load, start, enable, disable, restart, or deploy any live unit. The deleted
local installer and pilot-runbook patch are not replaced here. Activation is an
outstanding integration owned by the canonical immutable runtime installer.

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

The deployed database topology is PostgreSQL 17 with PostGIS 3.6. The
production frontend is on Vercel. Caddy fronts the production API, staging API,
and staging frontend.

## Readiness and release identity seam

local_health_check.py contacts only the fixed numeric-loopback endpoints:

- production: http://127.0.0.1:8000/health/ready
- staging: http://127.0.0.1:8001/health/ready

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

The manifest includes the read-only outbox backlog probe. The canonical public
monitor is its only scheduler: it invokes the installed artifact for production
and staging during its existing five-minute run. The probe has no separate
service or timer. Each query qualifies `public.market_event_outbox`. The caller
pins the local database host and port, strips ambient libpq routing and password
variables, and passes only an explicit `PGPASSFILE` for authentication. Source
wiring alone does not prove that the artifact or its libpq authentication has
been released. The external-monitor installation recipe must stop before
replacing or enabling the monitor until the canonical installer has promoted
the exact manifest-attested probe and an owner-only `PGPASSFILE` covers both
backup roles. This local monitor contract does not install either prerequisite.

## Dual-run and retirement gate

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

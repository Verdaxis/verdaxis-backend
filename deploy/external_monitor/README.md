# Verdaxis External Monitor, Recovery, and Diagnosis

This directory is a source-only monitor, recovery, and diagnosis design. It is
not evidence that these exact bytes are installed. The current external canary
runs on the shared host `194.233.68.86`
(`vmi1840561.contaboserver.net`) and checks both public APIs every five minutes.
Production API and PostgreSQL run separately on `169.58.37.164`
(`vmi3623757`). The shared host's local production port 8000 is retired, and
its production backend and recovery units are masked.

Do not replace `/usr/local/sbin/verdaxis-monitor` or its units with this
directory. Preserve the installed local probes and use only the reviewed
selective installed patch described by
`/home/verdaxis-prod/verdaxis/PRODUCTION_HOST.md`. Source presence and matching
filenames do not prove release or activation. Current runtime identity comes
from each public `/health/ready` response, not from a dated SHA in this file.

## Event outbox backlog

The source design invokes the byte-attested
`/usr/local/libexec/verdaxis-monitor/outbox_backlog_probe.py` once for each
deployed database. Production uses `dbname=verdaxis user=verdaxis_backup` and
staging uses
`dbname=verdaxis_staging user=verdaxis_backup_staging`. Its current code assumes
both targets are at `host=127.0.0.1 port=5432`. That assumption is not valid in
the split deployment: production is on the EU host and staging is on the shared
host. This check must not be activated until a reviewed integration separates
the two locations without adding a general database route. Ambient libpq
routing and password variables must remain stripped, and only an explicit
`PGPASSFILE` may be preserved for authentication.

This source change does not activate the check. Before an operator-approved
release, the canonical immutable installer must promote the matching probe
artifact from `deploy/monitor/artifact-manifest.json` and provide noninteractive
libpq authentication outside command arguments, such as an owner-only
`PGPASSFILE`. Do not place a database password in the monitor command line or
the monitor status.

## Guarded recovery

In this source design, `verdaxis-monitor.service` exits nonzero when a check
remains failed after its built-in retries. systemd then starts
`verdaxis-recover.service`.

Recovery is deterministic and allowlisted. It may:

1. restart one Verdaxis backend when its loopback readiness endpoint is
   unavailable and no deployment transaction is active;
2. reload or restart Caddy through the validated configuration when loopback
   readiness is healthy but a public Verdaxis API route is not; or
3. start the existing Verdaxis backup service when backup freshness or
   completeness is the only recoverable backup condition and disk headroom is
   sufficient.

The same failure fingerprint receives at most one recovery attempt per hour.
Every attempt runs the complete public monitor again. A passing monitor records
and reports recovery; a failed, ineligible, or suppressed attempt exits
nonzero and starts `verdaxis-codex-diagnose.service`.

Recovery never edits source or configuration, checks out Git revisions,
deploys, migrates or restores a database, changes users or credentials,
modifies DNS, invokes Vercel, prunes storage, or executes a command proposed by
Codex. Frontend rollback and storage cleanup remain diagnosis-only until each
has a separately credentialed, independently verified runbook.

## Automatic diagnosis

The diagnosis service:

1. reads the sanitized monitor status;
2. exits without diagnosis if the current monitor status is healthy;
3. fingerprints the failure and suppresses the same fingerprint for one hour;
4. collects the preceding recovery report plus bounded service state, recent
   journals, Git revisions, Verdaxis-scoped Caddy repository status and route
   parsing, and disk state;
5. redacts email addresses, credentials, authorization values, and URL query
   strings;
6. invokes `codex exec` with an ephemeral session, a read-only sandbox, a hard
   timeout, and a strict JSON output schema; and
7. sends the structured diagnosis to the existing Telegram destination.

The shared `/etc/caddy` repository snapshot is limited to the Caddy entrypoint,
Verdaxis route and enabled symlink, and required-host registry. Changes to
unrelated project routes are not Verdaxis incident evidence.

Codex cannot edit files, use `sudo`, restart services, deploy, commit, push, or
mutate production. The systemd unit runs as `jons-openclaw` with
`NoNewPrivileges`, no capabilities, a read-only host/home view outside its
private state directory, hidden Docker/GitHub/Vercel/SSH/application credential
paths, and a dedicated Codex home containing only its authentication state.
Telegram credentials are available to the wrapper but are removed from the
Codex child environment.

Incident and diagnosis JSON is mode `0600` under
`/var/lib/verdaxis-autodiag`. Only the latest 40 of each are retained.

## Independent public monitor

These units define an independent-watchdog design for `144.126.151.136`.
Source presence is not evidence that the design is installed or current. The
authoritative current external canary remains the shared-host
`verdaxis-monitor.timer`. If the independent design receives separate release
evidence, its source-controlled service definition starts
`verdaxis-external-recover.service` when those checks fail. That service may
make one SSH request per hour to start only `verdaxis-monitor.service` on the
Verdaxis host. The next external check confirms whether public service
recovered; the external monitor retains its existing hourly Telegram
deduplication and recovery notification.

The forced key cannot open a shell, forward ports, run arbitrary commands, or
invoke recovery directly. A complete host or network outage still requires the
hosting provider to restore reachability; the watchdog retries when the host
returns.

## Release boundary

This directory has no installation or activation procedure. The installed
shared-host canary and its local probes remain authoritative. Update them only
through the selective process in
`/home/verdaxis-prod/verdaxis/PRODUCTION_HOST.md`.

A future release requires all of the following before separate operator
authorization:

1. a reviewed split-host design for API and database checks;
2. promotion of the exact `outbox_backlog_probe.py` bytes attested by
   `deploy/monitor/artifact-manifest.json` through the canonical immutable
   installer; and
3. an owner-only `PGPASSFILE` that covers the required backup roles without
   exposing credentials in arguments or status output.

Source verification does not satisfy these release conditions. Do not copy
executables or units from this directory, reload systemd, or enable, start, or
restart a monitor from this source tree. Never place application, database,
Vercel, GitHub, or SSH credentials in the diagnosis environment.

## Onboarding attention source contract

This section describes source behavior only. It does not authorize installation
or activation on either current host.

The production-only onboarding monitor checks database state every five minutes
and sends one Telegram alert per user and actionable stage. It reports rejected
onboarding, expiring organization setup, stalled email verification, approval
required, and no first login within two hours of full approval. Demo, test,
canary, and administrator accounts are excluded. Its state file contains only
user identifiers, stages, and timestamps.

The design has a one-time silent baseline: existing stages are suppressed
without later recovery messages, while later accounts and stage changes alert
normally. Any promotion, baseline, or timer activation requires a separate
reviewed release through the canonical installer.

## Diagnosis configuration contract

Luna is the automatic default to keep recurring incident cost bounded. Set
`CODEX_AUTODIAG_MODEL=gpt-5.6-sol` only for a deliberate deeper diagnostic
pass after reviewing the first report.

Before every diagnosis, the wrapper validates the owner-only central
`/home/jons-openclaw/.codex/auth.json` and atomically synchronizes it into the
dedicated Codex home. This prevents a rotated central refresh token from
leaving automatic diagnosis on a revoked credential. Set `CODEX_AUTH_SOURCE`
only when deliberately relocating the central credential. Do not copy Codex
history, sessions, configuration, logs, or plugin state into this directory.

## Independent watchdog source contract

The independent VPS units remain an unreleased design. They do not supersede
the shared-host canary. Any later release must use a root-owned environment and
state, a dedicated restricted Ed25519 key, a pinned host key, and a forced
command limited to starting `verdaxis-monitor.service`. It must not grant a
shell, forwarding, arbitrary commands, or direct recovery. Installation and
activation require a separate reviewed release record.

## Source verification

These checks validate tracked source only. They do not install or activate it.

```bash
/usr/bin/python3 -m py_compile \
  deploy/external_monitor/verdaxis_monitor.py \
  deploy/external_monitor/verdaxis_autodiag.py \
  deploy/external_monitor/verdaxis_recover.py
python3 -m json.tool \
  deploy/external_monitor/diagnosis.schema.json >/dev/null
systemd-analyze verify \
  deploy/external_monitor/systemd/verdaxis-monitor.service \
  deploy/external_monitor/systemd/verdaxis-monitor.timer \
  deploy/external_monitor/systemd/verdaxis-monitor-verify.service \
  deploy/external_monitor/systemd/verdaxis-recover.service \
  deploy/external_monitor/systemd/verdaxis-codex-diagnose.service \
  deploy/external_monitor/systemd/verdaxis-external-monitor.service \
  deploy/external_monitor/systemd/verdaxis-external-monitor.timer \
  deploy/external_monitor/systemd/verdaxis-external-recover.service \
  deploy/external_monitor/systemd/verdaxis-onboarding-attention.service \
  deploy/external_monitor/systemd/verdaxis-onboarding-attention.timer
pytest tests/monitor/test_external_monitor.py \
  tests/monitor/test_outbox_backlog_probe.py \
  tests/monitor/test_external_autodiag.py \
  tests/monitor/test_external_recovery.py -q
```

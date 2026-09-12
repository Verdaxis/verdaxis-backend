# Verdaxis External Monitor, Recovery, and Diagnosis

This directory is the tracked source for the public Verdaxis monitor installed
as `/usr/local/sbin/verdaxis-monitor`. The monitor checks public routes,
rendered frontend behavior, frontend API targets, signup and analytics
canaries, event-delivery progress, backups, storage, and Caddy integrity every
five minutes.

## Event outbox backlog

The monitor invokes the byte-attested
`/usr/local/libexec/verdaxis-monitor/outbox_backlog_probe.py` once for each
deployed database. Production uses `dbname=verdaxis user=verdaxis_backup` and
staging uses
`dbname=verdaxis_staging user=verdaxis_backup_staging`. Both targets pin
`host=127.0.0.1 port=5432`; ambient libpq host, port, service, password, and
options cannot redirect the child. Only an explicit `PGPASSFILE` is preserved
for authentication. These identities match the runtime ACL source and have
read-only table access. A count above 1,000 or an oldest pending age above 300
seconds is a monitor failure. Missing probe bytes, invalid output, timeout, or
database access failure also fails closed. The alert error is a stable category
so changing measurements cannot bypass the hourly incident cooldown. Counts
and ages stay in the structured status detail, and database diagnostics are not
copied into monitor status or alerts.

This source change does not activate the check. Before an operator-approved
release, the canonical immutable installer must promote the matching probe
artifact from `deploy/monitor/artifact-manifest.json` and provide noninteractive
libpq authentication outside command arguments, such as an owner-only
`PGPASSFILE`. Do not place a database password in the monitor command line or
the monitor status.

## Guarded recovery

`verdaxis-monitor.service` exits nonzero when a check remains failed after its
built-in retries. systemd then starts `verdaxis-recover.service`.

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

The existing `verdaxis-external-monitor.timer` runs on the independent VPS at
`144.126.151.136` and checks production and staging public routes every five
minutes. Its source-controlled service definition starts
`verdaxis-external-recover.service` when those checks fail. That service may
make one SSH request per hour to start only `verdaxis-monitor.service` on the
Verdaxis host. The next external check confirms whether public service
recovered; the external monitor retains its existing hourly Telegram
deduplication and recovery notification.

The forced key cannot open a shell, forward ports, run arbitrary commands, or
invoke recovery directly. A complete host or network outage still requires the
hosting provider to restore reachability; the watchdog retries when the host
returns.

## Local installation

Install only from a reviewed, committed source revision. **Stop before this
recipe** unless both prerequisites are complete:

1. The canonical immutable installer has promoted the exact
   `outbox_backlog_probe.py` artifact attested by
   `deploy/monitor/artifact-manifest.json` to
   `/usr/local/libexec/verdaxis-monitor/outbox_backlog_probe.py`.
2. An owner-only pgpass file contains production and staging backup-role
   authentication, and `/etc/verdaxis-monitor.env` sets `PGPASSFILE` to its
   absolute path.

Do not replace the monitor executable or reload, enable, start, or restart its
timer before both prerequisites are verified. Probe promotion belongs to the
canonical installer; this recipe does not copy that artifact.

```bash
sudo install -o root -g root -m 0755 \
  deploy/external_monitor/verdaxis_monitor.py \
  /usr/local/sbin/verdaxis-monitor
sudo install -o root -g root -m 0755 \
  deploy/external_monitor/verdaxis_autodiag.py \
  /usr/local/sbin/verdaxis-codex-diagnose
sudo install -o root -g root -m 0755 \
  deploy/external_monitor/verdaxis_recover.py \
  /usr/local/sbin/verdaxis-recover
sudo install -d -o root -g root -m 0755 \
  /usr/local/share/verdaxis-monitor
sudo install -o root -g root -m 0644 \
  deploy/external_monitor/diagnosis.schema.json \
  /usr/local/share/verdaxis-monitor/diagnosis.schema.json
sudo install -o root -g root -m 0644 \
  deploy/external_monitor/systemd/verdaxis-monitor.service \
  /etc/systemd/system/verdaxis-monitor.service
sudo install -o root -g root -m 0644 \
  deploy/external_monitor/systemd/verdaxis-monitor.timer \
  /etc/systemd/system/verdaxis-monitor.timer
sudo install -o root -g root -m 0644 \
  deploy/external_monitor/systemd/verdaxis-monitor-verify.service \
  /etc/systemd/system/verdaxis-monitor-verify.service
sudo install -o root -g root -m 0644 \
  deploy/external_monitor/systemd/verdaxis-codex-diagnose.service \
  /etc/systemd/system/verdaxis-codex-diagnose.service
sudo install -o root -g root -m 0644 \
  deploy/external_monitor/systemd/verdaxis-recover.service \
  /etc/systemd/system/verdaxis-recover.service
sudo install -d -o root -g jons-openclaw -m 0750 \
  /var/lib/verdaxis-monitor
sudo install -d -o jons-openclaw -g jons-openclaw -m 0700 \
  /var/lib/verdaxis-autodiag/codex-home
sudo install -o jons-openclaw -g jons-openclaw -m 0600 \
  /home/jons-openclaw/.codex/auth.json \
  /var/lib/verdaxis-autodiag/codex-home/auth.json
```

This recipe deliberately stops before systemd activation. A separately
authorized operator may reload and enable the existing timer only after the
installed probe digest matches the manifest, the owner and mode of its pgpass
file are verified without printing the file, and the source verification checks
below pass.

The first four settings in `/etc/verdaxis-monitor.env` are optional.
`PGPASSFILE` is required before monitor replacement or activation:

```dotenv
CODEX_AUTODIAG_MODEL=gpt-5.6-luna
CODEX_AUTODIAG_TIMEOUT_SECONDS=600
CODEX_AUTODIAG_COOLDOWN_SECONDS=3600
VERDAXIS_RECOVERY_COOLDOWN_SECONDS=3600
PGPASSFILE=/etc/verdaxis-monitor/pgpass
```

`TELEGRAM_BOT_TOKEN` and `TELEGRAM_CHAT_ID` are shared with the existing
monitor alert path. Never put application, database, Vercel, GitHub, or SSH
credentials in the diagnosis environment.

## Onboarding attention monitor

The production-only onboarding monitor checks database state every five minutes
and sends one Telegram alert per user and actionable stage. It reports rejected
onboarding, expiring organization setup, stalled email verification, approval
required, and no first login within two hours of full approval. Demo, test,
canary, and administrator accounts are excluded. Its state file contains only
user identifiers, stages, and timestamps.

Install the units, inspect current candidates without sending, then silently
baseline historical cases before enabling the timer:

```bash
sudo install -o root -g root -m 0644 \
  deploy/external_monitor/systemd/verdaxis-onboarding-attention.service \
  /etc/systemd/system/verdaxis-onboarding-attention.service
sudo install -o root -g root -m 0644 \
  deploy/external_monitor/systemd/verdaxis-onboarding-attention.timer \
  /etc/systemd/system/verdaxis-onboarding-attention.timer
sudo install -d -o verdaxis-prod -g verdaxis-prod -m 0700 \
  /var/lib/verdaxis-onboarding-attention
sudo systemctl daemon-reload
sudo -u verdaxis-prod ./venv/bin/python -m app.cli.onboarding_attention \
  --dry-run
sudo -u verdaxis-prod ./venv/bin/python -m app.cli.onboarding_attention \
  --bootstrap
sudo systemctl enable --now verdaxis-onboarding-attention.timer
sudo systemctl start verdaxis-onboarding-attention.service
```

Bootstrap is a one-time activation step: existing stages are suppressed without
later recovery messages. New accounts and stage changes alert normally. Review
runs with `journalctl -u verdaxis-onboarding-attention.service`.

Luna is the automatic default to keep recurring incident cost bounded. Set
`CODEX_AUTODIAG_MODEL=gpt-5.6-sol` only for a deliberate deeper diagnostic
pass after reviewing the first report.

Before every diagnosis, the wrapper validates the owner-only central
`/home/jons-openclaw/.codex/auth.json` and atomically synchronizes it into the
dedicated Codex home. This prevents a rotated central refresh token from
leaving automatic diagnosis on a revoked credential. Set `CODEX_AUTH_SOURCE`
only when deliberately relocating the central credential. Do not copy Codex
history, sessions, configuration, logs, or plugin state into this directory.

## Independent monitor installation

The independent VPS already owns
`/usr/local/libexec/verdaxis-external-monitor`, its root-owned environment, and
its state under `/var/lib/verdaxis-external-monitor`. Install the three tracked
systemd units without adding a second monitor or timer:

```bash
sudo install -o root -g root -m 0644 \
  deploy/external_monitor/systemd/verdaxis-external-monitor.service \
  /etc/systemd/system/verdaxis-external-monitor.service
sudo install -o root -g root -m 0644 \
  deploy/external_monitor/systemd/verdaxis-external-monitor.timer \
  /etc/systemd/system/verdaxis-external-monitor.timer
sudo install -o root -g root -m 0644 \
  deploy/external_monitor/systemd/verdaxis-external-recover.service \
  /etc/systemd/system/verdaxis-external-recover.service
sudo systemctl daemon-reload
sudo systemctl enable --now verdaxis-external-monitor.timer
```

Create one dedicated Ed25519 key under
`/etc/verdaxis-external-monitor/recovery_ed25519`, readable only by root and
the `verdaxis-monitor` group. Pin the verified Verdaxis host key in
`/etc/verdaxis-external-monitor/known_hosts`; do not use `accept-new`.

The matching public key on the Verdaxis host must use this exact restriction:

```text
from="144.126.151.136",restrict,command="/usr/bin/sudo -n /bin/systemctl start verdaxis-monitor.service" ssh-ed25519 ...
```

The existing `jons-openclaw` sudo policy permits that exact forced command; do
not install an unrestricted watchdog key.

## Verification

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

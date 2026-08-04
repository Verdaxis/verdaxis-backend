# Verdaxis External Monitor, Recovery, and Diagnosis

This directory is the tracked source for the public Verdaxis monitor installed
as `/usr/local/sbin/verdaxis-monitor`. The monitor checks public routes,
rendered frontend behavior, frontend API targets, signup and analytics
canaries, backups, storage, and Caddy integrity every five minutes.

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
2. fingerprints the failure and suppresses the same fingerprint for one hour;
3. collects the preceding recovery report plus bounded service state, recent
   journals, Git revisions, Verdaxis Caddy route parsing, and disk state;
4. redacts email addresses, credentials, authorization values, and URL query
   strings;
5. invokes `codex exec` with an ephemeral session, a read-only sandbox, a hard
   timeout, and a strict JSON output schema; and
6. sends the structured diagnosis to the existing Telegram destination.

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

Install only from a reviewed, committed source revision:

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
sudo systemctl daemon-reload
sudo systemctl enable --now verdaxis-monitor.timer
```

Optional settings in `/etc/verdaxis-monitor.env`:

```dotenv
CODEX_AUTODIAG_MODEL=gpt-5.6-luna
CODEX_AUTODIAG_TIMEOUT_SECONDS=600
CODEX_AUTODIAG_COOLDOWN_SECONDS=3600
VERDAXIS_RECOVERY_COOLDOWN_SECONDS=3600
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

If the interactive Codex login is replaced or revoked, refresh the dedicated
copy of `auth.json` with the two `install` commands above. Do not copy Codex
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
pytest tests/monitor/test_external_autodiag.py \
  tests/monitor/test_external_recovery.py -q
```

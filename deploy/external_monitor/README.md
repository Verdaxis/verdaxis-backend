# Verdaxis External Monitor and Automatic Diagnosis

This directory is the tracked source for the public Verdaxis monitor installed
as `/usr/local/sbin/verdaxis-monitor`. The monitor checks public routes,
rendered frontend behavior, frontend API targets, signup and analytics
canaries, backups, storage, and Caddy integrity every five minutes.

## Automatic diagnosis

`verdaxis-monitor.service` exits nonzero when a check remains failed after its
built-in retries. systemd then starts `verdaxis-codex-diagnose.service`.

The diagnosis service:

1. reads the sanitized monitor status;
2. fingerprints the failure and suppresses the same fingerprint for one hour;
3. collects bounded service state, recent journals, Git revisions, Verdaxis
   Caddy route parsing, and disk state;
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

## Installation

Install only from a reviewed, committed source revision:

```bash
sudo install -o root -g root -m 0755 \
  deploy/external_monitor/verdaxis_monitor.py \
  /usr/local/sbin/verdaxis-monitor
sudo install -o root -g root -m 0755 \
  deploy/external_monitor/verdaxis_autodiag.py \
  /usr/local/sbin/verdaxis-codex-diagnose
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
  deploy/external_monitor/systemd/verdaxis-codex-diagnose.service \
  /etc/systemd/system/verdaxis-codex-diagnose.service
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
```

`TELEGRAM_BOT_TOKEN` and `TELEGRAM_CHAT_ID` are shared with the existing
monitor alert path. Never put application, database, Vercel, GitHub, or SSH
credentials in the diagnosis environment.

Luna is the automatic default to keep recurring incident cost bounded. Set
`CODEX_AUTODIAG_MODEL=gpt-5.6-sol` only for a deliberate deeper diagnostic
pass after reviewing the first report.

If the interactive Codex login is replaced or revoked, refresh the dedicated
copy of `auth.json` with the two `install` commands above. Do not copy Codex
history, sessions, configuration, logs, or plugin state into this directory.

## Verification

```bash
/usr/bin/python3 -m py_compile \
  deploy/external_monitor/verdaxis_monitor.py \
  deploy/external_monitor/verdaxis_autodiag.py
python3 -m json.tool \
  deploy/external_monitor/diagnosis.schema.json >/dev/null
systemd-analyze verify \
  deploy/external_monitor/systemd/verdaxis-monitor.service \
  deploy/external_monitor/systemd/verdaxis-monitor.timer \
  deploy/external_monitor/systemd/verdaxis-codex-diagnose.service
pytest tests/monitor/test_external_autodiag.py -q
```

The monitor checks public URLs but runs on this VPS. If the entire VPS or its
network is unavailable, it cannot invoke Codex; that failure class still needs
an independently hosted monitor and recovery trigger.

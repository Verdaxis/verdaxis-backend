# Shared-host external monitor: selective Rust install

This procedure is for the PM to run on the shared staging host after review.
No host was accessed to prepare it. Do not install the full monitor directory or
run the general backend installer for this change. No restart of Caddy,
PostgreSQL, backends or unrelated software is needed.

## Scope and prerequisites

- Linux x86_64, the release's supported glibc and runtime libraries, GNU coreutils,
  Bash, flock, systemd and authenticated `gh` for artifact download.
- Use the reviewed PR checkout for units and `install.sh`. The binary is the CI
  release artifact, not a local build. Record its run ID, full commit and SHA256
  before privileged installation; do not use a moving branch or latest release.
- Keep `/etc/verdaxis-monitor.env` and all monitor state unchanged. Never print
  its contents. Confirm the environment names locally without exposing values.
- Confirm `verdaxis-recover.service` is masked. This procedure never unmasks it.
- Existing Caddy, ChromeDriver, Chromium and render user must remain available.
  Rust rendering needs `RENDER_USER` when run as root. If the installed env does
  not name it, stop and obtain operator approval for a separate env change.
- Both services may take up to 30 minutes. Stop the timer and allow an active run
  to finish; do not kill it or run monitor and verify concurrently.

## Pinned artifact

The exact download commands and binary digest will be recorded here after the
selected staging CI run completes. Installation must not start before that record
is complete and all checks for the selected run pass.

## Install

Run from the reviewed checkout. `BINARY` is the downloaded release binary and
`SHA256` is the reviewed digest in the pinned record, not a hash inferred from
an untrusted local file. Keep the printed rollback key.

```bash
systemctl is-enabled verdaxis-monitor.timer
systemctl is-active verdaxis-monitor.timer
systemctl is-enabled verdaxis-recover.service
sudo systemctl stop verdaxis-monitor.timer
systemctl show verdaxis-monitor.service verdaxis-monitor-verify.service \
  --property=Id,ActiveState,SubState
# Wait for both services to be inactive or failed before continuing.
sudo bash deploy/external_monitor/install.sh install "$BINARY" "$SHA256"
```

The installer validates SHA256 before replacement. It backs up existing files as
`FILE.bak.TIMESTAMP`, records absent destinations under
`/var/lib/verdaxis-monitor-install/TIMESTAMP`, atomically replaces the binary and
two units, then runs `systemctl daemon-reload`. Repeating the same install does
not replace unchanged files. A partial install leaves the timer stopped and
prints a rollback key before any replacement. Backups remain until the operator
has accepted the release. The installer never changes timer enablement or starts
services. It does not overwrite `/usr/local/sbin/verdaxis-monitor`.

## Verify and activate

```bash
printf '%s  %s\n' "$SHA256" /usr/local/libexec/verdaxis/verdaxis-read-monitor \
  | sudo sha256sum --check --status
sudo systemd-analyze verify \
  /etc/systemd/system/verdaxis-monitor.service \
  /etc/systemd/system/verdaxis-monitor-verify.service
systemctl show verdaxis-monitor.service verdaxis-monitor-verify.service \
  --property=Id,ExecStart,EnvironmentFiles
sudo systemctl start verdaxis-monitor-verify.service
systemctl show verdaxis-monitor-verify.service \
  --property=Result,ExecMainStatus,ActiveState
# Continue only for Result=success and ExecMainStatus=0.
sudo systemctl start verdaxis-monitor.service
systemctl show verdaxis-monitor.service --property=Result,ExecMainStatus,ActiveState
systemctl is-enabled verdaxis-recover.service
# Continue only for success and a still-masked recovery unit.
sudo systemctl start verdaxis-monitor.timer
systemctl list-timers --all verdaxis-monitor.timer
```

Verification suppresses Telegram only. Existing Healthchecks pings, canaries,
state and status writes still occur. Do not clear state to force checks. Cached
expensive checks retain their existing interval. A canary cleanup failure is a
separate backend release issue handled by PR #170; do not hide it in this change.
If verification fails, leave the timer stopped and use rollback. Inspect logs on
the host under the operator's secret-handling rules; do not paste raw logs or env.

## Rollback

Use the printed timestamp, even after a partial install. Stop the timer and wait
for active runs to finish before restoring files. Rollback is repeatable and
restores original modes/ownership. Files absent before installation are removed.
It leaves env, monitor state, Python helpers and the recovery mask untouched.

```bash
sudo systemctl stop verdaxis-monitor.timer
systemctl show verdaxis-monitor.service verdaxis-monitor-verify.service \
  --property=Id,ActiveState,SubState
# Wait for inactive/failed, then set the key printed by install:
ROLLBACK_KEY='TIMESTAMP_FROM_INSTALL'
sudo bash deploy/external_monitor/install.sh rollback "$ROLLBACK_KEY"
systemctl show verdaxis-monitor.service verdaxis-monitor-verify.service \
  --property=Id,ExecStart
systemctl is-enabled verdaxis-recover.service
sudo systemctl start verdaxis-monitor-verify.service
systemctl show verdaxis-monitor-verify.service --property=Result,ExecMainStatus
# Restore the original active timer only after accepting the restored check.
sudo systemctl start verdaxis-monitor.timer
systemctl list-timers --all verdaxis-monitor.timer
```

## Environment mapping (names only)

No renames for these Python settings read by Rust:

`STATE_FILE`, `STATUS_FILE`, `CADDYFILE`, `MIN_CADDYFILE_BYTES`,
`HTTP_TIMEOUT_SECONDS`, `HTTP_RETRIES`, `HTTP_RETRY_DELAY_SECONDS`,
`FRONTEND_BUNDLE_CHECKS_ENABLED`, `BACKUP_STATUS_FILE`, `BACKUP_MAX_AGE_SECONDS`,
`CHROMEDRIVER_BIN`, `CHROMIUM_PROFILE_PARENT`, `LANG`, `LC_ALL`,
`RENDERED_PAGE_CHECKS_ENABLED`, `RENDER_TIMEOUT_SECONDS`, `RENDER_RETRIES`,
`RENDER_RETRY_DELAY_SECONDS`, `ANALYTICS_DATA_DIR`, `ANALYTICS_MAX_DATA_BYTES`,
`MIN_DISK_FREE_PERCENT`, `SIGNUP_CANARY_TARGETS`, `MONITOR_TOKEN`,
`SIGNUP_CANARY_ENABLED`, `DEMO_TRADE_CANARY_ENABLED`,
`EXPENSIVE_CHECK_INTERVAL_SECONDS`, `ALERT_COOLDOWN_SECONDS`, `TELEGRAM_DISABLED`,
`TELEGRAM_BOT_TOKEN`, `TELEGRAM_CHAT_ID`, `HEALTHCHECKS_PING_URL`.

Rust also reads `HOME`, `RENDER_USER`, `TELEGRAM_API_BASE`. `HOME` supplies the
render profile location; `RENDER_USER` is required for root rendering.
`ANALYTICS_CANARY_ENABLED` and `ANALYTICS_WEBSITE_ID` are no longer read: the
Umami ingestion canary was already retired in Rust. `PGPASSFILE` is not read by
the external monitor: the source-only Python database/outbox probe is not part of
the installed Rust monitor. Rust reads local monitor unit states instead and
opens no database connection. Existing Python recovery/diagnosis settings remain
unchanged. This comparison uses tracked source, not undisclosed host env values.

## What stays Python

`verdaxis_autodiag.py` and `verdaxis_recover.py`, their unit files, schema and
tests remain. There is no Rust diagnosis command. The parsed Rust `recover`
command returns `not_available`, so it is not a replacement. Recovery remains
masked by operator choice. The old installed Python monitor executable stays as
an inactive rollback asset; the two active units no longer call it. Nothing in
`deploy/monitor/` or other host projects is removed by this procedure.

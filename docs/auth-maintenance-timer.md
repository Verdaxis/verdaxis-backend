# Authentication maintenance timer contract

Authentication cleanup is independent from news ingestion and web workers.
Run one bounded transaction with:

```text
./venv/bin/python -m app.cli.auth_maintenance --batch-size 1000
```

Each category removes/clears at most the requested batch size (1-10,000):

- expired refresh sessions;
- fully revoked refresh families older than 24 hours, while retaining revoked
  predecessors when a usable successor still exists for replay detection;
- expired or consumed pending registrations and their password hashes/PII;
- expired/malformed password-reset and email-verification hashes; and
- expired legacy plaintext email tokens while the identity compatibility
  revision still exists.

The command prints counts only. It never selects or logs token values, password
hashes, email addresses, provider keys, or database exception text.

## Installation and runbook

Copy the matching files from `deploy/systemd/` to `/etc/systemd/system/`, then:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now verdaxis-auth-maintenance.timer
sudo systemctl start verdaxis-auth-maintenance.service
sudo systemctl status verdaxis-auth-maintenance.timer
sudo journalctl -u verdaxis-auth-maintenance.service --since today
```

Use the `-staging` unit names for staging. Verify the unit's `WorkingDirectory`,
`EnvironmentFile`, interpreter, and database all refer to the same environment.
Do not enable production and staging units against one `.env`. The timer is
hourly with a randomized delay; repeated bounded runs drain a backlog safely.

Before installation, validate all committed units:

```bash
systemd-analyze verify deploy/systemd/verdaxis-auth-maintenance*.service \
  deploy/systemd/verdaxis-auth-maintenance*.timer
```

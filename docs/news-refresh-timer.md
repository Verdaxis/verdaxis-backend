# News refresh timer contract

Web workers expose read-only news routes and never run a scheduler. The public
`POST /api/news/refresh` mutation endpoint is removed. One refresh is executed
only through the process entrypoint:

```text
./venv/bin/python -m app.cli.refresh_news
```

Production and staging each have exactly one source-controlled systemd timer
and matching `Type=oneshot` service in `deploy/systemd/`. A PostgreSQL
transaction advisory lock makes concurrent timer or CLI invocations mutually
exclusive across processes; overlap exits successfully without duplicate work.

## Operator-held installation

Install and enable each environment independently because their approved SHAs
may differ. First verify the exact release from its matching clean checkout:

```bash
./scripts/install_systemd_units.sh --dry-run \
  --environment production --source-ref <approved-production-sha>
./scripts/install_systemd_units.sh --dry-run \
  --environment staging --source-ref <approved-staging-sha>
```

After approval, repeat each command with `--apply`. That copies only the
release-attested unit bytes and reloads systemd when files changed; it does not
enable or start anything. Enabling the matching timer is a separate live action:

```bash
sudo systemctl enable --now verdaxis-news-refresh.timer
sudo systemctl enable --now verdaxis-news-refresh-staging.timer
```

Never enable both environment timers against the same checkout or `.env`.
Confirm backend workers retain no scheduler and the removed public refresh
route remains absent.

# News refresh timer contract

Web workers expose read-only news routes and never run a scheduler. The public
`POST /api/news/refresh` mutation endpoint is removed. One refresh is executed
only through the process entrypoint:

```text
./venv/bin/python -m app.cli.refresh_news
```

Production uses the licensed Webz.io News API as the primary open-web source
when `WEBZ_API_TOKEN` is configured. The request is one bounded page with no
pagination or automatic retry. Missing credentials, provider failure, invalid
responses, and zero usable records fall back to the source-controlled maritime
RSS feeds. Staging normally leaves `WEBZ_API_TOKEN` unset and therefore uses
RSS; supply the token only transiently for an explicit integration check.

Both environment timers run at 00:00, 06:00, 12:00, and 18:00 in the server's
Asia/Singapore timezone. With the token configured only in production, this
caps scheduled Webz usage at 120 to 124 calls per month. Manual service starts
consume an additional call. Keep the token only in the protected backend
`.env`; it must never appear in source, logs, database rows, public API
responses, or frontend configuration.

Webz.io is a news-discovery source only. Articles never enter the trusted
market-signal ingestion path and cannot create benchmarks, indications,
fair-price bands, physical stems, orders, or trades.

Production and staging each have exactly one source-controlled systemd timer
and matching `Type=oneshot` service in `deploy/systemd/`. A PostgreSQL
transaction advisory lock makes concurrent timer or CLI invocations mutually
exclusive across processes; overlap exits successfully without duplicate work.
Each service checks its environment's durable `.runtime-deploy/<environment>.state`
file and refuses blocked/readiness-pending starts, so a timer cannot execute
newly selected source before matching release identity has been published and
readiness has passed.

## Operator-held installation

Install and enable each environment independently because their approved SHAs
may differ. First verify the exact release from its matching clean checkout:

```bash
./scripts/install_systemd_units.sh --dry-run \
  --environment production --source-ref <approved-production-sha>
./scripts/install_systemd_units.sh --dry-run \
  --environment staging --source-ref <approved-staging-sha>
```

After approval, repeat each command with `--apply`. The current environment
manifest also names the backend and product-analytics prune units, for five
files total. The exact manifest and every named file are materialized from the
approved commit into private root-owned staging and digest-checked before
installation. The installer persists pending state,
always reloads systemd, and clears pending state only after a successful reload;
it does not enable or start anything. Enabling the matching news
timer is a separate live action:

```bash
sudo systemctl enable --now verdaxis-news-refresh.timer
sudo systemctl enable --now verdaxis-news-refresh-staging.timer
```

Never enable both environment timers against the same checkout or `.env`.
Confirm backend workers retain no scheduler and the removed public refresh
route remains absent.

In the eventual combined security release, retain these singleton news owners
and add the matching environment's audited auth-maintenance service/timer pair
to `deploy/systemd/runtime-units.manifest`. The manifest-driven installer needs
no code change and continues to stage only immutable committed bytes. The
runtime-only five-unit manifest is not a claim that the isolated branch is a
complete runtime+security deployment.

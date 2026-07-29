# Demo Activity and Canary Ownership

Verdaxis uses separate synthetic systems during the demo phase:

1. **Signup canary** — private, mutating operational monitoring that checks the public signup flow remains healthy.
2. **Analytics ingestion canary** — private, mutating monitoring of the analytics collector and cleanup path.
3. **Demo market activity** — disclosed sample market activity that helps users understand how the platform behaves when live liquidity exists.

## Signup canary

The signup canary is not user-facing market activity. It remains on the legacy
root `verdaxis-monitor.service`, including its current alert/cooldown path,
until an equivalent external monitor has been independently reviewed. The new
local health and backup readers do not implement signup or analytics canaries
and do not receive monitor tokens or alert credentials. Their sanitized
outcomes page through a separate least-privilege alert consumer.

The canary checks:

- `POST /api/auth/register`
- `POST /api/auth/register-with-org`
- backend persistence of a new pending user and organization

Canary users and organizations must use obvious synthetic identifiers and must not be counted as real market participants.
Canary requests include the private monitor token and use `canary+...@*.canary.verdaxis.exchange`
addresses so the backend can exercise signup persistence without sending Resend verification emails.
Because these synthetic records are deleted immediately, they do not create
organization-join or immutable audit rows; real registrations always create
both before commit.
Real user signups must always continue through the normal verification-email path.

The same migration rule applies to the analytics ingestion canary: it remains
on the legacy unit until an external, independently reviewed replacement checks
ingestion and safe cleanup. Never add either mutating canary to
`local_health_check.py`.

The production frontend is hosted on Vercel; Caddy fronts production/staging
APIs and the staging frontend. Their public DNS, TLS, and rendered-page checks
belong to the external monitor. Local disk/backup paging belongs to the separate
local alert consumer. Caddy validation belongs to deployment preflight and
`caddy-safe-reload`. During migration, dual-run for at least 30 hours and do not
disable the root timer until local alert delivery, the public monitor, and every
remaining legacy responsibility have signed-off replacement coverage. Rollback
restores and verifies the root timer first. See `deploy/monitor/README.md`.

## Demo market activity

Demo market activity is generated only from system-owned demo organizations. It must remain visibly labelled as demo/preview liquidity in user-facing surfaces.

The source-controlled production and staging demo services have independent
timers. Each timer is the sole scheduler owner for its matching environment;
neither service or timer names, orders, requires, schedules, or mutates the
other. A staging failure therefore cannot block production, and a production
run cannot reach staging.

Production runs as the non-login `verdaxis-demo-production` user and loads only
`/etc/verdaxis-monitor/demo/production.env`; staging runs as the distinct
`verdaxis-demo-staging` user and loads only its staging counterpart. Each file
is mode `0600`, owned by that environment's user, and contains only the minimum
environment-scoped application credentials needed for demo activity. Neither
user belongs to the backup-reader group, and each unit makes the other
environment's credential/release files plus all application and backup env
files inaccessible.

Each service also loads its own deploy-managed `.runtime-release.env` and uses
one launcher that forces the exact `ENVIRONMENT`, verifies the full nonzero
40-hex `RELEASE_SHA` at the exact repository root, archives that commit into a
private read-only runtime snapshot, and executes demo activity from the
snapshot. Mutable or dirty checkout bytes cannot be attested and then executed.
Git uses one fixed `safe.directory` while disabling ambient, global, and system
configuration, so the distinct service users do not hit dubious-ownership
errors and cannot attest a parent or nested checkout. Neither defaults to
development or uses nested `sudo`. Each service also refuses to
start while its own deploy-state guard exists; the guard path is scoped to that
same backend and environment. Sysusers/tmpfiles describe the distinct
identities and credential modes, while `artifact-manifest.json` inventories
their exact source bytes. Those declarations have no effect until a separately
reviewed canonical immutable installer consumes them; this branch provides no
standalone install or activation path.

Rules:

- Demo orders and trades are owned by deterministic demo organizations.
- Demo listings are blocked from real user trade execution.
- Demo orders do not auto-match with real orders.
- Demo trade-tape entries are labelled as demo.
- Activity is limited to the approved products, ports, and availability windows.
- Old generated demo activity is pruned so the market looks current without growing indefinitely.

The system must never use synthetic signups to imply real user growth. Signup canaries are monitoring data only; demo trades are sample market data only.

# Verdaxis Exchange — Backend API

Maritime fuel trading exchange platform backend.

## Local development

Run development servers only from an isolated worktree or clone. Do not use
the canonical production or staging checkout, and do not bind the deployed
ports 8000 or 8001. Choose an unused loopback port from the ephemeral range:

```bash
cd /path/to/isolated/verdaxis-backend-worktree
source venv/bin/activate
ENVIRONMENT=development \
  uvicorn app.main:app --host 127.0.0.1 --port <unused-ephemeral-port>
```

## Live Deployment

The deployed backends run on separate hosts through systemd, not Docker
Compose:

| Environment | Host | Canonical checkout | Loopback listener |
| --- | --- | --- | --- |
| Production | `169.58.37.164` (`vmi3623757`) | `/home/verdaxis-prod/verdaxis/prod/be` | `127.0.0.1:8000` |
| Staging | `194.233.68.86` (`vmi1840561.contaboserver.net`) | `/home/verdaxis-prod/verdaxis/staging/be` | `127.0.0.1:8001` |

The shared host retains old production rollback material, but its local port
8000 is retired and its production backend and recovery units are masked. Do
not deploy production there or unmask those units. On the shared host, read
`/home/verdaxis-prod/verdaxis/PRODUCTION_HOST.md` before deployment or
recovery work. Treat dated release records as evidence only; read the current
full release SHA from the target environment's `/health/ready` response.

```bash
# Staging: run on 194.233.68.86 only.
test "$(/usr/bin/hostname -f)" = vmi1840561.contaboserver.net && (
  set -e
  sudo -n -u verdaxis-prod -H bash -lc \
    'cd /home/verdaxis-prod/verdaxis/staging/be && ./scripts/deploy.sh --dry-run'
  sudo -n -u verdaxis-prod -H env \
    APPROVED_RELEASE_SHA=<sha> \
    MIGRATION_APPROVED_SOURCE_SHA=<same-sha> \
    MIGRATION_EXPECTED_CURRENT_REVISION=<exact-current> \
    MIGRATION_TARGET_REVISION=<allowlisted-target> \
    /home/verdaxis-prod/verdaxis/staging/be/scripts/deploy.sh
)
```

Open a separate administrator session for production:

```bash
ssh verdaxis-admin@169.58.37.164
```

Run the complete production sequence only at the EU-host prompt. A failed host
test prevents both deploy commands:

```bash
test "$(/usr/bin/hostname)" = vmi3623757 && (
  set -e
  sudo -n -u verdaxis-prod -H bash -lc \
    'cd /home/verdaxis-prod/verdaxis/prod/be && ./scripts/deploy.sh --dry-run'
  sudo -n -u verdaxis-prod -H env \
    APPROVED_RELEASE_SHA=<sha> \
    MIGRATION_APPROVED_SOURCE_SHA=<same-sha> \
    MIGRATION_EXPECTED_CURRENT_REVISION=<exact-current> \
    MIGRATION_TARGET_REVISION=<allowlisted-target> \
    /home/verdaxis-prod/verdaxis/prod/be/scripts/deploy.sh
)
```

### Deployment contract

[scripts/deploy.sh](scripts/deploy.sh) fixes the branch, service, and guard path
from the canonical checkout and refuses a dirty tree. It also removes ambient
Git, database, and Python routing controls.

- Dry-run resolves one full SHA and verifies only that archived commit. It
  checks the [unit manifest](deploy/systemd/runtime-units.manifest),
  [migration checkpoints](deploy/migration-checkpoints.tsv), ACL bundle, and
  systemd bytes without executing candidate Python or opening the live database.
- A real deploy requires the same approved SHA, an exact current revision, and
  one literal allowlisted target. It refuses `head`, moved source, unexpected
  live state, and unlisted transitions before source mutation.
- Migration uses explicit application and migrator URLs with distinct roles.
  The archived [ACL helper](scripts/converge_runtime_acls.py),
  [policy](deploy/postgres/app_acl_policy.sql), and
  [convergence SQL](deploy/postgres/converge_runtime_object_acls.sql) rebuild
  runtime ACLs transactionally before restart.
- A per-environment lock and `.runtime-deploy/<environment>.state` remain in
  force through restart and readiness. The backend, news-refresh, and
  product-analytics-prune units accept only absent or `restart-authorized`
  state. The order-expiry-reminder and auth-maintenance units refuse any
  present state. Auth maintenance also loads `.runtime-release.env` and
  preflights the effective `ENVIRONMENT` before execution. These systemd
  conditions govern new starts only; creating state does not stop or drain an
  already-running oneshot.
- `/health/ready` must return exact status, database state, environment, and
  full SHA before the deploy state clears. `/health/live` proves only process
  response; `/health` remains a readiness alias.
- Dependency installation uses [requirements.txt](requirements.txt) and
  [constraints.txt](constraints.txt); missing constraints or an unsafe
  environment file fails closed.

### Unit-install contract

[scripts/install_systemd_units.sh](scripts/install_systemd_units.sh) performs
the no-change check with `--dry-run --environment <production|staging>
--source-ref <approved-40-hex-sha>`.

- It attests the fixed checkout, archives the committed allowlist and unit
  bytes, verifies their digest manifest, and runs `systemd-analyze verify` only
  on private staged bytes. It executes no checkout Python, preflight, Alembic,
  or candidate backend.
- `--apply` uses an environment lock and root-owned pending state. It prepares
  all replacements first, rejects unsafe destinations, rolls back partial
  replacement, and always runs `systemctl daemon-reload`. Failed reload remains
  retryable.
- The installer never enables, starts, restarts, or deploys a service. Timer
  enablement is a separate operator action. Install guard-aware units before
  relying on deploy-state enforcement.
- Rollback is a clean forward-revert release through the same health gate.
  Never downgrade the database or release metadata independently of code.

This production branch is the integrated runtime, security, and market release.
Its committed checkpoint allowlist preserves each reviewed pause through the
production delivery-report head. Operators still approve and execute one
literal checkpoint at a time rather than traversing to `head`. Protected seed,
quarantine, market-evidence, and existing-organization provenance writes remain
outside the application role; audit and status history remain append-only.
Forward reverts must preserve the authentication cutoff, acceptance priority,
and durable command receipts. Never perform an identity-only or database
downgrade rollback.

## API Endpoints

### Authentication
| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| POST | `/api/auth/login` | No | Login (returns access + refresh tokens) |
| POST | `/api/auth/refresh` | No | Refresh tokens |
| POST | `/api/auth/register` | No | Register new user |
| GET | `/api/auth/me` | Yes | Get current user profile |
| PUT | `/api/auth/me/password` | Yes | Change password (returns fresh tokens) |

### Order Book
| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| GET | `/api/orderbook` | No | List all open orders |
| GET | `/api/orderbook/bids` | No | List open BIDs |
| GET | `/api/orderbook/asks` | No | List open ASKs |
| POST | `/api/orderbook` | Yes | Place order (auto-matches crossing orders) |
| GET | `/api/orderbook/my` | Yes | List user's own orders |
| DELETE | `/api/orderbook/{id}` | Yes | Cancel own order |

### Trades
| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| POST | `/api/trades` | Yes | Hit an order to create a trade |
| GET | `/api/trades/my` | Yes | List user's trades |
| PUT | `/api/trades/{id}/confirm` | Yes | Confirm trade (counterparty only) |
| PUT | `/api/trades/{id}/decline` | Yes | Decline trade (counterparty only) |
| PUT | `/api/trades/{id}/deliver` | Yes | Mark delivered (with final qty/price) |
| POST | `/api/trades/{id}/pay` | Yes | Mark paid (seller only) |

### Price Discovery (Public)
| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| GET | `/api/prices` | No | Aggregated trade prices (24h) with source/scope/demo provenance |
| GET | `/api/prices/reference` | No | Daily VWAP reference prices |
| GET | `/api/trade-tape` | No | Anonymized 7-day confirmed trade tape; filterable by `market_product`, `delivery_point_id`, `region`, and `availability_window` |

Market-data surfaces use a shared provenance vocabulary:

- `source_kind`: `CONFIRMED_TRADE`, `LIVE_ORDER`, `DEMO_SEED`, `BENCHMARK_REFERENCE`, `MIXED_SOURCE`, `NO_DATA`, or `UNKNOWN`.
- `scope`: `DELIVERY_POINT`, `REGION`, `PRODUCT`, or `UNKNOWN`.
- `demo_status`: `REAL_ONLY`, `DEMO_ONLY`, `MIXED`, `UNKNOWN`, or `NOT_APPLICABLE`.

Aggregate responses expose real/demo/unknown counts. Unknown contributors remain `UNKNOWN` rather than being reported as real activity.

### Compliance
| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| GET | `/api/compliance/fleet` | Yes | Fleet-wide compliance summary |
| GET | `/api/compliance/vessels/{id}/score` | Yes | Single vessel score |
| POST | `/api/compliance/scenario` | Yes | What-if fuel mix scenario |
| GET | `/api/compliance/fuels` | No | Fuel GHG intensity reference |

### Real-Time (SSE)
| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| GET | `/api/stream/prices` | No | Price update events |
| GET | `/api/stream/orderbook` | No | Order events (created/cancelled/matched) with append-only provenance fields |
| GET | `/api/stream/trades` | No | Trade lifecycle events with append-only provenance fields |

### Admin (ADMIN role only)
| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| GET | `/api/admin/analytics/overview` | Admin | Platform stats |
| GET | `/api/admin/analytics/daily` | Admin | Daily breakdown |
| GET | `/api/admin/analytics/product-usage?days=7\|30\|90` | Admin | Aggregated behavioral usage plus authoritative registrations, logins, and order-placing organizations |
| GET | `/api/admin/audit-logs` | Admin | Audit trail |

Behavioral analytics is optional. Configure the server-only
`ANALYTICS_ENABLED`, `UMAMI_BASE_URL`, `UMAMI_WEBSITE_ID`,
`UMAMI_API_USERNAME`, `UMAMI_API_PASSWORD`, and bounded
`ANALYTICS_REQUEST_TIMEOUT_SECONDS` values to enable it. Missing or unavailable
Umami returns a degraded behavioral section with HTTP 200 while preserving the
Verdaxis database counts. See `docs/behavioral-analytics-contract.md` for the
privacy and event contracts.

## Test Accounts

| Email | Password | Role |
|-------|----------|------|
| buyer@buy.com | password | BUYER |
| seller@sell.com | password | SUPPLIER |

## Tests

```bash
ENVIRONMENT=test RELEASE_SHA=test \
  JWT_SECRET=test-secret-key-for-testing-minimum-32-chars \
  DATABASE_URL=sqlite+aiosqlite:///:memory: \
  python -m pytest tests/unit/ -v
```

Integration/E2E suites additionally require explicit pytest opt-in and an
attested disposable server on a numeric-loopback ephemeral port. Production,
staging, `localhost`, and ports 8000/8001 are refused by collection guards.
Start the source-tree producer with `ENVIRONMENT=test`, a matching
`DISPOSABLE_TEST_TOKEN`, a real release identity
`RELEASE_SHA=$(git rev-parse HEAD)` (the readiness assertions require a
full 40-hex SHA; the producer refuses the unit-test `test` sentinel), and
`python -m tests.disposable_server --port <ephemeral-port>`.

Run the mutating suites only with the explicit opt-in flags; collection
skips them otherwise:

```bash
DISPOSABLE_ITEST_PASSWORD=... \
  python -m pytest tests/integration -v \
  --run-disposable-integration \
  --disposable-target-url http://127.0.0.1:59123 \
  --disposable-target-token <token-provisioned-into-the-disposable-server>
```

There is no staging or production integration target: mutating suites accept
only an attested `127.0.0.1` ephemeral-port disposable server. (The former
`ALLOW_TEST_MUTATIONS`/`TEST_RUNTIME_ENV` resolver in `tests/runtime_config.py`
remains for staging-scoped tooling but no suite consumes it.)

Use `scripts/run_product_analytics_postgres_tests.sh` for a disposable
PostGIS container. It binds a unique loopback port, runs migrations, checks
Alembic drift, applies and validates the idempotent app/migrator/backup role
policy, exercises the runtime upgrade/downgrade roundtrip, and removes only
the container it created.

Credentialed API CORS is fail-closed by environment: production allows only
the canonical production landing/app origins, staging only its staging origin,
and development/test only enumerated localhost origins. Wildcards are never
used with credentials.

## Seed safety

Every executable seeder requires `SEED_DATABASE_URL`,
`ALLOW_SEED_MUTATIONS=I_UNDERSTAND_SEED_MUTATIONS`, `SEED_RUNTIME_ENV`, and an
exact matching `SEED_TARGET_DATABASE`. Production/system databases,
superuser URLs, non-loopback targets, and non-canonical availability windows
are denied. Disposable database names end in `_test`; staging is exactly
`verdaxis_staging`. Seeders never inherit an implicit application URL.

Historical repository versions contained a live database credential in seed
scripts. Current-tree source cleanup is insufficient and does not revoke it;
rewriting repository history is not a substitute for rotation. An operator
must create and verify a replacement least-privilege credential, update the
secret store and deployed environment, attest the replacement role, then
revoke the exposed credential. This repository change performs none of those
external actions.

## Feature Branches

- `feature/oauth-integration` — Google + Microsoft SSO (169 tests, ready for merge)

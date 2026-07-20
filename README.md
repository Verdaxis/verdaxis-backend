# Verdaxis Exchange — Backend API

Maritime fuel trading exchange platform backend.

## Quick Start

```bash
cd /home/verdaxis-prod/verdaxis/staging/be
source venv/bin/activate
ENVIRONMENT=development uvicorn app.main:app --host 127.0.0.1 --port 8000  # local only
```

## Live Deployment

The VPS runs the backend via systemd, not Docker Compose.

```bash
# Staging
cd /home/verdaxis-prod/verdaxis/staging/be
./scripts/deploy.sh --dry-run
./scripts/deploy.sh

# Production
cd /home/verdaxis-prod/verdaxis/prod/be
./scripts/deploy.sh --dry-run
./scripts/deploy.sh
```

The deploy helper infers the correct branch and service from the path and always refuses dirty worktrees. Dry-run materializes the exact remote candidate privately and really runs read-only runtime/database, dependency resolver, and Alembic-head checks while skipping every deployed-state mutation. An actual deploy creates `.runtime-deploying` before source mutation; every runtime-owned service refuses to start under that guard. Once the clean exact remote commit is selected, deploy atomically publishes its full SHA through `.runtime-release.env` before invoking new-tree preflight, pip, or Alembic. Later failures retain aligned code/identity and leave the release fail-closed instead of rolling metadata back; post-restart failure also stops the backend. After restart, parsed `/health/ready` JSON must report exact `status="ok"`, `db="ok"`, environment, and full release SHA. The application does not invoke Git. Off-host monitoring must use readiness; `/health/live` proves only that a process responds. The legacy `/health` path is a readiness alias.

`./scripts/install_systemd_units.sh --dry-run --environment <production|staging> --source-ref <approved-40-hex-sha>` is the no-change unit-install preflight. It accepts only the fixed checkout for the selected environment, requires that checkout to be clean and exactly at the approved release SHA, and stages the exact committed backend, singleton-news, and product-analytics-prune unit bytes in a private root-owned directory. It verifies their SHA-256 manifest there and never installs from mutable worktree paths. Production and staging are promoted independently and need not use the same SHA. After credentials, CORS, migration heads, and release identity are ready, an approved operator may repeat that environment's command with `--apply`; it installs only changed units and runs `systemctl daemon-reload` only when needed. It never enables, starts, restarts, or deploys a service. The guard-aware units must be installed before relying on the new deploy failure contract. Enabling each environment's news and prune timers is a separate operator-held action. Rollbacks publish a clean forward revert commit as a new release and use the same preflight/health gate; never automatically downgrade schema or roll release metadata back independently of code.

Cross-branch integration remains gated: runtime metadata stays directly after `pa_20260715_analytics_facts`, security integration reparents `sec_20260720_identity` onto runtime and preserves its chain through `sec_20260720_device`, and combined immutable unit allowlists add the production/staging auth-maintenance service/timer pairs. The runtime-only branch is not a complete combined runtime+security release.

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

Run the command above for the current test count.

Integration tests require an explicit `TEST_API_URL`; they never default to a
running service. Mutating helpers require both an exact acknowledgement and a
positive `TEST_RUNTIME_ENV` attestation. A disposable local API example is:

```bash
TEST_API_URL=http://127.0.0.1:18765 \
  ALLOW_TEST_MUTATIONS=I_UNDERSTAND_TEST_MUTATIONS \
  TEST_RUNTIME_ENV=disposable \
  TEST_DISPOSABLE_DB_NAME=verdaxis_runtime_test \
  python -m pytest tests/integration -v
```

For approved staging only, use `TEST_API_URL=https://api-staging.verdaxis.exchange`
with the same two guard variables and `TEST_RUNTIME_ENV=staging`. Production
hosts, `144.126.151.136`, and localhost:8000 are categorically refused.
The exact `http://127.0.0.1:8001` target is accepted only with
`TEST_RUNTIME_ENV=staging`; disposable tests reject both live ports.

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

# Verdaxis Exchange — Backend API

Maritime fuel trading exchange platform backend.

## Quick Start

```bash
cd /home/verdaxis-prod/verdaxis/staging/be
source venv/bin/activate
uvicorn app.main:app --host 0.0.0.0 --port 8000
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

The deploy helper infers the correct branch and service from the path, refuses dirty worktrees by default, runs migrations, restarts systemd, and checks the public health endpoint.

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
ENVIRONMENT=test JWT_SECRET=test-secret-key-for-testing-minimum-32-chars \
  python -m pytest tests/unit/ -v
```

Run the command above for the current test count.

Integration tests require an explicit `TEST_API_URL`; they never default to a
running service. Because the suite includes mutating flows, a non-loopback
target additionally requires:

```bash
TEST_API_URL=http://127.0.0.1:8000 python -m pytest tests/integration -v
```

For an approved remote staging target, add
`ALLOW_REMOTE_TEST_MUTATIONS=I_UNDERSTAND_REMOTE_TEST_MUTATIONS`. Production
Verdaxis API hosts are always refused.

Use `scripts/run_product_analytics_postgres_tests.sh` for a disposable
PostGIS container. It binds a unique loopback port, runs migrations, checks
Alembic drift, and removes only the container it created.

## Feature Branches

- `feature/oauth-integration` — Google + Microsoft SSO (169 tests, ready for merge)

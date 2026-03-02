# Verdaxis Exchange — Backend API

Maritime fuel trading exchange platform backend.

## Quick Start

```bash
cd /home/verdaxis-prod/verdaxis-backend
source venv/bin/activate
uvicorn app.main:app --host 0.0.0.0 --port 8000
```

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
| GET | `/api/prices` | No | Aggregated trade prices (24h) |
| GET | `/api/prices/reference` | No | Daily VWAP reference prices |

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
| GET | `/api/stream/orderbook` | No | Order events (created/cancelled/matched) |
| GET | `/api/stream/trades` | No | Trade lifecycle events |

### Admin (ADMIN role only)
| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| GET | `/api/admin/analytics/overview` | Admin | Platform stats |
| GET | `/api/admin/analytics/daily` | Admin | Daily breakdown |
| GET | `/api/admin/audit-logs` | Admin | Audit trail |

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

155 tests passing.

## Feature Branches

- `feature/oauth-integration` — Google + Microsoft SSO (169 tests, ready for merge)

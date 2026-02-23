# Architecture

> FastAPI + SQLAlchemy 2 (async) + PostgreSQL 15/PostGIS + Alembic + Pydantic v2

## File Map

```
app/
  main.py                       # FastAPI app init, CORS, mounts all routers under /api
  config.py                     # Pydantic Settings — env vars, DB URL assembly, CORS origins
  database.py                   # AsyncSession factory (asyncpg), Base declarative, get_db DI
  admin.py                      # SQLAdmin panel at /admin — User/Org CRUD + System Health page
  core/
    auth.py                     # Legacy get_current_user — JIT provisioning + auth bypass
    security.py                 # bcrypt hashing (via passlib), HS256 JWT token creation
    exceptions.py               # Placeholder (empty)
  models/
    __init__.py                 # Imports all models (required for Alembic autogenerate)
    user.py                     # User, Organization, UserRole, UserStatus, OrgType, TierLabel
    port.py                     # Port (PostGIS), PortIntelligence, Vessel
    marketplace.py              # InventoryItem, FuelType enum
    orderbook.py                # OrderBookOrder (BID/ASK), Trade, enums (OrderSide, TradeStatus)
    orders.py                   # Commission (legacy match_id FK + new trade_id FK)
    matchmaking.py              # MatchSuggestion (scored BID/ASK pairs)
    notification.py             # Notification, NotificationType
    compliance.py               # TraceabilityEvent, ComplianceLedger
    producer.py                 # ProducerProject (PostGIS location, GENA import)
  routers/
    auth_simple.py              # Active auth — login, register, register-with-org, /me
    orderbook.py                # Unified order book CRUD — list/create/update/cancel orders
    trades.py                   # Trade lifecycle — create/confirm/decline/deliver/pay
    matchmaking.py              # Match suggestions — generate, list, dismiss
    price_discovery.py          # Public price ticker — aggregated trade prices
    availability.py             # Public fuel availability by port (map data)
    demand.py                   # Public anonymized BID demand signals
    producers.py                # Public producer project list (map data)
    notifications.py            # User notification CRUD — list, mark read
    inventory.py                # Supplier inventory management + publish-to-ASK
    ports.py                    # Port data with PostGIS coordinate extraction
    vessels.py                  # Vessel data (org-scoped, admin sees all)
    compliance.py               # Compliance ledger + document verification stub
    ai.py                       # Gemini AI chat endpoint
    orders.py                   # Admin commission management (legacy)
    dashboard.py                # System health metrics (CPU, RAM, disk)
  schemas/
    user.py                     # UserCreate, UserResponse, RegistrationResponse
    organization.py             # OrganizationCreate, OrganizationResponse
    orderbook.py                # Order/Trade/CI-pricing schemas, PriceSummary
    orders.py                   # Commission schemas
    availability.py             # PortFuelAvailability, AvailabilityLevel enum
    demand.py                   # DemandSignal, UrgencyLevel enum
    producer.py                 # ProducerProjectResponse
    marketplace.py              # InventoryItem schemas
    compliance.py               # Compliance schemas
    port.py                     # Port schemas
    vessel.py                   # Vessel schemas
  services/
    ai_service.py               # Gemini chat + document analysis (stub)
    matchmaking.py              # Score-based BID/ASK matching (0-100, region groups, price gap)
    ci_pricing.py               # Carbon intensity adjusted pricing (FuelEU ref: 91 gCO2eq/MJ)
alembic/
  env.py                        # Async migrations, overrides sqlalchemy.url from Settings
  versions/                     # Migration scripts (16 revisions)
scripts/
  seed.py                       # Database seeder (uses legacy PublicListing/Order models)
  seed_maersk_vessels.py        # Supplementary vessel seed data
  import_gena_csv.py            # Import producer projects from GENA CSV
  check_users.py                # Utility to list users
  deploy.sh                     # Server deploy script
  test_purchase_flow.py         # Manual end-to-end purchase test
tests/
  conftest.py                   # Shared fixtures — httpx client, sample data
  unit/                         # 16 test files — pure logic, sqlite in-memory
  integration/                  # 4 test files — against running Docker backend
  e2e_local_api.py              # Manual local E2E test
  e2e_remote_flow.py            # Manual remote E2E test
templates/
  system_health.html            # Jinja2 template for admin health dashboard
docker-compose.yml              # postgres(PostGIS) + redis + backend + frontend
Dockerfile                      # Python 3.11-slim, pip install, uvicorn
.github/workflows/backend-ci.yml  # Unit tests on PR, deploy-on-push to main
```

## Dependency Flow

```
                    +-----------+
                    |  main.py  |  Mounts 15 routers under /api
                    +-----+-----+
                          |
            +-------------+-------------+
            |                           |
     +------+------+            +------+------+
     |   routers/  |            |   admin.py  |  /admin (SQLAdmin)
     +------+------+            +------+------+
            |                          |
     +------+------+            +------+------+
     |  schemas/   |            |  database   |  AsyncSession + Base
     +------+------+            +------+------+
            |                          |
     +------+------+                   |
     |  services/  |                   |
     +------+------+                   |
            |                          |
     +------+------+            +------+------+
     |   models/   +------------+   alembic/  |  Migration autogenerate
     +------+------+            +-------------+
            |
     +------+------+
     |  config.py  |  .env -> Pydantic Settings -> DATABASE_URL, JWT_SECRET, etc.
     +-------------+
```

## Key Patterns

- **Dual auth implementations**: Active auth in `routers/auth_simple.py` (most routers). Legacy auth in `core/auth.py` (vessels, inventory, compliance, ai). Both decode HS256 JWT but differ in error handling and bypass support.
- **Org-scoped access**: Users belong to organizations. Orders, trades, and inventory are scoped to `organization_id`. Role-based checks are inline in handlers.
- **Trade lifecycle state machine**: `PENDING_CONFIRMATION -> CONFIRMED -> DELIVERED -> PAID`. Decline from PENDING restores order quantity. Only counterparty (non-initiator) can confirm/decline.
- **PostGIS serialization**: Geography columns must be nulled before Pydantic serialization. Coordinates extracted via `ST_X`/`ST_Y` and set as dynamic attributes.
- **Enum duplication**: Enums are defined separately in models and schemas (not shared).
- **Static routes before parametric**: In orderbook router, named paths (`/bids`, `/asks`, `/my`) are defined before `/{order_id}` to avoid path conflicts.

## Entry Points

| Entry Point | Purpose |
|---|---|
| `uvicorn app.main:app` | Start the API server |
| `alembic upgrade head` | Run database migrations |
| `python scripts/seed.py` | Seed demo data |
| `/admin` | SQLAdmin panel (session auth) |
| `/docs` | Swagger UI |
| `/health` | Health check |

## Run Commands

```bash
# Start all services
docker compose up -d --build

# Run backend directly (dev)
uvicorn app.main:app --reload --host 0.0.0.0 --port 8000

# Unit tests (no DB required)
DATABASE_URL="sqlite+aiosqlite:///:memory:" pytest tests/unit/ -v

# Integration tests (requires running Docker)
pytest tests/integration/ -v

# Migrations
alembic upgrade head
alembic revision --autogenerate -m "description"
```

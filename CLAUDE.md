# Verdaxis Backend - Claude Code Instructions

## Project Overview

Backend API for Verdaxis -- a maritime intelligence and procurement platform. Handles fuel procurement (order book with BID/ASK matching), compliance auditing (EU ETS, FuelEU Maritime), port intelligence with geospatial data, AI copilot via Google Gemini, and a trade lifecycle (create -> confirm -> deliver -> pay).

**Repo:** `jonathanjie/verdaxis-backend`
**Runtime:** Python 3.10+ / FastAPI / PostgreSQL 15 with PostGIS / SQLAlchemy 2 (async) / Alembic

## Architecture

```
app/
  main.py              # FastAPI app, CORS, router registration
  config.py            # Pydantic Settings (reads .env)
  database.py          # AsyncSession factory, Base, get_db dependency
  admin.py             # SQLAdmin panel (User/Org CRUD + System Health page)
  core/
    auth.py            # get_current_user (JWT verify + JIT provisioning, legacy)
    security.py        # bcrypt hashing, create_access_token (HS256)
    exceptions.py      # Placeholder (empty)
  models/              # SQLAlchemy ORM models
    __init__.py        # Imports all models (required for Alembic autogenerate)
    user.py            # User, Organization, UserRole, UserStatus, OrgType, TierLabel
    port.py            # Port (PostGIS), PortIntelligence, Vessel
    marketplace.py     # InventoryItem, FuelType enum
    orderbook.py       # OrderBookOrder (BID/ASK), Trade, enums
    orders.py          # Commission (legacy match_id FK + new trade_id FK)
    matchmaking.py     # MatchSuggestion
    notification.py    # Notification, NotificationType
    compliance.py      # TraceabilityEvent, ComplianceLedger
    producer.py        # ProducerProject (PostGIS location)
  routers/             # API route handlers
  schemas/             # Pydantic request/response models
  services/            # Business logic
    ai_service.py      # Google Gemini chat + document analysis (stub)
    matchmaking.py     # Score-based BID/ASK matching (0-100 score)
    ci_pricing.py      # Carbon intensity adjusted pricing (FuelEU ref: 91 gCO2eq/MJ)
alembic/               # Database migrations
scripts/               # Seed data, deploy, CSV import
tests/
  unit/                # Pure logic tests (no DB required)
  integration/         # Tests against running Docker backend
```

## Tech Stack

| Component      | Details                                                    |
| -------------- | ---------------------------------------------------------- |
| Framework      | FastAPI with async/await everywhere                        |
| Database       | PostgreSQL 15 + PostGIS 3.3 (via `postgis/postgis` image) |
| ORM            | SQLAlchemy 2.0 with `AsyncSession` (`asyncpg` driver)     |
| Migrations     | Alembic (async, reads `DATABASE_URL` from Settings)        |
| Geospatial     | GeoAlchemy2 (`Geography(POINT, 4326)`)                     |
| Auth           | JWT HS256 (self-signed), bcrypt password hashing           |
| AI             | Google Generative AI SDK (`gemini-pro`)                    |
| Admin Panel    | SQLAdmin at `/admin` (session-based auth)                  |
| Serialization  | Pydantic v2 with `from_attributes = True`                  |
| HTTP Client    | httpx (for tests)                                          |
| Container      | Docker Compose (postgres + redis + backend + frontend)     |

## Development Commands

```bash
# Activate virtual environment
source /home/verdaxis-prod/verdaxis-backend/venv/bin/activate

# Start services (from project root)
docker compose up -d --build

# Run the backend directly (without Docker)
uvicorn app.main:app --reload --host 0.0.0.0 --port 8000

# Run all unit tests (no DB required, uses sqlite in-memory)
DATABASE_URL="sqlite+aiosqlite:///:memory:" pytest tests/unit/ -v

# Run integration tests (requires running Docker backend)
pytest tests/integration/ -v

# Run integration tests against production
TEST_API_URL=http://144.126.151.136:8000 pytest tests/integration/ -v

# Run Alembic migrations
alembic upgrade head

# Create a new migration
alembic revision --autogenerate -m "description_here"

# Seed the database
python scripts/seed.py

# Full deploy script (on server)
bash scripts/deploy.sh
```

## Deployment

**Server:** `verdaxis-prod@144.126.151.136`
**API:** `https://api.verdaxis.exchange/api` (Caddy reverse proxy -> `localhost:8000`)
**Swagger:** `https://api.verdaxis.exchange/docs`
**Admin Panel:** `https://api.verdaxis.exchange/admin` (credentials from `ADMIN_USERNAME`/`ADMIN_PASSWORD` in `.env`)

### CI/CD (GitHub Actions)

Push to `main` triggers `.github/workflows/backend-ci.yml`:
1. Runs unit tests with `DATABASE_URL=sqlite+aiosqlite:///:memory:`
2. If tests pass AND it was a push (not PR), SSH deploys to VPS:
   ```
   git pull origin main -> docker compose down -> docker compose up -d --build -> docker system prune -f
   ```

### Manual Deploy

```bash
ssh verdaxis-prod@144.126.151.136
cd ~/verdaxis-backend
git pull origin main
docker compose down
docker compose up -d --build
# Run migrations if schema changed:
docker exec verdaxis-backend alembic upgrade head
```

## Database & Migrations

- **Docker Compose maps port 5433 -> 5432** (not the default 5432 on host)
- The database URL is assembled from individual env vars (`DATABASE_HOST`, `DATABASE_PORT`, `DATABASE_NAME`, `DATABASE_USER`, `DATABASE_PASSWORD`) unless `DATABASE_URL` is explicitly set
- In Docker, `DATABASE_HOST=verdaxis-db` (the container name)
- Alembic's `env.py` overrides `sqlalchemy.url` from Settings at runtime. The `alembic.ini` value (`driver://user:pass@localhost/dbname`) is never used.
- Alembic `include_object` excludes `spatial_ref_sys` (PostGIS system table) from autogeneration
- All models MUST be imported in `app/models/__init__.py` or Alembic autogenerate will miss them

### Key Tables

| Table                | Purpose                                           |
| -------------------- | ------------------------------------------------- |
| `users`              | Auth, role (BUYER/SUPPLIER/ADMIN), status         |
| `organizations`      | Companies. Users belong to orgs. Has `domain` for auto-matching on registration |
| `orderbook_orders`   | Unified order book. side=BID (buyer wants) or ASK (seller offers) |
| `trades`             | Matched transactions. Lifecycle: PENDING -> CONFIRMED -> DELIVERED -> PAID |
| `match_suggestions`  | AI-scored potential matches between BID and ASK orders |
| `commissions`        | Verdaxis revenue tracking per trade (0.5% default rate) |
| `ports`              | PostGIS POINT geometry. ID format: `SGSIN`, `NLRTM`, etc. |
| `vessels`            | Buyer fleet. PostGIS current/previous location    |
| `inventory_items`    | Supplier stock per port per fuel type              |
| `notifications`      | In-app notifications with JSON `data` payload     |
| `producer_projects`  | Fuel production facilities for the map (GENA data) |
| `compliance_ledger`  | EU ETS/FuelEU financial transactions               |

## API Routes (all prefixed with `/api`)

### Public (no auth)
- `GET /api/prices` -- Aggregated trade price summaries (price ticker)
- `GET /api/availability` -- Fuel availability by port (map data)
- `GET /api/demand` -- Anonymized buyer demand signals
- `GET /api/producers` -- Producer project list (map data)
- `GET /api/orderbook` -- All open orders
- `GET /api/orderbook/bids` -- Open BID orders
- `GET /api/orderbook/asks` -- Open ASK orders
- `GET /api/orderbook/aggregated` -- Market data grouped by region/fuel/side
- `GET /api/orderbook/regions` -- Distinct regions
- `GET /api/orderbook/fuel-types` -- Distinct fuel types
- `GET /api/orderbook/with-ci` -- Orders enriched with CI-adjusted pricing
- `GET /api/listings` -- Backward-compatible ASK listing view
- `GET /` -- Health message
- `GET /health` -- Status check

### Auth (`/api/auth`)
- `POST /api/auth/login` -- OAuth2 password form (email in `username` field) -> JWT
- `POST /api/auth/register` -- Returns `{status: "created", user}` or `{status: "requires_org", registration_token}` if org not found by email domain
- `POST /api/auth/register-with-org` -- Complete registration with new org creation
- `GET /api/auth/me` -- Current user info

### Authenticated
- `POST /api/orderbook` -- Place BID (BUYER) or ASK (SUPPLIER) order
- `PUT /api/orderbook/{id}` -- Update own order
- `DELETE /api/orderbook/{id}` -- Cancel own order
- `GET /api/orderbook/my` -- Own orders with trade counts
- `POST /api/trades/` -- Hit an order to create a trade
- `GET /api/trades/my` -- Own trades
- `PUT /api/trades/{id}/confirm` -- Counterparty confirms
- `PUT /api/trades/{id}/decline` -- Counterparty declines (restores order quantity)
- `PUT /api/trades/{id}/deliver` -- Mark delivered with final qty/price (calculates commission)
- `POST /api/trades/{id}/pay` -- Seller marks as paid
- `GET /api/notifications` -- User's notifications
- `GET /api/notifications/unread-count`
- `PATCH /api/notifications/{id}/read`
- `PATCH /api/notifications/read-all`
- `GET /api/vessels` -- Org-scoped (admin sees all)
- `GET /api/inventory` -- Supplier's inventory
- `POST /api/inventory` -- Add inventory item
- `POST /api/inventory/{id}/publish` -- Convert inventory to ASK order
- `GET /api/compliance/ledger` -- Org compliance records
- `POST /api/compliance/verify` -- Upload document for AI verification (stub)
- `POST /api/ai/chat` -- Gemini chat
- `GET /api/matchmaking/suggestions` -- Match suggestions for org
- `POST /api/matchmaking/generate/{order_id}` -- Trigger match generation
- `PATCH /api/matchmaking/suggestions/{id}/dismiss`
- `GET /api/dashboard/health` -- System metrics (CPU, RAM, disk)

### Admin Only
- `GET /api/orders/admin/commissions` -- All commissions
- `GET /api/orders/admin/commissions/summary` -- Commission stats
- `PUT /api/orders/admin/commissions/{id}` -- Update commission status

## Authentication

- **Protocol:** JWT HS256, self-signed with `JWT_SECRET` from `.env`
- **Token lifetime:** 24 hours (1440 minutes)
- **Login:** `POST /api/auth/login` uses OAuth2 `username`/`password` form fields. The `username` field contains the email address.
- **Two `get_current_user` implementations exist:**
  - `app/routers/auth_simple.py` -- The active one. Used by most routers. Decodes JWT `sub` claim as user UUID.
  - `app/core/auth.py` -- Legacy. Has JIT provisioning and dev bypass logic. Used only by `vessels`, `inventory`, `compliance`, and `ai` routers.
- **Registration flow:** If the user's email domain matches an existing `Organization.domain`, the user is created immediately linked to that org. Otherwise, a short-lived registration token is returned and the user must call `/register-with-org` to create their org first.
- **Auth bypass:** Controlled by `ENABLE_AUTH_BYPASS=true` in `.env`. Creates/returns a `dev@admin.com` user. Only works with the legacy `core/auth.py` path.

### Test Credentials

| Role     | Email                  | Password    |
| -------- | ---------------------- | ----------- |
| Admin    | admin@verdaxis.com     | ***REMOVED***    |
| Buyer    | buyer1@verdaxis.com    | password123 |
| Supplier | supplier1@verdaxis.com | password123 |

## Coding Conventions

1. **All I/O is async.** DB queries use `AsyncSession`, all route handlers are `async def`.
2. **Dependency injection** via `Depends()` for `get_db` (session) and `get_current_user` (auth).
3. **Pydantic v2 schemas** with `class Config: from_attributes = True` for ORM serialization.
4. **Enums are `(str, enum.Enum)`** in both models and schemas. They are duplicated between model and schema layers (not shared).
5. **UUIDs for all primary keys** except `Port.id` (which uses string codes like `SGSIN`).
6. **Numeric fields use `Decimal`** in models and schemas, mapped to `Numeric(precision, scale)` in PostgreSQL.
7. **PostGIS geography columns** (`Geography(POINT, 4326)`) are set to `None` before serialization since Pydantic cannot serialize `WKBElement`. Lat/lng are extracted via `ST_X`/`ST_Y` and set as dynamic attributes on the ORM object.
8. **Role-based access control** is checked inline in route handlers (`if current_user.role != UserRole.BUYER: raise HTTPException(403, ...)`).
9. **Router prefix pattern:** Some routers define their prefix in `APIRouter(prefix="/...")`, others rely on `@router.get("/endpoint")` path. All are mounted with `prefix=settings.API_V1_STR` (`/api`) in `main.py`.
10. **Orderbook router: static routes before parametric.** `/bids`, `/asks`, `/my`, `/aggregated`, `/regions`, `/fuel-types`, `/with-ci` are defined before `/{order_id}` to avoid path conflicts.

## Known Gotchas

1. **API returns numbers as strings.** Pydantic serializes `Decimal` fields as strings by default. The frontend must wrap numeric fields with `Number()` before arithmetic. Affected fields: `quantity_mt`, `price_per_mt_usd`, `final_quantity_mt`, `final_price_per_mt`, `final_total_usd`, etc.

2. **Two `get_current_user` implementations.** Most routers import from `app.routers.auth_simple`. A few older routers (`vessels`, `inventory`, `compliance`, `ai`) import from `app.core.auth`. Both decode JWT but the legacy one also supports auth bypass and JIT user provisioning. Be careful which one a router uses.

3. **Seed script uses legacy models.** `scripts/seed.py` imports `PublicListing`, `Order`, `ListingStatus`, `OrderStatus` from `app.models.orders` -- these are legacy pre-orderbook models. The seed script will fail if those classes have been removed. It needs updating to use the current `OrderBookOrder`/`Trade` models.

4. **PostGIS serialization.** Never return a raw `Geography`/`Geometry` column to Pydantic. Always extract coordinates with `ST_X`/`ST_Y`, set them as attributes, and null out the geography column before returning. See `routers/ports.py` for the pattern.

5. **Docker Compose postgres port is 5433 on host**, not 5432. If connecting from host machine, use `localhost:5433`. Inside Docker network, containers use `verdaxis-db:5432`.

6. **`alembic.ini` sqlalchemy.url is a dummy.** The real URL comes from `app.config.settings.DATABASE_URL` and is set in `alembic/env.py`. Do not edit the URL in `alembic.ini`.

7. **Authentik is deprecated.** References to Authentik in config and old code are dead. Auth is purely self-managed JWT. Do not re-enable Authentik integration.

8. **Commission model has dual FKs.** `Commission.match_id` points to legacy `orders` table (NOT nullable). `Commission.trade_id` points to the new `trades` table (nullable). Legacy FK is kept for historical data.

9. **`datetime.utcnow()` is used throughout** for timestamps. This is deprecated in Python 3.12+ in favor of `datetime.now(timezone.utc)`. Active deprecation warnings on Python 3.12. Will break on Python 3.13. Should be migrated soon.

10. **Trade lifecycle is strict.** The state machine is: `PENDING_CONFIRMATION -> CONFIRMED -> DELIVERED -> PAID`. Decline from PENDING restores order quantity. Only the counterparty (non-initiator) can confirm/decline. Only the seller can mark as paid.

11. **AvailabilityWindow enum values are hardcoded quarters** (Q1 2025 through Q4 2026, plus Forward 2027/2028). Q1-Q4 2025 are now historical. Needs 2027+ values added.

12. **`python-jose` is unmaintained.** Last release was 2022 (v3.5.0) with known CVEs. Recommend migrating to `PyJWT` or `joserfc` for JWT handling.

13. **`passlib` is unmaintained.** Last release was 2020 (v1.7.4). Depends on the deprecated `crypt` module removed in Python 3.13. Recommend migrating to direct `bcrypt` or `argon2-cffi`.

14. **Redis container is running but unused.** `docker-compose.yml` provisions a Redis container, but no application code references Redis. It consumes memory and creates unnecessary attack surface. Should be removed or utilized.

## Environment Variables

Key variables in `.env` (loaded by `pydantic-settings`):

```
DATABASE_HOST=verdaxis-db       # "localhost" for non-Docker
DATABASE_PORT=5432
DATABASE_NAME=verdaxis
DATABASE_USER=postgres
DATABASE_PASSWORD=...
DATABASE_URL=                   # Optional override (e.g. sqlite+aiosqlite:///:memory: for tests)
JWT_SECRET=...                  # MUST be strong in production
GEMINI_API_KEY=...              # Optional, AI features degrade gracefully without it
ADMIN_USERNAME=...              # For /admin panel login
ADMIN_PASSWORD=...
ENABLE_AUTH_BYPASS=false        # Never true in production
```

## Git Workflow

- **`main`** branch: Active development. Push triggers CI/CD deploy.
- **`prod`** branch: Exists but currently mirrors `main`.
- CI runs unit tests then deploys via SSH on push to `main`.

## Known Gotchas (continued)

15. **Never use `--reload` in production Docker.** The `docker-compose.yml` `command:` used to include `--reload`, which caused uvicorn's `StatReload` to poll all 11,243 files in the bind-mounted `/app` directory (including `venv/` with 3,267 `.py` files and `postgres_data/`). This burned 243% CPU doing nothing. The fix: production compose uses plain `uvicorn` without `--reload`; dev uses `docker-compose.override.yml` with `--reload-dir` targeting only source directories.

16. **`.dockerignore` does NOT affect bind mounts.** The existing `.dockerignore` correctly excludes `venv/` and `postgres_data/` from `docker build` context, but the `volumes: - .:/app` bind mount bypasses it entirely. If using `--reload` with a bind mount, you MUST use `--reload-dir` to whitelist directories, not rely on `.dockerignore`.

17. **Frontend polls `/api/notifications` even when unauthenticated.** The frontend has a polling loop that hits `GET /api/notifications` and receives `401 Unauthorized` repeatedly. This generates log noise and wastes request cycles. The frontend should check auth state before starting the polling interval, or the polling should stop after receiving a 401.

18. **Server is exposed on `0.0.0.0:8000` and receives internet scanner traffic.** Random IPs probe for `/bins/`, `httpbin.org`, `/backup/`, etc. Consider restricting the backend port to `127.0.0.1:8000` in `docker-compose.yml` and letting Caddy handle external traffic exclusively.

## Development with Hot Reload

For local development with hot-reload, the project includes `docker-compose.override.yml`:

```bash
# Dev mode (auto-reload enabled via override):
docker compose up -d --build

# Production mode (no reload, rename override first):
mv docker-compose.override.yml docker-compose.override.yml.dev
docker compose up -d --build
```

The override uses `--reload-dir` to watch only `app/`, `alembic/`, and `scripts/` directories, plus `watchfiles` (inotify-based) instead of the default `StatReload` (polling). This keeps CPU near zero even in dev mode.

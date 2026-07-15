# Verdaxis Backend - Claude Code Instructions

## Live VPS Layout

The live VPS deployment is systemd-based, not Docker-based:

- Production backend: `/home/verdaxis-prod/verdaxis/prod/be`, branch `prod`, service `verdaxis-backend.service`, health `https://api.verdaxis.exchange/health`
- Staging backend: `/home/verdaxis-prod/verdaxis/staging/be`, branch `staging`, service `verdaxis-backend-staging.service`, health `https://api-staging.verdaxis.exchange/health`
- Deploy helper: `./scripts/deploy.sh`

The deploy helper prints branch, SHA, service, and health target, refuses dirty worktrees by default, runs Alembic, restarts the correct systemd service, and checks live health. Use `./scripts/deploy.sh --dry-run` before real deploys. Use `ALLOW_DIRTY=1` only for an intentional hotfix deploy from a known dirty tree.

Read ARCHITECTURE.md before exploring the codebase.

## Project Overview

Backend API for Verdaxis -- a maritime intelligence and procurement platform. Handles fuel procurement (order book with BID/ASK matching), compliance auditing (EU ETS, FuelEU Maritime), port intelligence with geospatial data, AI copilot via Google Gemini, and a trade lifecycle (create -> confirm -> deliver -> pay).

**Repo:** `jonathanjie/verdaxis-backend`
**Runtime:** Python 3.10+ / FastAPI / PostgreSQL 15 with PostGIS / SQLAlchemy 2 (async) / Alembic

## Development Commands

```bash
# Activate virtual environment from the current live tree
source ./venv/bin/activate

# Run the backend directly for local development
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

# Full deploy script (on server, from prod/be or staging/be)
./scripts/deploy.sh --dry-run
./scripts/deploy.sh
```

## Deployment

**Server:** `verdaxis-prod@144.126.151.136`
**API:** `https://api.verdaxis.exchange/api` (Caddy reverse proxy -> `localhost:8000`)
**Swagger:** `https://api.verdaxis.exchange/docs`
**Admin Panel:** `https://api.verdaxis.exchange/admin` (credentials from `ADMIN_USERNAME`/`ADMIN_PASSWORD` in `.env`)

### CI/CD (GitHub Actions)

CI (`.github/workflows/backend-ci.yml`) runs the unit-test suite and a strict `pip-audit` dependency gate on pushes and PRs to `staging` and `prod` — the branches that are actually deployed. CI does NOT deploy: deploys are operator-run on the VPS via `scripts/deploy.sh` against the systemd services described above. Any remaining docs referring to Docker-based CI or a `main` deploy branch are legacy.

### Manual Deploy

```bash
ssh verdaxis-prod@144.126.151.136
cd /home/verdaxis-prod/verdaxis/staging/be   # or /home/verdaxis-prod/verdaxis/prod/be
./scripts/deploy.sh --dry-run
./scripts/deploy.sh
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
- **Token lifetime:** 15-minute access tokens plus 7-day refresh tokens
- **Login:** `POST /api/auth/login` uses OAuth2 `username`/`password` form fields. The `username` field contains the email address.
- **Refresh transport:** `POST /api/auth/refresh` accepts the refresh token from either the JSON body or the HttpOnly `refresh_token` cookie scoped to `/api/auth`. Login, refresh, logout, and password change rotate or clear that cookie.
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
11. **After completing work, update ARCHITECTURE.md if file structure or key relationships changed.**

## Known Gotchas

1. **API returns numbers as strings.** Pydantic serializes `Decimal` fields as strings by default. The frontend must wrap numeric fields with `Number()` before arithmetic. Affected fields: `quantity_mt`, `price_per_mt_usd`, `final_quantity_mt`, `final_price_per_mt`, `final_total_usd`, etc.

2. **Two `get_current_user` implementations.** Most routers import from `app.routers.auth_simple`. A few older routers (`vessels`, `inventory`, `compliance`, `ai`) import from `app.core.auth`. Both decode JWT but the legacy one also supports auth bypass and JIT user provisioning. Be careful which one a router uses.

3. **`scripts/seed.py` is now a thin runner over `app.seeds.seed_all()`.** If you need to change demo market data, update `app/seeds/catalog_seed.py` and `app/seeds/market_seed.py` rather than reintroducing ad hoc legacy seed logic in the script itself.

4. **PostGIS serialization.** Never return a raw `Geography`/`Geometry` column to Pydantic. Always extract coordinates with `ST_X`/`ST_Y`, set them as attributes, and null out the geography column before returning. See `routers/ports.py` for the pattern.

5. **Docker Compose postgres port is 5433 on host**, not 5432. If connecting from host machine, use `localhost:5433`. Inside Docker network, containers use `verdaxis-db:5432`.

6. **`alembic.ini` sqlalchemy.url is a dummy.** The real URL comes from `app.config.settings.DATABASE_URL` and is set in `alembic/env.py`. Do not edit the URL in `alembic.ini`.

7. **Authentik is deprecated.** References to Authentik in config and old code are dead. Auth is purely self-managed JWT. Do not re-enable Authentik integration.

8. **Commission model has dual FKs.** `Commission.match_id` points to legacy `orders` table (NOT nullable). `Commission.trade_id` points to the new `trades` table (nullable). Legacy FK is kept for historical data.

9. **`datetime.utcnow()` is used throughout** for timestamps. This is deprecated in Python 3.12+ in favor of `datetime.now(timezone.utc)`. Active deprecation warnings on Python 3.12. Will break on Python 3.13. Should be migrated soon.

10. **Trade lifecycle is strict.** The state machine is: `PENDING_CONFIRMATION -> CONFIRMED -> DELIVERED -> PAID`. Decline from PENDING restores order quantity. Only the counterparty (non-initiator) can confirm/decline. Only the seller can mark as paid.

11. **Availability windows are canonical strings, not a static enum.** Persist `SPOT`, `YYYY-MM`, `YYYY-QN`, and legacy-compatible `YYYY-CAL`. UI labels like `M`, `M+1`, and `Next Quarter` must be resolved to canonical codes before they hit the API.

12. **`python-jose` is unmaintained.** Last release was 2022 (v3.5.0) with known CVEs. Recommend migrating to `PyJWT` or `joserfc` for JWT handling.

13. **`passlib` is unmaintained.** Last release was 2020 (v1.7.4). Depends on the deprecated `crypt` module removed in Python 3.13. Recommend migrating to direct `bcrypt` or `argon2-cffi`.

14. **Redis container is running but unused.** `docker-compose.yml` provisions a Redis container, but no application code references Redis. It consumes memory and creates unnecessary attack surface. Should be removed or utilized.

15. **Never use `--reload` in production Docker.** The `docker-compose.yml` `command:` used to include `--reload`, which caused uvicorn's `StatReload` to poll all 11,243 files in the bind-mounted `/app` directory (including `venv/` with 3,267 `.py` files and `postgres_data/`). This burned 243% CPU doing nothing. The fix: production compose uses plain `uvicorn` without `--reload`; dev uses `docker-compose.override.yml` with `--reload-dir` targeting only source directories.

16. **`.dockerignore` does NOT affect bind mounts.** The existing `.dockerignore` correctly excludes `venv/` and `postgres_data/` from `docker build` context, but the `volumes: - .:/app` bind mount bypasses it entirely. If using `--reload` with a bind mount, you MUST use `--reload-dir` to whitelist directories, not rely on `.dockerignore`.

17. **Frontend polls `/api/notifications` even when unauthenticated.** The frontend has a polling loop that hits `GET /api/notifications` and receives `401 Unauthorized` repeatedly. This generates log noise and wastes request cycles. The frontend should check auth state before starting the polling interval, or the polling should stop after receiving a 401.

18. **Server is exposed on `0.0.0.0:8000` and receives internet scanner traffic.** Random IPs probe for `/bins/`, `httpbin.org`, `/backup/`, etc. Consider restricting the backend port to `127.0.0.1:8000` in `docker-compose.yml` and letting Caddy handle external traffic exclusively.

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

- **`staging`** branch: live staging backend source at `/home/verdaxis-prod/verdaxis/staging/be`.
- **`prod`** branch: live production backend source at `/home/verdaxis-prod/verdaxis/prod/be`.
- **`main`** branch: legacy/default development branch in older docs and workflows.
- Do not assume a push to `main` deploys the current live topology; verify `.github/workflows/backend-ci.yml` before relying on CI/CD.

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
<!-- codesight-local:start -->
## Codesight Bootstrap

Before exploring the tree, read:
1. `.codesight/wiki/index.md` — 200-token catalog of all wiki articles (start here)
2. `.codesight/wiki/overview.md` — architecture and high-impact files
3. Load topic articles on demand: `.codesight/wiki/<topic>.md` (auth, database, payments, users, ui, etc.)
4. `.codesight/CODESIGHT.md` — full route/schema/lib map (fallback if wiki missing)
2. `.codesight/libs.md` if present
3. `.codesight/routes.md` if the task touches routes or handlers
4. `.codesight/schema.md` if the task touches models or database code

Only open full source files after consulting the wiki first.
<!-- codesight-local:end -->

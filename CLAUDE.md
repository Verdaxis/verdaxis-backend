# Verdaxis Backend - Claude Code Instructions

## Live VPS Layout

The live VPS deployment is systemd-based, not Docker-based:

- Production backend: `/home/verdaxis-prod/verdaxis/prod/be`, branch `prod`, service `verdaxis-backend.service`, readiness `https://api.verdaxis.exchange/health/ready`
- Staging backend: `/home/verdaxis-prod/verdaxis/staging/be`, branch `staging`, service `verdaxis-backend-staging.service`, readiness `https://api-staging.verdaxis.exchange/health/ready`
- Production Uvicorn binds `127.0.0.1:8000`; staging binds `127.0.0.1:8001`. Caddy/reverse-proxy health URLs are the public surfaces.
- Deploy helper: `./scripts/deploy.sh`
- Unit installer: `./scripts/install_systemd_units.sh --dry-run --environment <production|staging> --source-ref <approved-40-hex-sha>` from that environment's fixed, clean checkout. It stages the exact committed environment allowlist and unit bytes with a digest manifest in a private root-owned directory before preflight; production and staging refs are independent.
- News refresh is owned only by `verdaxis-news-refresh.timer` in production and `verdaxis-news-refresh-staging.timer` in staging. Web workers do not schedule it, and the API has no manual refresh route.
- Product-analytics pruning is owned only by its environment-specific timer. Its service requires PostgreSQL/network readiness, uses bounded retries and the news sandbox, and its destructive CLI requires an explicit deployed environment and full SHA matching `.runtime-release.env`; it has no development default.

The deploy helper always refuses dirty worktrees. Canonical checkouts fix their branch, guard directory, and trusted tool path and discard ambient Git/database routing overrides. Dry-run resolves and archives one remote full SHA, verifies only its immutable unit manifest, migration policy, ACL convergence bundle, and systemd bytes with trusted tooling, and never executes candidate Python/build/Alembic code or supplies it with `.env`, live DB, home, agent, or network access. It prints `APPROVED_RELEASE_SHA=<sha>`; real deploy requires that exact release SHA, the same exact `MIGRATION_APPROVED_SOURCE_SHA`, an exact expected current revision, and an allowlisted literal target from that SHA's `deploy/migration-checkpoints.tsv`. Missing values, aliases such as `head`, moved source, unexpected live state, and unlisted transitions refuse before source mutation. Checkpoint application and startup revision verification require explicit app/migrator URLs on the same database endpoint with distinct exact roles; they never fall back to `DATABASE_URL`. The release artifact publishes the exact target as `MIGRATION_REVISION`; backend units verify that revision rather than source head, so reviewed pauses remain startable. Actual deploy uses a durable per-environment flock and `.runtime-deploy/<environment>.state` before source mutation; every runtime service fails closed except the explicit restart-authorized phase. State remains through restart/readiness and any failure or interruption, then clears only after exact readiness. There is no identity-only rollback or destructive Git reset. It accepts readiness only when parsed JSON reports exact `status=ok`, `db=ok`, environment, and full release SHA. The app consumes this artifact from systemd and never invokes Git. Install the guard-aware unit bundle before relying on this contract. After dry-run, pass all four approval/checkpoint variables shown in `README.md`; never use `alembic upgrade head` for staging or production deployment.

Integration gate: leave `rh_20260720_runtime_metadata` directly after `pa_20260715_analytics_facts`; the combined tree reparents `sec_20260720_identity` onto `rh` and keeps security linear through `sec_20260720_device` before market migrations. Add explicit checkpoint transitions at each reviewed identity/device/quarantine pause. At the security checkpoint, extend the ACL policy with exact pending-registration, organization-join, refresh-session, and user admission/KYC table/column writes; do not let new columns inherit authority. The combined immutable unit manifest must add both environments' audited auth-maintenance service/timer bytes without changing installer code. Audit/status history is app-append-only. Seed, quarantine, market evidence, and provenance mutation require integration-owned trusted writers; the legacy market-signal CLI needs a separately approved operator identity, never blanket app DML. Preserve four Uvicorn workers; the shared SSE transport is built (market_event_dispatch outbox sequencer + org-bound hub; see docs/market-event-dispatch.md) and its staging activation stays behind the combined gates. Do not retain local-monitor's identity-only failure rollback, which would pair stale SHA with new bytes.
The local-monitor source does not modify or own the deploy helper, application
settings/readiness producer, runtime release transaction, database preflight,
or Alembic execution. Canonical runtime integration must atomically publish
matching environment/full-SHA runtime and monitor identity, keep both aligned
with the code that remains checked out after a failure, and validate the exact
four-key `/health/ready` corpus before declaring a restart successful. The
monitor contract and integration seams are in `deploy/monitor/README.md`.
There is no monitor-owned installer or activation path in this branch. Any
live deploy, even a dry run, requires separate operator authorization.

Read ARCHITECTURE.md before exploring the codebase.

## Project Overview

Backend API for Verdaxis -- a maritime intelligence and procurement platform. Handles fuel procurement (order book with BID/ASK matching), compliance auditing (EU ETS, FuelEU Maritime), port intelligence with geospatial data, AI copilot via Google Gemini, and a trade lifecycle (create -> confirm -> deliver -> pay).

**Repo:** `jonathanjie/verdaxis-backend`
**Runtime:** Python 3.10+ / FastAPI / PostgreSQL 17.9 with PostGIS 3.6.2 / SQLAlchemy 2 (async) / Alembic

## Development Commands

```bash
# Activate virtual environment from the current live tree
source ./venv/bin/activate

# Run the backend directly for local development
uvicorn app.main:app --reload --host 0.0.0.0 --port 8000

# Run all unit tests (no DB required, uses sqlite in-memory)
ENVIRONMENT=test RELEASE_SHA=test \
  JWT_SECRET=test-secret-key-for-testing-minimum-32-chars \
  DATABASE_URL="sqlite+aiosqlite:///:memory:" pytest tests/unit/ -v

# Integration/E2E is mutating and requires an explicitly started disposable
# server on a numeric-loopback ephemeral port. The source-tree producer is:
ENVIRONMENT=test \
RELEASE_SHA=$(git rev-parse HEAD) \
DISPOSABLE_TEST_TOKEN=<32-plus-character-token> \
venv/bin/python -m tests.disposable_server --port <ephemeral-port>

# Run pytest with the same token; without these flags collection skips the
# integration/E2E suites. Staging/production are never integration targets.
DISPOSABLE_ITEST_PASSWORD=... pytest tests/integration/ -v \
  --run-disposable-integration \
  --disposable-target-url http://127.0.0.1:<ephemeral-port> \
  --disposable-target-token <matching-32-plus-character-token>

# Build a disposable/development schema only; deployed environments use the
# source-attested literal checkpoint helper, never `upgrade head`
alembic upgrade head

# Create a new migration
alembic revision --autogenerate -m "description_here"

# Seed an explicitly attested staging/disposable database only; see README
SEED_DATABASE_URL=... SEED_TARGET_DATABASE=verdaxis_staging \
  SEED_RUNTIME_ENV=staging ALLOW_SEED_MUTATIONS=I_UNDERSTAND_SEED_MUTATIONS \
  python scripts/seed.py

# Full deploy script (on server, from prod/be or staging/be)
./scripts/deploy.sh --dry-run
./scripts/deploy.sh

# No-change unit provenance/install preflight for one independently promoted env
./scripts/install_systemd_units.sh --dry-run \
  --environment staging --source-ref <approved-staging-40-hex-sha>
```

## Deployment

**Server:** `verdaxis-prod@144.126.151.136`
**API:** `https://api.verdaxis.exchange/api` (Caddy reverse proxy -> `127.0.0.1:8000`; staging -> `127.0.0.1:8001`)
**Swagger:** `https://api.verdaxis.exchange/docs`
**Admin Panel:** `https://api.verdaxis.exchange/admin` (credentials from `ADMIN_USERNAME`/`ADMIN_PASSWORD` in `.env`)

The production frontend (`app.verdaxis.exchange`) is hosted on Vercel. Caddy
fronts the production API, staging API, and staging frontend; it does not serve
the production frontend build.

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
- Alembic compares columns, foreign keys, indexes, types, defaults, and comments. It excludes only explicitly enumerated PostGIS/system or documented legacy objects; targeted callbacks cover non-native enum storage and Python-owned defaults.
- All models MUST be imported in `app/models/__init__.py` or Alembic autogenerate will miss them

### Key Tables

| Table                | Purpose                                           |
| -------------------- | ------------------------------------------------- |
| `users`              | Auth, role (BUYER/SUPPLIER/ADMIN), status         |
| `organizations`      | Companies. Users belong to orgs. Has `domain` for auto-matching on registration |
| `orderbook_orders`   | Unified order book. side=BID (buyer wants) or ASK (seller offers) |
| `market_support_authorizations` | Exact, one-use customer authority and evidence for an assisted ASK |
| `staff_capability_assignments` | Revocable, expiring administrator capabilities for market support |
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
- `GET /health/ready` -- Bounded database readiness; use for off-host monitoring
- `GET /health/live` -- Process-only liveness
- `GET /health` -- Backward-compatible readiness alias

### Auth (`/api/auth`)
- `POST /api/auth/login` -- OAuth2 password form (email in `username` field) -> JWT
- `POST /api/auth/register` -- Returns `{status: "created", user}` or `{status: "requires_org", registration_token}` if org not found by email domain
- `POST /api/auth/register-with-org` -- Complete registration with new org creation
- `POST /api/auth/invitations/resolve` -- Resolve a valid admin-issued invitation secret
- `POST /api/auth/invitations/accept` -- Accept a pre-approved invitation, set a password, and issue a session
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
- `POST /api/ai/chat` -- Gemini chat
- `GET /api/matchmaking/suggestions` -- Match suggestions for org
- `POST /api/matchmaking/generate/{order_id}` -- Trigger match generation
- `PATCH /api/matchmaking/suggestions/{id}/dismiss`

### Admin Only
- `GET /api/auth/admin/invitations/organizations` -- Eligible real organizations for pre-approved invitations
- `POST /api/auth/admin/invitations` -- Create or rotate a single-use pre-approved invitation
- `PUT /api/auth/approve/{user_id}` -- Approve a verified account and queue a frozen, transition-scoped sign-in email for row-serialized post-commit delivery
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
- **Admin invitation flow:** An admin may prepare a buyer or supplier account in an existing approved real organization. The recipient accepts the single-use link, agrees to Terms/Privacy, sets a password, and receives a normal session without another approval. The admin is responsible for delivering the copied link securely to the intended recipient.
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

12. **JWT helpers use the declared `PyJWT` dependency.** Do not reintroduce undeclared `python-jose` imports in scripts or tests.

13. **`passlib` is unmaintained.** Last release was 2020 (v1.7.4). Depends on the deprecated `crypt` module removed in Python 3.13. Recommend migrating to direct `bcrypt` or `argon2-cffi`.

14. **Redis is an intentional Docker Compose dependency.** `docker-compose.yml` provisions Redis for the upcoming shared event/rate-limit work; keep the service and its configuration intact.

15. **Never use `--reload` in production Docker.** The `docker-compose.yml` `command:` used to include `--reload`, which caused uvicorn's `StatReload` to poll all 11,243 files in the bind-mounted `/app` directory (including `venv/` with 3,267 `.py` files and `postgres_data/`). This burned 243% CPU doing nothing. The fix: production compose uses plain `uvicorn` without `--reload`; dev uses `docker-compose.override.yml` with `--reload-dir` targeting only source directories.

16. **`.dockerignore` does NOT affect bind mounts.** The existing `.dockerignore` correctly excludes `venv/` and `postgres_data/` from `docker build` context, but the `volumes: - .:/app` bind mount bypasses it entirely. If using `--reload` with a bind mount, you MUST use `--reload-dir` to whitelist directories, not rely on `.dockerignore`.

17. **Frontend polls `/api/notifications` even when unauthenticated.** The frontend has a polling loop that hits `GET /api/notifications` and receives `401 Unauthorized` repeatedly. This generates log noise and wastes request cycles. The frontend should check auth state before starting the polling interval, or the polling should stop after receiving a 401.

18. **Production backend exposure is systemd-loopback only.** Production binds `127.0.0.1:8000` and staging binds `127.0.0.1:8001`; Caddy handles external traffic. Docker Compose remains a development/disposable topology and is not the live service manager.

## Environment Variables

Key variables in `.env` (loaded by `pydantic-settings`):

```
ENVIRONMENT=production          # development/test/staging/production
RELEASE_SHA=...                 # Full 40-hex SHA required in staging/production
DATABASE_HOST=verdaxis-db       # "localhost" for non-Docker
DATABASE_PORT=5432
DATABASE_NAME=verdaxis
DATABASE_USER=verdaxis_app       # prod exact; staging is verdaxis_app_staging
DATABASE_PASSWORD=...
DATABASE_URL=                   # Optional override (e.g. sqlite+aiosqlite:///:memory: for tests)
MIGRATOR_DATABASE_URL=          # Required deployed: verdaxis_migrator[_staging], same DB, explicit non-placeholder password
JWT_SECRET=...                  # MUST be strong in production
GEMINI_API_KEY=...              # Optional, AI features degrade gracefully without it
ADMIN_USERNAME=...              # For /admin panel login
ADMIN_PASSWORD=...
ENABLE_AUTH_BYPASS=false        # Never true in production
DB_POOL_SIZE=2                  # Per-worker SQLAlchemy pool
DB_MAX_OVERFLOW=1               # Per-worker overflow; see docs/runtime-hardening.md
UVICORN_WORKERS=4               # Authoritative systemd/config/pool worker count
DB_SERVICE_COUNT=2              # Immutable deployed prod + staging topology
DB_MAX_CONNECTIONS=100
DB_RESERVED_CONNECTIONS=20      # Deployed minimum maintenance reserve
DB_STATEMENT_TIMEOUT_MS=30000
DB_LOCK_TIMEOUT_MS=3000
DB_IDLE_IN_TRANSACTION_SESSION_TIMEOUT_MS=60000
MIGRATOR_STATEMENT_TIMEOUT_MS=300000
MIGRATOR_LOCK_TIMEOUT_MS=30000
MIGRATOR_IDLE_IN_TRANSACTION_SESSION_TIMEOUT_MS=300000
KYC_MAX_FILE_BYTES=10485760   # 10 MiB per document
KYC_MAX_TOTAL_BYTES=20971520  # 20 MiB per KYC request
HEALTH_READINESS_TIMEOUT_SECONDS=2
BACKEND_CORS_ORIGINS=          # Omit for exact environment-specific allowlist
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

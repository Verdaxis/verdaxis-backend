# AI Context: Verdaxis Backend

> **Purpose**: This file provides structural and contextual information for AI agents working on this codebase.

## Project Context

Verdaxis is a maritime platform designed to modernize fuel procurement and compliance. The backend serves as the central intelligence hub, aggregation data from geospatial sources, compliance registries, and market participants.

## Architecture Guidelines

- **Framework**: FastAPI. Designed for high performance and async I/O.
- **Database**:
  - Uses **SQLAlchemy v2** with `AsyncSession`.
  - **PostGIS** is used for geospatial queries (e.g., finding nearby ports).
  - **Alembic** handles schema migrations.
- **Asynchronous**: All I/O bound operations (DB, API calls) must be `async`.

## Key Directory Structure

| Directory | Purpose |
|Path|Description|
|---|---|
| `app/routers/` | **Controllers**. Define endpoints. Logic should be minimal here, delegating to services/CRUD. |
| `app/models/` | **DB Code**. SQLAlchemy ORM definitions. Maps to DB tables. |
| `app/schemas/` | **DTOs**. Pydantic models for data validation and sterilization. |
| `app/services/` | **Business Logic**. Complex operations, AI calls, and third-party integrations. |
| `app/core/` | **Config**. Settings, Security, and Database connection setup. |

## Core Data Models

- **User / Organization**: Users belong to Organizations (Buyer or Supplier).
- **Vessel**: Core asset for Buyers. Has geospatial/tracking data.
- **Port**: Geospatial entity. Uses PostGIS `Geometry` types.
- **QuoteRequest**: Central entity for the Marketplace. Links Buyers, Vessels, and Ports to Suppliers.
- **ComplianceRecord**: Stores EU ETS/FuelEU verification data.

## Coding Conventions

1.  **Strict Typing**: Use Python type hints everywhere.
2.  **Pydantic**: Use `ConfigDict(from_attributes=True)` (formerly `orm_mode`) for DB-to-JSON conversion.
3.  **Dependency Injection**: Use `Depends()` for DB sessions (`get_db`) and current user (`get_current_user`).
4.  **Error Handling**: Raise `HTTPException` with clear detail strings.

## AI Integration

- **Router**: `app/routers/ai.py`
- **Usage**: The AI service (Google GenAI) is used for:
  - Analyzing market trends from text.
  - Optimizing fuel procurement strategies.
  - Summarizing compliance reports.

## Deployment Access

- **VPS Host**: 144.126.151.136
- **User**: verdaxis-prod
- **Command**: `ssh verdaxis-prod@144.126.151.136`
- **Backend API**: http://144.126.151.136:8000/
- **Swagger Docs**: http://144.126.151.136:8000/docs

## Test Credentials

| Role         | Email              | Password    |
| ------------ | ------------------ | ----------- |
| **Admin**    | admin@verdaxis.com | ***REMOVED***    |
| **Buyer**    | buyer@demo.com     | buyer123    |
| **Supplier** | supplier@demo.com  | supplier123 |

### Authentication Endpoints

- `POST /api/auth/login` - Login and get JWT token
- `POST /api/auth/register` - Register new user (status: PENDING by default)
- `GET /api/auth/me` - Get current user info
- `PUT /api/auth/approve/{user_id}` - Admin approves pending user
- `PUT /api/auth/switch-role/{BUYER|SUPPLIER|ADMIN}` - Admin switches role for testing

---

## Git Workflow

### Branches

- **`main`**: Development branch. All active development happens here.
- **`prod`**: Production branch. Stable releases only.

### Commands (Local)

```bash
# Push to main (development)
git add -A && git commit -m "your message" && git push origin main

# Deploy to production
git checkout prod
git merge main
git push origin prod
git checkout main
```

### Commands (Server)

```bash
# SSH to server
ssh verdaxis-prod@144.126.151.136

# Pull latest and restart
cd ~/verdaxis-backend
git pull origin main
docker compose down && docker compose up -d --build
```

---

## Deployment

### One-Command Deploy (from local)

```bash
# From Verdaxis root directory
./scripts/push-deploy.sh                 # Deploy both frontend and backend
./scripts/push-deploy.sh --backend-only  # Deploy backend only
./scripts/push-deploy.sh --with-tests    # Run tests before deploying
```

### Manual Server Deploy

```bash
# SSH to server
ssh verdaxis-prod@144.126.151.136

# Deploy backend
cd ~/verdaxis-backend && bash scripts/deploy.sh
```

### Docker Commands (on server)

```bash
docker compose up -d --build    # Start/rebuild containers
docker compose down             # Stop containers
docker compose logs -f backend  # View backend logs
docker exec -it verdaxis-backend bash  # Shell into container
```

---

## Testing

### Run All Tests

```bash
# Activate virtual environment
source venv/bin/activate

# Run all tests
pytest tests/ -v

# Run unit tests only (no Docker required)
pytest tests/unit/ -v

# Run integration tests (requires Docker backend running)
pytest tests/integration/ -v
```

### Test Structure

| Directory            | Purpose                                             |
| -------------------- | --------------------------------------------------- |
| `tests/unit/`        | Unit tests for isolated functions (security, utils) |
| `tests/integration/` | API endpoint tests against running Docker backend   |
| `tests/conftest.py`  | Pytest fixtures and configuration                   |

### Running Tests Against Remote

```bash
# Test against production server
TEST_API_URL=http://144.126.151.136:8000 pytest tests/integration/ -v
```

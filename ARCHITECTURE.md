# Architecture

> FastAPI + SQLAlchemy 2 (async) + PostgreSQL 15/PostGIS + Alembic + Pydantic v2
> PyJWT (HS256) + bcrypt + slowapi rate limiting + structlog JSON logging

## File Map

```
app/
  main.py                       # FastAPI app, CORS, structlog, request correlation IDs, rate limiter
  config.py                     # Pydantic Settings — env vars, JWT config, OAuth, auto-matching toggle
  database.py                   # AsyncSession factory (asyncpg), connection pooling (20/40), SQLite guard
  admin.py                      # SQLAdmin panel at /admin
  rate_limit.py                 # slowapi Limiter singleton (key=remote_address)
  core/
    security.py                 # PyJWT + bcrypt — create_access_token, create_refresh_token, decode_token
  models/
    __init__.py                 # Imports all models for Alembic autogenerate
    user.py                     # User (with password_changed_at, oauth_provider), Organization, enums
    port.py                     # Port (PostGIS), PortIntelligence, Vessel
    marketplace.py              # InventoryItem, FuelType enum
    orderbook.py                # OrderBookOrder (BID/ASK), Trade, canonical availability_window strings, enums (OrderSide, TradeStatus)
    orders.py                   # Commission (legacy match_id + trade_id FKs)
    matchmaking.py              # MatchSuggestion
    watchlist.py                 # Watchlist, typed targets, event feed for Market Radar
    notification.py             # Notification, NotificationType (11 types incl trade events)
    compliance.py               # TraceabilityEvent, ComplianceLedger
    producer.py                 # ProducerProject (PostGIS, GENA import)
    audit.py                    # AuditLog (JSONB changes, indexed action/resource/timestamp)
  routers/
    auth_simple.py              # JWT auth — login/register, cookie-backed refresh rotation, password change, /me, RBAC
    oauth.py                    # [feature branch] Google + Microsoft OIDC via Authlib
    orderbook.py                # Order/listing CRUD, supplier ASK template endpoint, certification guardrails
    trades.py                   # Trade lifecycle — create/confirm/decline/deliver/pay + SSE events
    matchmaking.py              # Match suggestions — generate, list, dismiss
    price_discovery.py          # Public price ticker + daily VWAP reference prices
    stream.py                   # SSE endpoints — /stream/prices, /stream/orderbook, /stream/trades
    compliance_api.py           # Compliance scoring — fleet scores, vessel scores, what-if scenarios
    admin_analytics.py          # Platform analytics — overview stats + daily breakdown (ADMIN only)
    availability.py             # Fuel availability by port
    demand.py                   # Anonymized BID demand signals
    producers.py                # Producer project list (map data)
    notifications.py            # User notification CRUD
    inventory.py                # Supplier inventory + publish-to-ASK
    ports.py                    # Port data with PostGIS
    vessels.py                  # Vessel data (org-scoped)
    compliance.py               # Compliance ledger (legacy)
    ai.py                       # Gemini AI chat proxy
    orders.py                   # Admin commission management
    audit.py                    # Admin audit log query
    dashboard.py                # System health metrics
  schemas/
    user.py                     # UserCreate (min 8 chars pw), UserResponse, PasswordChangeRequest
    organization.py             # OrganizationCreate/Response
    orderbook.py                # Order/Trade schemas, supplier metadata pack, ASK template response, canonical availability window validation
    [others unchanged]
  services/
    event_bus.py                # AsyncIO pub/sub — per-channel queues, 200 subscriber cap, backpressure
    matching_engine.py          # Match-on-insert — price-time priority within canonical market identity, partial fills, auto-confirm
    benchmarks.py               # Benchmark lookup keyed by market_product + delivery_point + availability_window
    availability_windows.py     # Canonical availability code parsing, sorting, display labels, legacy alias normalization
    compliance_scoring.py       # Pure function scoring — FuelEU/ETS/CII, 9 fuels, scenario engine
    audit_service.py            # record_audit() — async audit logging
    ai_service.py               # Gemini chat + document analysis (stub)
    matchmaking.py              # Score-based BID/ASK matching (0-100)
    watchlists.py               # Market Radar helpers: default container, typed targets, slice summaries
    watchlist_events.py         # Slice/pin event emission from order lifecycle changes
    ci_pricing.py               # Carbon intensity adjusted pricing
  middleware/
    rbac.py                     # require_role() factory — FastAPI dependency for role-based access

tests/unit/                     # 155 tests (auth, matching, compliance, events, pricing, schemas)
tests/integration/              # Auth hardening, trade lifecycle, orderbook E2E
  alembic/versions/               # Migrations incl. canonical availability-window rewrite + defaults
```

## Key Patterns

- **Match-on-insert:** `POST /orderbook` → `db.flush()` → `match_order()` → `db.commit()` (atomic); executable matches now require exact `product + delivery_point + availability_window`
- **Supplier ASK invariants:** ASK creation/update requires explicit `certification_declared=true` plus a non-empty `certification_scheme`; `GET /orderbook/my/latest-ask-template` returns safe defaults for the next listing and resets off-spec state
- **SSE broadcasting:** `event_bus.publish(channel, event_type, data)` → subscribers via AsyncIO queues
- **Compliance scoring:** Pure function `calculate_compliance_score()` — no DB, 100% testable
- **Dual-token JWT:** 15-min access + 7-day refresh, `password_changed_at` for stateless invalidation
- **Cookie-backed refresh:** refresh token is also rotated through an HttpOnly `refresh_token` cookie scoped to `/api/auth`, while access tokens remain bearer tokens
- **Rate limiting:** slowapi per-route (5/min login, 3/min password, 60/min prices, 30/min reference)
- **Availability windows:** Persist canonical codes (`SPOT`, `YYYY-MM`, `YYYY-QN`, legacy-compatible `YYYY-CAL`); UI-relative labels like `M+1` must be resolved before persistence
- **Green-fuels market model:** Benchmarks and default matchmaking key on `market_product + delivery_point + availability_window`; supplier sustainability/compliance fields stay out of the hard market key
- **Market Radar watchlists:** Watchlists are observer-only. Typed targets store either canonical slices or pinned order snapshots, and order create/update/cancel paths emit slice/pin events without feeding core matchmaking.

## Revenue Streams

1. **Transaction fees (0.5%)** — commission_amount_usd on Trade model
2. **Compliance SaaS ($200-500/vessel/mo)** — /compliance/fleet, /compliance/scenario
3. **Data products ($1K-5K/seat/mo)** — /prices/reference (daily VWAP)
4. **Platform analytics** — /admin/analytics/overview, /admin/analytics/daily

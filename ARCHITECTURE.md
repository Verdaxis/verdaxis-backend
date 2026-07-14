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
    security.py                 # PyJWT + bcrypt — access/refresh/stream token creation, decode_token
  models/
    __init__.py                 # Imports all models for Alembic autogenerate
    user.py                     # User (password_changed_at, must_change_password), Organization, enums
    port.py                     # Port (PostGIS), PortIntelligence, Vessel
    marketplace.py              # InventoryItem, FuelType enum
    orderbook.py                # OrderBookOrder (BID/ASK), Trade, canonical availability_window strings, enums (OrderSide, TradeStatus)
    live_slice_benchmark.py     # Persisted same-side slice VWAP aggregates for fast marketplace benchmark reads
    orders.py                   # Commission (legacy match_id + trade_id FKs)
    matchmaking.py              # MatchSuggestion
    watchlist.py                 # Watchlist, typed targets, event feed for Market Radar
    notification.py             # Notification, NotificationType (11 types incl trade events)
    user_preference.py          # Per-user JSON preferences by namespace (market_watch, notifications, tutorial)
    compliance.py               # TraceabilityEvent, ComplianceLedger
    producer.py                 # ProducerProject (PostGIS, GENA import)
    audit.py                    # AuditLog (JSONB changes, indexed action/resource/timestamp)
  routers/
    auth_simple.py              # JWT auth — login/register, cookie-backed refresh rotation, password change, /me, RBAC
    orderbook.py                # Order/listing CRUD, supplier ASK template endpoint, certification guardrails
    trades.py                   # Trade lifecycle — create/confirm/decline/deliver/pay + SSE events
    matchmaking.py              # Match suggestions — generate, list, dismiss
    price_discovery.py          # Public price ticker + daily VWAP reference prices
    trade_tape.py               # Public anonymized 7-day confirmed trade tape with exact delivery-point filtering
    curves.py                   # Forward curve data products plus legacy board/table/slice endpoints
    stream.py                   # SSE endpoints — /stream/prices, /stream/orderbook, /stream/trades
    compliance_api.py           # Compliance scoring — fleet scores, vessel scores, what-if scenarios
    admin_analytics.py          # Platform and product-usage aggregates (ADMIN only; Umami degrades independently)
    availability.py             # Fuel availability by port
    demand.py                   # Anonymized BID demand signals
    producers.py                # Producer project list (map data)
    notifications.py            # User notification CRUD
    preferences.py              # Authenticated server-persisted user preferences
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
    preferences.py              # Strict namespace schemas for user preferences
    organization.py             # OrganizationCreate/Response
    orderbook.py                # Order/Trade schemas, price summaries, supplier metadata pack, ASK template response, canonical availability window validation
    market_activity.py          # Shared source/scope/demo-status provenance enums for market data
    behavioral_analytics.py     # Typed privacy-bounded admin product-usage response
    [others unchanged]
  services/
    event_bus.py                # AsyncIO pub/sub — per-channel queues, 200 subscriber cap, backpressure
    matching_engine.py          # Match-on-insert — price-time priority within canonical market identity, partial fills, auto-confirm
    benchmarks.py               # External/manual benchmark lookup keyed by market_product + delivery_point + availability_window
    forward_curve_market_slices.py # Canonical public Forward Curve table/slice read models and label policy
    market_signal_ingestion.py    # Trusted signal importer: validate -> verified ingestion runs whose rows classify REAL (see docs/market-signal-ingestion.md)
    live_benchmarks.py          # Persisted live same-side slice VWAP rebuilds + read-through fallback
    availability_windows.py     # Canonical availability code parsing, sorting, display labels, legacy alias normalization
    compliance_scoring.py       # Pure function scoring — FuelEU/ETS/CII, 9 fuels, scenario engine
    audit_service.py            # record_audit() — non-committing async audit logging (call inside the caller's transaction, before its commit)
    audit_actions.py            # THE source of truth for covered audit actions (registry of constants; meta-test forbids string literals at call sites)
    ai_service.py               # Gemini chat + document analysis (stub)
    matchmaking.py              # Score-based BID/ASK matching (0-100)
    watchlists.py               # Market Radar helpers: default container, typed targets, slice summaries
    watchlist_events.py         # Slice/pin event emission from order lifecycle changes
    behavioral_analytics.py     # Optional async Umami client, token/aggregate caches, post-commit conversion events
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
- **SSE broadcasting:** `event_bus.publish(channel, event_type, data)` → subscribers via AsyncIO queues; order/trade payloads are append-only enriched with market source/scope/demo provenance
- **Compliance scoring:** Pure function `calculate_compliance_score()` — no DB, 100% testable
- **JWT auth:** 15-min access + 7-day refresh, plus 60-second `type="stream"` tokens from `/auth/stream-token` for SSE query-param auth. Ordinary API auth only accepts access tokens; activity SSE query auth only accepts stream tokens.
- **Cookie-backed refresh:** refresh token is also rotated through an HttpOnly `refresh_token` cookie scoped to `/api/auth`, while access tokens remain bearer tokens
- **Rate limiting:** slowapi per-route (5/min login, 3/min password, 60/min prices, 30/min reference)
- **Availability windows:** Persist canonical codes (`SPOT`, `YYYY-MM`, `YYYY-QN`, legacy-compatible `YYYY-CAL`); UI-relative labels like `M+1` must be resolved before persistence
- **Green-fuels market model:** Matching and live slice benchmarks key on `side + market_product + delivery_point + availability_window`; supplier sustainability/compliance fields stay out of the hard market key
- **Market provenance contract:** Market-data responses use shared `source_kind`, `scope`, and `demo_status` fields. Aggregate data exposes real/demo/unknown counts; unknown contributors remain `UNKNOWN` rather than being collapsed into real/demo/mixed.
- **Forward Curve monitoring:** `/curves/forward/table` and `/curves/forward/slice` use `forward_curve_market_slices.py` as the canonical public read model for approved `market_product + delivery_point + availability_window` slices. Products aggregate by canonical market product, delivery points are restricted to the approved trading ports, and public cells expose server-owned label policy plus redacted source/demo/staleness fields. `/curves/forward/board` remains for older clients.
- **Real signal ingestion:** `scripts/ingest_market_signals.py` (staging-guarded, dry-run default) + `services/market_signal_ingestion.py` produce verified `market_signal_ingestion_runs` whose rows satisfy the trust predicate in `forward_monitoring.py` and render as REAL. CSV formats, staleness semantics, redaction invariants, and rollback SQL: `docs/market-signal-ingestion.md`.
- **Price discovery provenance:** `/prices` 24h summaries classify confirmed trade buckets as `CONFIRMED_TRADE`, `DEMO_SEED`, `MIXED_SOURCE`, or `UNKNOWN` using the same demo organization rules as trade tape.
- **Trade tape scope:** `/trade-tape` returns anonymized confirmed trade prints for the last 7 days. Exact delivery-point history is available only when clients filter by `delivery_point_id` and entries return `scope="DELIVERY_POINT"` with delivery-point fields. `provenance_kind` is legacy-compatible; new clients should prefer `source_kind`/`demo_status` when present on newer surfaces.
- **Market Radar watchlists:** Watchlists are observer-only. Typed targets store either canonical slices or pinned order snapshots, and order create/update/cancel paths emit slice/pin events without feeding core matchmaking. New event payloads carry provenance at emission time; legacy events return `UNKNOWN` rather than doing response-time order lookups.
- **Behavioral analytics:** Umami is an optional failure-isolated dependency. `GET /admin/analytics/product-usage?days=7|30|90` combines bounded Umami aggregates with authoritative UTC-period database counts. Server conversion events and the documented browser reporting taxonomy use separate allowlists. Registration, organization, order, and trade events are scheduled only after commits, carry bounded originating request metadata for Umami bot classification/environment attribution, and use a strict property allowlist; collector drops/failures never alter endpoint response contracts. Aggregate successes cache for at most five minutes and failures for at most 30 seconds. `totaltime / visits` is exposed only as average session duration, not active engagement. See `docs/behavioral-analytics-contract.md`.

## Revenue Streams

1. **Transaction fees (0.5%)** — commission_amount_usd on Trade model
2. **Compliance SaaS ($200-500/vessel/mo)** — /compliance/fleet, /compliance/scenario
3. **Data products ($1K-5K/seat/mo)** — /prices/reference (daily VWAP)
4. **Platform analytics** — /admin/analytics/overview, /admin/analytics/daily, /admin/analytics/product-usage

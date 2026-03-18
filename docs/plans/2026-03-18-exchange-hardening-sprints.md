---
title: Verdaxis Exchange Hardening — Sprints 5–8
id: exchange-hardening-sprints
edition: 1.0
status: approved
created: 2026-03-18
---

# Verdaxis Exchange Hardening — Sprint Planning (Sprints 5–8)

## Overview

Four sprints extending the Verdaxis backend from a functional order book into a
production-grade institutional exchange. Builds on the 155-test, 40-endpoint
foundation delivered in Sprints 1–4.

**Total stories:** 26
**Total points:** ~94

---

## Sprint 5: Trading Sophistication

**Goal:** Support advanced order types (STOP, STOP_LIMIT, AON, OCO) and order
amendments so institutional traders have the tools they need.
**Stories:** 8 | **Points:** 29

---

### STORY-S5-001 — Order type enum + model fields

**Points:** 3

Add `STOP`, `STOP_LIMIT`, `AON`, and `OCO` to the order type system. Extend
`OrderBookOrder` with the fields required to carry stop and linked-order
semantics without breaking existing LIMIT orders. A new `OrderType` enum is
introduced; existing orders default to `LIMIT`.

**Files to create/modify:**
- `app/models/orderbook.py` — add `OrderType` enum (`LIMIT`, `STOP`, `STOP_LIMIT`, `AON`, `OCO`); add `order_type`, `stop_price`, `linked_order_id` columns to `OrderBookOrder`
- `app/schemas/orderbook.py` — extend `OrderBookOrderCreate` and `OrderBookOrderResponse` with the new fields
- `alembic/versions/<rev>_add_order_type_fields.py` — migration adding the three columns (nullable, `order_type` defaults to `"LIMIT"`)
- `tests/unit/test_orderbook_schemas.py` — schema validation tests for new fields

**Acceptance criteria:**
- `OrderType` enum present in `app/models/orderbook.py` with all four new values
- `OrderBookOrder` table has `order_type VARCHAR`, `stop_price NUMERIC(10,2)`, `linked_order_id UUID` columns after `alembic upgrade head`
- Creating an order without `order_type` yields `order_type = "LIMIT"` (no regression)
- `stop_price` is required when `order_type` is `STOP` or `STOP_LIMIT` (Pydantic validator enforces)
- Unit tests cover valid and invalid field combinations; all existing tests remain green

---

### STORY-S5-002 — TRIGGERED status in OrderBookStatus

**Points:** 1

Add `TRIGGERED` to `OrderBookStatus` so stop orders have a distinct lifecycle
state between resting (`OPEN`) and active (`OPEN` after trigger converts them
to a live limit order). This is a small enum extension but must be consistent
across model, schema, and any status-filtering queries.

**Files to create/modify:**
- `app/models/orderbook.py` — add `TRIGGERED = "TRIGGERED"` to `OrderBookStatus`
- `app/schemas/orderbook.py` — mirror in schema enum
- `tests/unit/test_orderbook_schemas.py` — assert `TRIGGERED` is a valid status value

**Acceptance criteria:**
- `OrderBookStatus.TRIGGERED` exists and serialises to the string `"TRIGGERED"`
- `GET /api/orderbook` filter queries that accept a `status` param do not break on `TRIGGERED`
- No existing tests broken by the new enum member
- Database enum column accepts `"TRIGGERED"` after migration (or is `VARCHAR` — confirm `native_enum=False` pattern already used)

---

### STORY-S5-003 — StopEngine service

**Points:** 5

Implement a `StopEngine` service that scans resting STOP and STOP_LIMIT orders
and converts them to live LIMIT orders when the market price crosses their
`stop_price`. The engine is designed to be called from trade execution so it
fires on every fill event.

**Files to create/modify:**
- `app/services/stop_engine.py` — new file; `async def trigger_stops(db: AsyncSession, fuel_type: str, last_price: Decimal) -> list[OrderBookOrder]`
- `tests/unit/test_stop_engine.py` — new file; unit tests using SQLite in-memory

**Acceptance criteria:**
- BUY stop (`side=BID`, `stop_price`) triggers when `last_price >= stop_price`
- SELL stop (`side=ASK`, `stop_price`) triggers when `last_price <= stop_price`
- On trigger, `order_type` is changed to `LIMIT` and status transitions from `TRIGGERED` back to `OPEN` so the matching engine will pick it up
- STOP_LIMIT follows the same trigger rule but retains a limit `price_per_mt_usd` after activation
- Returns only newly triggered orders (idempotent on already-OPEN orders)
- Unit tests cover BUY trigger, SELL trigger, no-trigger (price hasn't crossed), and STOP_LIMIT activation

---

### STORY-S5-004 — AON match logic

**Points:** 3

Extend `matching_engine.py` to honour All-Or-Nothing semantics. An AON order
must only match if the full `remaining_quantity_mt` can be filled in a single
pass; partial fills must be rejected for that order.

**Files to create/modify:**
- `app/services/matching_engine.py` — add AON guard before trade creation inside the crossing loop
- `tests/unit/test_matching_engine.py` — new test cases for AON full-fill and AON no-fill-when-partial

**Acceptance criteria:**
- AON order does not partially fill even when a crossing order has sufficient liquidity to partially match
- AON order fills completely when a single resting order satisfies the full quantity
- Non-AON orders are not affected (existing tests stay green)
- A resting AON order can be the counterparty to an aggressor that covers its full quantity
- Tests confirm zero trades when AON cannot be filled in full

---

### STORY-S5-005 — OCO paired orders

**Points:** 5

Implement One-Cancels-Other order pairs. When one leg of an OCO pair fills or
is manually cancelled, the other leg must be cancelled atomically. A new
endpoint `POST /api/orderbook/oco` creates both legs in a single transaction.

**Files to create/modify:**
- `app/routers/orderbook.py` — add `POST /api/orderbook/oco` route (static route, before `/{order_id}`)
- `app/schemas/orderbook.py` — `OCOCreateRequest` (two `OrderBookOrderCreate` legs), `OCOResponse`
- `app/services/matching_engine.py` — add `cancel_oco_sibling(db, filled_order)` called after any fill that sets `linked_order_id` on the filled order
- `tests/integration/test_orderbook.py` — OCO creation and sibling-cancel integration test

**Acceptance criteria:**
- `POST /api/orderbook/oco` creates two orders with `linked_order_id` pointing to each other; returns both in response
- When leg A fills, leg B is set to `CANCELLED` in the same DB transaction
- When leg A is manually deleted (`DELETE /api/orderbook/{id}`), leg B is also cancelled
- Both legs must belong to the same `organization_id` (validated server-side; 422 otherwise)
- OCO endpoint requires authentication (BUYER or SUPPLIER role)

---

### STORY-S5-006 — Order amendment with audit trail

**Points:** 5

Allow traders to amend `price_per_mt_usd` or `quantity_mt` on an open order
via `PATCH /api/orderbook/{id}`. Each amendment is recorded in a new
`OrderAuditLog` model to satisfy best-execution audit requirements.

**Files to create/modify:**
- `app/models/orderbook.py` — add `OrderAuditLog` model (`id`, `order_id FK`, `field_changed`, `old_value`, `new_value`, `amended_by FK users.id`, `amended_at`)
- `app/routers/orderbook.py` — add `PATCH /api/orderbook/{id}` handler (after existing `PUT`)
- `app/schemas/orderbook.py` — `OrderAmendRequest` (optional `price_per_mt_usd`, optional `quantity_mt`), `OrderAuditLogResponse`
- `alembic/versions/<rev>_add_order_audit_log.py` — migration
- `tests/integration/test_orderbook.py` — amendment happy path and unauthorised-amend (403) tests

**Acceptance criteria:**
- `PATCH /api/orderbook/{id}` accepts `price_per_mt_usd` and/or `quantity_mt`; updates order and writes one `OrderAuditLog` row per changed field
- Only the order's owner can amend it (403 otherwise)
- Amendment is only allowed for `OPEN` or `PARTIALLY_FILLED` orders (409 for other statuses)
- `remaining_quantity_mt` is adjusted proportionally when `quantity_mt` is reduced (but never below already-filled quantity)
- Audit log entries are readable via `GET /api/orderbook/{id}/audit` (new endpoint, same router)

---

### STORY-S5-007 — Hook StopEngine into trade execution

**Points:** 2

Wire the `StopEngine` into the post-fill path so every confirmed trade
automatically checks for newly triggerable stops on the same `fuel_type`.

**Files to create/modify:**
- `app/routers/orderbook.py` — call `trigger_stops(db, fuel_type, trade_price)` after `match_order()` returns trades
- `app/routers/trades.py` — same call in the manual trade-hit path (`POST /api/trades/`)
- `tests/unit/test_stop_engine.py` — integration-style test: place stop order, simulate fill, assert order transitions to OPEN

**Acceptance criteria:**
- After any trade is matched on a given `fuel_type`, `trigger_stops` is called with the trade's `price_per_mt_usd` as `last_price`
- Newly triggered orders transition to `OPEN` and are immediately eligible for the next match cycle
- No extra DB roundtrips when zero stop orders exist for that fuel type
- Existing order-placement tests are unaffected

---

### STORY-S5-008 — Frontend order type UI

**Points:** 5

Add order type selection to the order placement form so traders can create
STOP, STOP_LIMIT, and AON orders from the UI. OCO entry uses a dedicated
two-leg wizard.

**Files to create/modify (frontend repo):**
- `src/components/OrderPlaceModal.tsx` — add `order_type` selector dropdown; conditionally show `stop_price` field when type is STOP or STOP_LIMIT
- `src/components/OCOWizard.tsx` — new two-step wizard component for OCO order creation
- `src/lib/api.ts` — add `placeOCOOrder(legs)` calling `POST /api/orderbook/oco`
- `src/types/orderbook.ts` — extend `OrderType` union type

**Acceptance criteria:**
- Order type dropdown shows LIMIT (default), STOP, STOP_LIMIT, AON options
- `stop_price` input appears only when STOP or STOP_LIMIT is selected; form submission blocked if blank
- OCO wizard creates both legs and shows confirmation with both order IDs
- Existing LIMIT order flow is visually unchanged
- Form validates `stop_price > 0` client-side before API call

---

## Sprint 6: Market Integrity

**Goal:** Detect and surface market manipulation patterns (wash trading,
spoofing, front-running, layering, marking-the-close) so compliance officers
can review and act.
**Stories:** 6 | **Points:** 24

---

### STORY-S6-001 — SurveillanceEvent model + enums

**Points:** 2

Define the persistence layer for all market surveillance alerts. A single
`SurveillanceEvent` model captures the detected pattern, severity, status,
and links to the implicated orders and organisations.

**Files to create/modify:**
- `app/models/surveillance.py` — new file; `SurveillanceType`, `SurveillanceSeverity`, `SurveillanceStatus` enums; `SurveillanceEvent` ORM model (`id`, `event_type`, `severity`, `status`, `implicated_org_id FK`, `implicated_order_ids JSON`, `implicated_trade_ids JSON`, `description`, `detected_at`, `reviewed_at`, `reviewed_by FK users.id`, `reviewer_notes`)
- `app/models/__init__.py` — import `SurveillanceEvent`
- `alembic/versions/<rev>_add_surveillance_events.py` — migration
- `tests/unit/test_surveillance_model.py` — new file; model instantiation and enum value tests

**Acceptance criteria:**
- `SurveillanceType` contains at minimum: `WASH_TRADING`, `SPOOFING`, `FRONT_RUNNING`, `MARKING_THE_CLOSE`, `LAYERING`
- `SurveillanceSeverity` contains: `LOW`, `MEDIUM`, `HIGH`, `CRITICAL`
- `SurveillanceStatus` contains: `OPEN`, `UNDER_REVIEW`, `DISMISSED`, `ESCALATED`
- `SurveillanceEvent` table is created by migration with correct FK constraints
- `implicated_order_ids` and `implicated_trade_ids` are stored as JSON arrays of UUID strings

---

### STORY-S6-002 — Wash trading detector

**Points:** 3

Detect trades where `buyer_id == seller_id` at the organisation level. This is
the simplest and most egregious manipulation pattern; it must be checked on
every trade creation.

**Files to create/modify:**
- `app/services/surveillance.py` — new file; `async def check_wash_trade(db, trade: Trade) -> SurveillanceEvent | None`
- `app/routers/trades.py` and `app/routers/orderbook.py` — call `check_wash_trade` after each trade is created (self-trade prevention in the matching engine blocks same-org same-call-stack matches, but OTC and future manual paths need surveillance coverage)
- `tests/unit/test_surveillance.py` — new file; unit tests for wash trade detection

**Acceptance criteria:**
- Detector creates a `SurveillanceEvent` with `event_type=WASH_TRADING`, `severity=CRITICAL` when `trade.buyer_id == trade.seller_id`
- Detector returns `None` when buyer and seller are different organisations
- Matching engine's existing self-trade prevention (`organization_id != new_order.organization_id`) remains in place as a hard block; surveillance is a secondary observability layer
- Unit tests cover detected case, clean case, and edge case of null org IDs

---

### STORY-S6-003 — Spoofing detector

**Points:** 3

Flag participants who place large orders and cancel them rapidly before
execution. Spoofing distorts the visible order book to manipulate perceived
supply/demand.

**Files to create/modify:**
- `app/services/surveillance.py` — add `async def check_spoofing(db, cancelled_order: OrderBookOrder) -> SurveillanceEvent | None`; threshold: same org cancels ≥ 3 orders ≥ 100 MT within a 60-second window without any fill
- `app/routers/orderbook.py` — call `check_spoofing` in the `DELETE /api/orderbook/{id}` handler after status is set to `CANCELLED`
- `tests/unit/test_surveillance.py` — spoofing detection tests (at-threshold, below-threshold, filled-orders excluded)

**Acceptance criteria:**
- Alert fires when an organisation cancels ≥ 3 orders of ≥ 100 MT in ≤ 60 seconds where none were partially filled
- Alert severity is `HIGH`; `implicated_order_ids` lists all cancelled orders in the window
- Cancellations with any fill (`remaining_quantity_mt < quantity_mt`) are excluded from the pattern count
- Detector is idempotent — duplicate alerts are not raised if the same window is re-evaluated
- Configurable thresholds (count, size, window) are module-level constants, not magic numbers

---

### STORY-S6-004 — FrontRunning, MarkingTheClose, and Layering detectors

**Points:** 5

Implement three additional detectors. Front-running flags orders placed by an
org immediately before large orders from a counterparty they could have
foreknowledge of. Marking-the-close flags abnormal price-moving trades in the
final minutes of a session window. Layering flags a stack of same-side orders
at incrementing prices that are cancelled together.

**Files to create/modify:**
- `app/services/surveillance.py` — add `check_front_running`, `check_marking_the_close`, `check_layering` functions
- `app/services/surveillance.py` — add `async def run_all_detectors(db, trade: Trade | None, order: OrderBookOrder | None)` orchestrator that calls applicable detectors
- `tests/unit/test_surveillance.py` — tests for each new detector; both fire and no-fire cases

**Acceptance criteria:**
- `check_front_running`: flags when org A places an order within 5 seconds before a large order (≥ 500 MT) from org B in the same fuel_type/region, and org A has previously traded with org B (indicates information asymmetry); severity `HIGH`
- `check_marking_the_close`: flags trades that move the VWAP by ≥ 3% in the final 5-minute window of an availability period (SPOT delivery); severity `MEDIUM`
- `check_layering`: flags when ≥ 5 same-side orders from the same org exist at incrementally different prices (Δ ≤ 1%) and ≥ 3 are cancelled within 30 seconds of a fill on the opposite side; severity `HIGH`
- All three detectors return `None` when their pattern threshold is not met
- `run_all_detectors` can be called from a single hook point after any order event

---

### STORY-S6-005 — Surveillance API endpoints + COMPLIANCE_OFFICER role

**Points:** 5

Expose surveillance events to compliance staff via a secured REST API. Add
`COMPLIANCE_OFFICER` as a new user role with read/update access to surveillance
data only.

**Files to create/modify:**
- `app/models/user.py` — add `COMPLIANCE_OFFICER = "COMPLIANCE_OFFICER"` to `UserRole`
- `app/routers/surveillance.py` — new file; `GET /api/surveillance/events` (paginated, filterable by `status`, `event_type`, `severity`); `PATCH /api/surveillance/events/{id}` (update `status`, `reviewer_notes`); require `COMPLIANCE_OFFICER` or `ADMIN` role via `require_role` middleware
- `app/schemas/surveillance.py` — new file; `SurveillanceEventResponse`, `SurveillanceEventUpdate`
- `app/main.py` — register `surveillance_router`
- `alembic/versions/<rev>_add_compliance_officer_role.py` — migration if role is stored in a DB enum (check `native_enum=False` — VARCHAR needs no migration; add note if so)
- `tests/integration/test_surveillance_api.py` — new file; CRUD tests with compliance officer fixture

**Acceptance criteria:**
- `GET /api/surveillance/events` returns paginated list; `?status=OPEN` filter works
- `PATCH /api/surveillance/events/{id}` allows status transition to `UNDER_REVIEW`, `DISMISSED`, or `ESCALATED`; records `reviewed_by` and `reviewed_at`
- BUYER or SUPPLIER role receives 403 on all surveillance endpoints
- `COMPLIANCE_OFFICER` role cannot amend orders or access trade lifecycle endpoints (existing `require_role` guards suffice)
- Integration tests include: list-as-compliance-officer, update-status, forbidden-as-buyer

---

### STORY-S6-006 — Surveillance frontend dashboard

**Points:** 6

Build a compliance officer dashboard page showing the live alert queue with
filter and review actions.

**Files to create/modify (frontend repo):**
- `src/pages/SurveillanceDashboard.tsx` — new page; alert table with columns: type, severity, org, detected_at, status; row actions: Review, Dismiss, Escalate
- `src/components/SurveillanceEventDrawer.tsx` — slide-out detail panel showing implicated order/trade IDs, reviewer notes input, status update button
- `src/lib/api.ts` — `getSurveillanceEvents(filters)` and `updateSurveillanceEvent(id, patch)`
- `src/App.tsx` / routing — add `/compliance/surveillance` route, gated to `COMPLIANCE_OFFICER` and `ADMIN` roles

**Acceptance criteria:**
- Table renders with severity colour coding (CRITICAL=red, HIGH=orange, MEDIUM=yellow, LOW=grey)
- Filtering by status (OPEN / UNDER_REVIEW / DISMISSED / ESCALATED) updates table without page reload
- Reviewer can update status and add notes from the drawer; optimistic UI update on save
- Non-compliance-officer users cannot navigate to the page (redirect to home)
- Empty state shows "No alerts" message rather than an empty table

---

## Sprint 7: API-First SaaS

**Goal:** Enable third-party developers and data consumers to access Verdaxis
programmatically via OAuth2 client credentials, scoped API keys, and tiered
data products.
**Stories:** 6 | **Points:** 24

---

### STORY-S7-001 — OAuthClient model + migration

**Points:** 2

Introduce the `OAuthClient` model that represents a registered API consumer.
Clients authenticate via `client_id` / `client_secret` rather than user
credentials; they receive scoped access tokens.

**Files to create/modify:**
- `app/models/oauth.py` — new file; `OAuthClient` model (`id UUID PK`, `organization_id FK`, `client_id VARCHAR UNIQUE`, `client_secret_hash VARCHAR`, `name`, `scopes JSON` (list of strings), `tier VARCHAR` (FREE/STANDARD/PREMIUM), `is_active BOOL`, `created_at`, `last_used_at`)
- `app/models/__init__.py` — import `OAuthClient`
- `alembic/versions/<rev>_add_oauth_clients.py` — migration
- `tests/unit/test_oauth_model.py` — new file; model instantiation tests

**Acceptance criteria:**
- `OAuthClient` table exists after migration with correct columns and constraints
- `client_id` has a unique index
- `client_secret_hash` stores a bcrypt hash (never plaintext); a `verify_secret(plain)` helper method is on the model
- `scopes` is a JSON column storing a list like `["orderbook:read", "trades:read"]`
- `tier` defaults to `"FREE"` and accepts `FREE`, `STANDARD`, `PREMIUM`

---

### STORY-S7-002 — OAuth2 client CRUD endpoints

**Points:** 3

Allow authenticated organisation users to create, list, and delete API clients
for their organisation.

**Files to create/modify:**
- `app/routers/oauth.py` — new file; `POST /api/oauth/clients`, `GET /api/oauth/clients`, `DELETE /api/oauth/clients/{client_id}` — all require `get_current_user`; scoped to `current_user.organization_id`
- `app/schemas/oauth.py` — new file; `OAuthClientCreate` (name, requested_scopes, tier), `OAuthClientResponse` (includes `client_secret` only on creation — never returned again), `OAuthClientListItem`
- `app/main.py` — register `oauth_router`
- `tests/integration/test_oauth_clients.py` — new file; create, list, delete tests

**Acceptance criteria:**
- `POST /api/oauth/clients` generates a UUID `client_id` and cryptographically random `client_secret`; returns both in the response body (one-time reveal)
- `GET /api/oauth/clients` returns only clients belonging to the caller's organisation
- `DELETE /api/oauth/clients/{client_id}` soft-deletes (`is_active=False`) rather than hard-deletes
- Users from a different organisation cannot delete another org's client (403)
- Integration tests verify create returns secret, list doesn't return secret, delete deactivates

---

### STORY-S7-003 — OAuth2 token endpoint

**Points:** 5

Implement the `client_credentials` grant type so API clients can exchange
their `client_id` and `client_secret` for a short-lived access token.

**Files to create/modify:**
- `app/routers/oauth.py` — add `POST /api/oauth/token` (form body: `grant_type`, `client_id`, `client_secret`); returns `{"access_token": ..., "token_type": "bearer", "expires_in": 3600, "scope": "..."}`
- `app/core/oauth_jwt.py` — new file; `create_client_token(client: OAuthClient) -> str` (JWT with `sub=client_id`, `scopes`, `org_id`, `exp=1h`) and `decode_client_token(token) -> dict`; reuses `JWT_SECRET` from settings
- `tests/unit/test_oauth_jwt.py` — token creation and decode tests
- `tests/integration/test_oauth_clients.py` — add token endpoint test

**Acceptance criteria:**
- Valid `client_id`/`client_secret` pair returns a bearer token with `expires_in=3600`
- Invalid credentials return 401 with `{"error": "invalid_client"}`
- Inactive client (`is_active=False`) returns 401
- Token payload contains `client_id`, `organization_id`, `scopes` list, and `exp` claim
- `last_used_at` on `OAuthClient` is updated on successful token issuance

---

### STORY-S7-004 — Scope enforcement middleware

**Points:** 3

Add a `require_scope` dependency factory analogous to `require_role` so routes
can declare which OAuth scopes are needed. Both user-JWT and client-JWT paths
must work.

**Files to create/modify:**
- `app/middleware/scopes.py` — new file; `require_scope(*scopes)` dependency factory; checks JWT `scopes` claim if present (client token path) or grants full access for user tokens (user tokens have no scope restrictions for now)
- `app/routers/oauth.py` and relevant data endpoints — apply `require_scope` where appropriate
- `tests/unit/test_scope_middleware.py` — tests for scope-present pass, scope-missing 403, user-token bypass

**Acceptance criteria:**
- Client token missing a required scope receives `403 {"error": "insufficient_scope"}`
- Client token with the required scope passes through
- User JWT tokens (no `scopes` claim) are granted access without scope checks (backward compatibility)
- `require_scope` composes cleanly with `require_role` — can be stacked as multiple `Depends`
- Unit tests cover all three access paths

---

### STORY-S7-005 — Tiered data products with delay

**Points:** 5

Extend the existing `GET /api/prices/reference` endpoint to support delay tiers
so data consumers on lower-paid plans receive lagged data. FREE: 24-hour delay;
STANDARD: 1-hour delay; PREMIUM: real-time.

**Files to create/modify:**
- `app/routers/price_discovery.py` — modify `get_reference_prices` to accept an optional `Authorization: Bearer <client_token>` header; extract tier from client token (default `FREE` for unauthenticated); apply `cutoff = now - timedelta(hours=delay_hours[tier])` to the trade query
- `app/core/oauth_jwt.py` — add `get_client_tier(token) -> str` helper
- `app/schemas/price_discovery.py` — add `data_tier` and `delayed_by_hours` fields to `ReferencePriceResponse`
- `tests/integration/test_price_discovery.py` (or new `test_data_products.py`) — tier delay tests

**Acceptance criteria:**
- Unauthenticated request returns data with 24-hour delay and `data_tier: "FREE"` in response
- STANDARD client token returns data with 1-hour delay and `data_tier: "STANDARD"`
- PREMIUM client token returns real-time data (`delayed_by_hours: 0`) and `data_tier: "PREMIUM"`
- `delayed_by_hours` field is present in every response for transparency
- Existing integration tests for `/api/prices/reference` remain green (they hit the FREE tier by default)

---

### STORY-S7-006 — Per-client rate limiting by tier

**Points:** 6

Replace the current IP-based `slowapi` rate limiting with client-ID-aware
limits so API clients are rate-limited by their tier rather than by IP (which
breaks behind NAT or shared proxies).

**Files to create/modify:**
- `app/rate_limit.py` — extend with `client_key_func(request)`: returns `client_id` from JWT if present, falls back to IP; configure tier limits as constants (`FREE=60/min`, `STANDARD=300/min`, `PREMIUM=1200/min`)
- `app/middleware/rate_limit_tier.py` — new file; `TieredRateLimiter` class using an in-memory sliding-window counter (dict of `{key: deque[timestamp]}`); `check_limit(client_id, tier)` raises 429 with `Retry-After` header on breach
- `app/routers/price_discovery.py` — apply `TieredRateLimiter` to data product endpoints
- `tests/unit/test_rate_limit_tier.py` — window boundary tests, 429 path, tier-threshold verification

**Acceptance criteria:**
- FREE client is blocked after 60 requests/minute with `HTTP 429` and a `Retry-After` header
- STANDARD and PREMIUM clients have their respective higher limits
- IP fallback applies for unauthenticated requests (≤ 60/min)
- Counter is memory-bounded: entries older than the window are evicted from the deque
- Unit tests verify blocking at exactly the limit and passing at limit-1

---

## Sprint 8: Platform Stickiness

**Goal:** Deliver a personalised, role-aware dashboard with persistent widget
layouts so users make Verdaxis their daily operational surface.
**Stories:** 6 | **Points:** 17

---

### STORY-S8-001 — Dashboard + DashboardWidget ORM models

**Points:** 2

Define the persistence layer for user dashboards. Each user can have multiple
named dashboards, each containing a list of positioned widgets.

**Files to create/modify:**
- `app/models/user_dashboard.py` — new file; `Dashboard` model (`id UUID PK`, `user_id FK`, `name VARCHAR`, `is_default BOOL`, `created_at`, `updated_at`); `DashboardWidget` model (`id UUID PK`, `dashboard_id FK`, `widget_type VARCHAR`, `grid_x INT`, `grid_y INT`, `grid_w INT`, `grid_h INT`, `config JSON`, `created_at`)
- `app/models/__init__.py` — import both models
- `alembic/versions/<rev>_add_user_dashboards.py` — migration
- `tests/unit/test_dashboard_model.py` — model instantiation and relationship tests

**Acceptance criteria:**
- `dashboards` and `dashboard_widgets` tables exist after migration
- `Dashboard.user_id` is a non-nullable FK to `users.id` with CASCADE delete
- `DashboardWidget.dashboard_id` is a non-nullable FK to `dashboards.id` with CASCADE delete
- `is_default` has a partial unique constraint: at most one default dashboard per user (enforce via unique index on `(user_id, is_default) WHERE is_default = true`)
- `widget_type` is VARCHAR (not enum) to allow future widget types without migrations

---

### STORY-S8-002 — Dashboard CRUD endpoints

**Points:** 3

Expose the dashboard persistence layer via a REST API so the frontend can save
and retrieve layouts.

**Files to create/modify:**
- `app/routers/user_dashboard.py` — new file; `POST /api/dashboards`, `GET /api/dashboards`, `GET /api/dashboards/{id}`, `PUT /api/dashboards/{id}`, `DELETE /api/dashboards/{id}`; widget sub-routes: `POST /api/dashboards/{id}/widgets`, `PUT /api/dashboards/{id}/widgets/{widget_id}`, `DELETE /api/dashboards/{id}/widgets/{widget_id}`
- `app/schemas/user_dashboard.py` — new file; `DashboardCreate`, `DashboardResponse`, `WidgetCreate`, `WidgetUpdate`, `WidgetResponse`
- `app/main.py` — register `user_dashboard_router`
- `tests/integration/test_dashboards.py` — new file; full CRUD integration tests

**Acceptance criteria:**
- `POST /api/dashboards` creates a dashboard scoped to `current_user.id`
- `GET /api/dashboards` returns only dashboards belonging to the caller
- `PUT /api/dashboards/{id}/widgets` (bulk update) accepts a full widget list and replaces the current layout atomically (DELETE old + INSERT new in one transaction)
- User cannot access another user's dashboard (404 rather than 403 — do not leak existence)
- Integration tests cover: create, list, bulk-widget-update, cross-user isolation

---

### STORY-S8-003 — Expanded roles: TRADER and COMPLIANCE_OFFICER enforcement

**Points:** 2

The `COMPLIANCE_OFFICER` role was added in Sprint 6. This story adds `TRADER`
as an explicit role (currently users are `BUYER` or `SUPPLIER`; `TRADER` is for
multi-side participants) and wires both new roles into existing `require_role`
guards where the current `BUYER`/`SUPPLIER` split is too narrow.

**Files to create/modify:**
- `app/models/user.py` — add `TRADER = "TRADER"` to `UserRole` (if not already present from S6)
- `app/middleware/rbac.py` — update `require_role` docstring and any inline comments to reference new roles
- `app/routers/orderbook.py` — allow `TRADER` role wherever `BUYER` or `SUPPLIER` is allowed (side is determined by the order's `side` field, not the role)
- `tests/integration/test_api_endpoints.py` — add TRADER-role fixture and verify it can place both BID and ASK orders

**Acceptance criteria:**
- `UserRole.TRADER` exists in the enum
- A user with `role=TRADER` can place both `BID` and `ASK` orders without a 403
- `TRADER` role does not grant access to compliance/surveillance endpoints (those require `COMPLIANCE_OFFICER` or `ADMIN`)
- No existing BUYER/SUPPLIER tests broken by the role addition
- Registration endpoint accepts `TRADER` as a valid role value

---

### STORY-S8-004 — Default dashboard creation on signup

**Points:** 2

When a new user completes registration, automatically create a default
dashboard pre-populated with a standard set of widgets appropriate to their
role.

**Files to create/modify:**
- `app/routers/auth_simple.py` — call `create_default_dashboard(db, user)` after user is committed in the registration handler
- `app/services/dashboard_defaults.py` — new file; `async def create_default_dashboard(db: AsyncSession, user: User) -> Dashboard`; role-aware widget sets (BUYER: market watch + my bids + compliance score; SUPPLIER: my asks + trade tape + inventory; ADMIN: platform health + commission summary)
- `tests/integration/test_auth.py` — assert that registration response is followed by a GET /api/dashboards returning one dashboard with the expected widget count

**Acceptance criteria:**
- Every new user has exactly one dashboard (marked `is_default=True`) after registration
- BUYER default dashboard has at least 3 widgets; SUPPLIER at least 3; ADMIN at least 2
- `create_default_dashboard` is idempotent — if called twice, a second dashboard is NOT created (guard with a pre-check)
- Widget `config` JSON contains `{"widget_type": "<type>"}` at minimum; additional config keys are acceptable
- Integration test confirms the dashboard exists without an explicit `POST /api/dashboards` call

---

### STORY-S8-005 — Frontend widget system

**Points:** 5

Build the composable widget grid that renders each user's personalised
dashboard. Widgets are draggable and resizable via `react-grid-layout`; eight
widget types are implemented in this sprint.

**Files to create/modify (frontend repo):**
- `src/pages/Dashboard.tsx` — main page; fetches layout from `GET /api/dashboards?default=true`, renders `react-grid-layout` grid
- `src/components/widgets/MarketWatchWidget.tsx` — live order book summary (SSE-connected)
- `src/components/widgets/MyOrdersWidget.tsx` — user's open orders
- `src/components/widgets/TradeHistoryWidget.tsx` — recent confirmed trades
- `src/components/widgets/ComplianceScoreWidget.tsx` — fleet compliance gauge
- `src/components/widgets/PriceTickerWidget.tsx` — reference price table
- `src/components/widgets/NotificationsWidget.tsx` — unread notifications list
- `src/components/widgets/TradeTapeWidget.tsx` — real-time trade stream
- `src/components/widgets/PlatformHealthWidget.tsx` — system metrics (admin only)
- `src/components/WidgetContainer.tsx` — shared wrapper with drag handle, resize handle, widget-type label, and remove button
- `package.json` — add `react-grid-layout` dependency

**Acceptance criteria:**
- All 8 widget types render without errors when data is available
- Drag and resize events update local state immediately (optimistic) and persist to the backend on `onDragStop`/`onResizeStop`
- Removing a widget from the grid calls `DELETE /api/dashboards/{id}/widgets/{widget_id}`
- `PlatformHealthWidget` is only rendered when `current_user.role === "ADMIN"`; absent from the add-widget picker for other roles
- Layout is responsive: grid collapses to single-column on mobile viewport (< 768 px)

---

### STORY-S8-006 — Layout persistence + widget config save

**Points:** 3

Close the loop between the grid UI and the database. Grid position changes must
be debounced and persisted so layout survives page reload.

**Files to create/modify (frontend repo):**
- `src/pages/Dashboard.tsx` — add debounced `saveLayout(layout)` call (300 ms) using `PUT /api/dashboards/{id}/widgets` bulk-update on `onLayoutChange`
- `src/components/widgets/WidgetConfigPanel.tsx` — new slide-out panel for per-widget config (date range pickers, fuel type filter, etc.); save calls `PUT /api/dashboards/{id}/widgets/{widget_id}`
- `src/lib/api.ts` — `bulkUpdateWidgets(dashboardId, widgets)` and `updateWidget(dashboardId, widgetId, config)`
- `src/hooks/useDashboard.ts` — new hook encapsulating fetch, optimistic update, and save logic

**Acceptance criteria:**
- Layout changes are persisted within 300 ms of the last drag/resize event (debounced — not on every pixel move)
- Page reload restores the exact grid positions from the last save
- Widget config changes (e.g. changing fuel type on PriceTickerWidget) are saved via `PUT /api/dashboards/{id}/widgets/{widget_id}` and applied on next render
- `useDashboard` hook returns `{ dashboard, isLoading, saveLayout, updateWidget, removeWidget }` — Dashboard page does not make direct API calls
- Network failure during save shows an error toast; local state is not rolled back (eventual consistency)

---

## Story Summary Table

| Story | Title | Sprint | Points |
|---|---|---|---|
| S5-001 | Order type enum + model fields | 5 | 3 |
| S5-002 | TRIGGERED status | 5 | 1 |
| S5-003 | StopEngine service | 5 | 5 |
| S5-004 | AON match logic | 5 | 3 |
| S5-005 | OCO paired orders | 5 | 5 |
| S5-006 | Order amendment + audit trail | 5 | 5 |
| S5-007 | Hook StopEngine into execution | 5 | 2 |
| S5-008 | Frontend order type UI | 5 | 5 |
| **Sprint 5 total** | | | **29** |
| S6-001 | SurveillanceEvent model + enums | 6 | 2 |
| S6-002 | Wash trading detector | 6 | 3 |
| S6-003 | Spoofing detector | 6 | 3 |
| S6-004 | FrontRunning + MarkingTheClose + Layering | 6 | 5 |
| S6-005 | Surveillance API + COMPLIANCE_OFFICER role | 6 | 5 |
| S6-006 | Surveillance frontend dashboard | 6 | 6 |
| **Sprint 6 total** | | | **24** |
| S7-001 | OAuthClient model + migration | 7 | 2 |
| S7-002 | OAuth2 client CRUD | 7 | 3 |
| S7-003 | OAuth2 token endpoint | 7 | 5 |
| S7-004 | Scope enforcement middleware | 7 | 3 |
| S7-005 | Tiered data products | 7 | 5 |
| S7-006 | Per-client rate limiting by tier | 7 | 6 |
| **Sprint 7 total** | | | **24** |
| S8-001 | Dashboard + DashboardWidget models | 8 | 2 |
| S8-002 | Dashboard CRUD endpoints | 8 | 3 |
| S8-003 | Expanded roles enforcement | 8 | 2 |
| S8-004 | Default dashboard on signup | 8 | 2 |
| S8-005 | Frontend widget system | 8 | 5 |
| S8-006 | Layout persistence + widget config | 8 | 3 |
| **Sprint 8 total** | | | **17** |
| **Grand total** | | | **94** |

---

## Cross-Sprint Dependencies

```
S5-001 (enum/model) → S5-002 (TRIGGERED) → S5-003 (StopEngine) → S5-007 (hook)
S5-001             → S5-004 (AON) → S5-005 (OCO)
S5-001             → S5-006 (amendment)
S5-003, S5-007     → S5-008 (frontend — needs backend complete)

S6-001 (model)     → S6-002, S6-003, S6-004 (detectors)
S6-002–S6-004      → S6-005 (API)
S6-005             → S6-006 (frontend)

S7-001 (model)     → S7-002 (CRUD) → S7-003 (token) → S7-004 (scope) → S7-005 (tiers) → S7-006 (rate limiting)

S8-001 (model)     → S8-002 (CRUD) → S8-004 (signup hook)
S8-003 (roles)     → S8-004
S8-002             → S8-005 (frontend) → S8-006 (persistence)
```

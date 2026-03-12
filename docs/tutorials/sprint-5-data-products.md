# Sprint 5: Data Products & Monetization — Deep Tutorial

> **Verdaxis Exchange** — Turning a trading venue into a data business.

This tutorial walks through every component built in Sprint 5, explaining **why** each
piece exists, **how** it works under the hood, and **how to operate** it. Read it
front-to-back if you are new to the system, or jump to specific sections as a reference.

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Product & DeliveryPoint Catalog](#2-product--deliverypoint-catalog)
3. [FK Migration — From Strings to UUIDs](#3-fk-migration--from-strings-to-uuids)
4. [Forward Curve API](#4-forward-curve-api)
5. [Reference Price VWAP & CSV Export](#5-reference-price-vwap--csv-export)
6. [Subscription Model & Soft Gating](#6-subscription-model--soft-gating)
7. [Price Alerts](#7-price-alerts)
8. [Market Activity Feed (SSE)](#8-market-activity-feed-sse)
9. [Frontend Components](#9-frontend-components)
10. [Security Hardening](#10-security-hardening)
11. [Common Operations](#11-common-operations)
12. [Troubleshooting](#12-troubleshooting)

---

## 1. Architecture Overview

Sprint 5 adds a **monetization layer** on top of the existing exchange. The core
idea: Verdaxis already generates valuable data (trades, prices, market activity).
Sprint 5 packages that data into sellable products.

```
+------------------------------------------------------------------+
|                        Frontend (React + TS)                      |
|  +--------------+  +--------------+  +-----------------------+   |
|  | ForwardCurve |  | ActivityFeed |  |  PriceAlertManager    |   |
|  |  (Recharts)  |  |   (SSE)      |  |  (slide-out panel)    |   |
|  +------+-------+  +------+-------+  +-----------+-----------+   |
|         |                 |                      |               |
|---------|-----------------|----------------------|---------------|
|         v                 v                      v               |
|  +-----------------------------------------------------------+  |
|  |              FastAPI Backend (async)                        |  |
|  |                                                            |  |
|  |  /curves/forward      -> Forward curve from live orderbook |  |
|  |  /curves/forward/export -> CSV download (tier-gated)       |  |
|  |  /prices              -> Aggregated trade prices           |  |
|  |  /prices/reference    -> Daily VWAP benchmarks             |  |
|  |  /prices/reference/export -> CSV download                  |  |
|  |  /alerts              -> Price alert CRUD                  |  |
|  |  /subscriptions/me    -> Current org tier                  |  |
|  |  /stream/activity     -> SSE event stream                  |  |
|  |                                                            |  |
|  |  Middleware: require_tier("standard") -> soft gate         |  |
|  +----------------------------+-------------------------------+  |
|                               |                                  |
|  +----------------------------v-------------------------------+  |
|  |              PostgreSQL + SQLAlchemy (async)                |  |
|  |                                                            |  |
|  |  products ----------+                                      |  |
|  |  delivery_points ---|--- FK from orderbook_orders          |  |
|  |  subscriptions      |                                      |  |
|  |  price_alerts       |                                      |  |
|  |  trades ------------+                                      |  |
|  +------------------------------------------------------------+  |
+------------------------------------------------------------------+
```

**Data flow:** Trades happen on the orderbook -> confirmed trades feed the VWAP
reference price engine -> live orders feed the forward curve -> price movements
trigger alerts -> everything streams via SSE to the activity feed.

---

## 2. Product & DeliveryPoint Catalog

### Why?

Before Sprint 5, products were just strings: `fuel_type="VLSFO"`, `region="Singapore"`.
This made it impossible to:
- Ensure consistent naming (VLSFO vs vlsfo vs "VLS FO")
- Attach metadata (ISO standard, spec grade, unit of measure)
- Build foreign key relationships for price aggregation

### The Models

**File:** `app/models/catalog.py`

```python
class Product(Base):
    __tablename__ = "products"
    id            = Column(UUID, primary_key=True, default=uuid4)
    name          = Column(String(100), unique=True, nullable=False)  # "VLSFO"
    fuel_type     = Column(String(50), nullable=False)                # "VLSFO"
    spec_standard = Column(String(100))                               # "ISO 8217:2017"
    unit          = Column(String(20), default="MT")
    is_active     = Column(Boolean, default=True)

class DeliveryPoint(Base):
    __tablename__ = "delivery_points"
    id     = Column(UUID, primary_key=True, default=uuid4)
    name   = Column(String(100), unique=True, nullable=False)  # "Singapore (SING)"
    region = Column(String(50), nullable=False)                 # "Singapore"
    port   = Column(String(100))
    tz     = Column(String(50))                                 # "Asia/Singapore"
```

### Deterministic UUIDs (Seed Data)

**File:** `app/seeds/catalog_seed.py`

Products and delivery points use `uuid5(NAMESPACE_DNS, name)` to generate
**deterministic** UUIDs. This means:
- Running the seed twice won't create duplicates (same name = same UUID)
- Tests can reference `uuid5(NAMESPACE_DNS, "VLSFO")` without querying the DB
- FK migrations can map existing string data to known UUIDs

```python
from uuid import uuid5, NAMESPACE_DNS

PRODUCTS = [
    {"name": "VLSFO", "fuel_type": "VLSFO", "spec_standard": "ISO 8217:2017"},
    {"name": "HSFO",  "fuel_type": "HSFO",  "spec_standard": "ISO 8217:2017"},
    # ... 7 more
]

for p in PRODUCTS:
    p["id"] = uuid5(NAMESPACE_DNS, p["name"])
```

### API Endpoints

```
GET /api/v1/catalog/products          -> list active products
GET /api/v1/catalog/delivery-points   -> list active delivery points
```

Both are **unauthenticated** — they feed dropdown selectors in the frontend.

---

## 3. FK Migration — From Strings to UUIDs

### The Problem

The `orderbook_orders` table had `fuel_type VARCHAR` and `region VARCHAR` columns.
Sprint 5 replaces these with `product_id UUID` and `delivery_point_id UUID` FK columns.

### Migration Strategy (Two Migrations)

**Migration 1:** `catalog_2026_03_add_product_and_delivery_point.py`
- Creates `products` and `delivery_points` tables
- Seeds them with deterministic data

**Migration 2:** `fk_orderbook_2026_03_orderbook_product_dp_fks.py`
- Adds `product_id` and `delivery_point_id` columns to `orderbook_orders`
- Deletes all existing demo data (Sprint 4 orders/trades were demo-only)
- Makes `product_id` NOT NULL after cleanup
- `delivery_point_id` is nullable (not all orders specify a delivery point)

### Why Delete Demo Data?

Mapping `fuel_type="VLSFO"` to `product_id=uuid5(DNS, "VLSFO")` was considered, but:
1. Demo data had inconsistent strings ("vlsfo", "VLSFO 0.5%", etc.)
2. Sprint 4 trades were test data, not real trades
3. Clean slate is safer than fragile string matching

---

## 4. Forward Curve API

### Concept

A **forward curve** shows the market's view of future prices. For each time window
(Spot, 1-Week, 2-Week, 1-Month, Quarter), the API aggregates:
- **Best Bid** — highest buy order price
- **Best Ask** — lowest sell order price
- **Mid Price** — `(best_bid + best_ask) / 2`
- **Spread** — `best_ask - best_bid`
- **Volume** — total quantity across all orders
- **Order Count** — number of orders

### How It Works

**File:** `app/routers/curves.py`

```python
async def compute_forward_curve(db, product_id) -> list[ForwardCurvePoint]:
    # 1. SQL query: GROUP BY availability_window
    #    MAX(price) WHERE side='BID'   -> best_bid
    #    MIN(price) WHERE side='ASK'   -> best_ask
    #    SUM(quantity), COUNT(id)
    #
    # 2. Python post-processing:
    #    mid_price = (best_bid + best_ask) / 2
    #    spread = best_ask - best_bid
    #    (Only when BOTH bid and ask exist)
```

The SQL uses **conditional aggregation** — a single query with `CASE WHEN side='BID'`
inside aggregate functions, avoiding the need for separate bid/ask queries.

### Endpoints

```
GET  /api/v1/curves/forward?product_id=<uuid>
     -> JSON: { product_name, curve: [...], generated_at }

GET  /api/v1/curves/forward/export?product_id=<uuid>
     -> CSV download (tier-gated: requires "standard" subscription)
```

### CSV Export Gating

The CSV export uses `require_tier("standard")` — a FastAPI dependency that checks
the caller's organization subscription tier. Free-tier users get a `403` with a
message pointing them to upgrade. See [Section 6](#6-subscription-model--soft-gating).

---

## 5. Reference Price VWAP & CSV Export

### Concept

**VWAP** (Volume-Weighted Average Price) is the gold standard for reference pricing
in commodity markets. It weights each trade by its volume:

```
VWAP = SUM(price * quantity) / SUM(quantity)
```

This prevents a tiny 1 MT trade at $1000 from skewing the average when there's
a 10,000 MT trade at $500.

### How It Works

**File:** `app/routers/price_discovery.py`

```python
async def compute_reference_prices(db, ...) -> list[ReferencePriceItem]:
    # SQL:
    #   SUM(price_per_mt_usd * quantity_mt) as weighted_sum
    #   SUM(quantity_mt) as total_volume
    #   GROUP BY product_id, delivery_point_id, CAST(created_at AS DATE)
    #
    # Python:
    #   vwap = weighted_sum / total_volume
```

Only **confirmed, delivered, or paid** trades are included — pending or cancelled
trades are excluded to prevent manipulation.

### Endpoints

```
GET  /api/v1/prices
     -> Aggregated trade summaries (last N hours)

GET  /api/v1/prices/reference?from=2026-01-01&to=2026-03-01
     -> Daily VWAP by product + delivery_point

GET  /api/v1/prices/reference/export?from=2026-01-01&to=2026-03-01
     -> CSV download with columns:
       date, product_name, fuel_type, delivery_point_name,
       region, vwap_usd, volume_mt, trade_count
```

### Visibility Tiers

The `/reference` endpoint accepts `visibility=internal|external`:
- **external** (default): Public benchmark pricing
- **internal**: Platform-internal reference prices

Currently both return the same data — the field is a placeholder for when
Verdaxis wants to offer different price tiers (e.g., delayed vs. real-time).

---

## 6. Subscription Model & Soft Gating

### Tier Design

```
FREE        -> Basic access, 5 alerts max, no CSV export
STANDARD    -> CSV exports, unlimited alerts, priority SSE
ENTERPRISE  -> API access, custom integrations, SLA
```

### The Model

**File:** `app/models/subscription.py`

```python
class SubscriptionTier(str, Enum):
    FREE = "free"
    STANDARD = "standard"
    ENTERPRISE = "enterprise"

class Subscription(Base):
    __tablename__ = "subscriptions"
    id        = Column(UUID, primary_key=True, default=uuid4)
    org_id    = Column(UUID, ForeignKey("organizations.id"), unique=True)
    tier      = Column(SQLEnum(SubscriptionTier), default=SubscriptionTier.FREE)
    is_active = Column(Boolean, default=True)
```

Key design decision: **one subscription per org** (unique constraint on `org_id`).

### Soft Gating with `require_tier()`

**File:** `app/middleware/subscription.py`

```python
def require_tier(minimum: str):
    """FastAPI dependency factory -- returns 403 if org tier is below minimum."""
    TIER_ORDER = {"free": 0, "standard": 1, "enterprise": 2}

    async def checker(user=Depends(get_current_user), db=Depends(get_db)):
        sub = await get_or_create_subscription(db, user["org_id"])
        if TIER_ORDER.get(sub.tier.value, 0) < TIER_ORDER[minimum]:
            raise HTTPException(403, "Upgrade required")
        return sub
    return checker
```

Usage in a router:

```python
@router.get("/curves/forward/export")
async def export_curve(
    ...,
    _sub: Subscription = Depends(require_tier("standard")),  # Gate!
):
```

### "Soft" vs "Hard" Gating

The gating is **soft** — it returns a descriptive error, not a paywall. The frontend
shows an "Upgrade" button instead of the export button for free-tier users. This is
intentional: show the feature, let the user see it's gated, then prompt upgrade.

---

## 7. Price Alerts

### Concept

Users set a threshold price and direction (above/below). When a trade matches,
the alert fires once and records `triggered_at`.

### The Model

**File:** `app/models/alerts.py`

```python
class PriceAlert(Base):
    __tablename__ = "price_alerts"
    id                = Column(UUID, primary_key=True)
    org_id            = Column(UUID, ForeignKey("organizations.id"))
    product_id        = Column(UUID, ForeignKey("products.id"))
    delivery_point_id = Column(UUID, ForeignKey("delivery_points.id"), nullable=True)
    direction         = Column(String(10))   # "above" or "below"
    threshold_usd     = Column(Numeric(12,2))
    triggered_at      = Column(DateTime(timezone=True), nullable=True)  # NULL = active
```

### Alert Matching Logic

**File:** `app/services/activity.py`

```python
async def check_price_alerts(db, product_id, delivery_point_id, trade_price):
    # Find active alerts where:
    #   product_id matches AND
    #   (delivery_point_id matches OR alert.delivery_point_id IS NULL) AND
    #   triggered_at IS NULL AND
    #   ((direction='above' AND trade_price >= threshold) OR
    #    (direction='below' AND trade_price <= threshold))
```

The `OR IS NULL` on delivery_point_id is important: an alert with no delivery point
fires for **any** delivery point for that product. This lets users set broad alerts.

### Free Tier Limit

Free-tier users are limited to **5 active alerts**. The count is checked on create:

```python
FREE_TIER_ALERT_LIMIT = 5

existing = await db.execute(
    select(func.count(PriceAlert.id))
    .where(PriceAlert.org_id == org_id, PriceAlert.triggered_at.is_(None))
)
if existing.scalar() >= FREE_TIER_ALERT_LIMIT:
    raise HTTPException(403, "Free tier: max 5 active alerts")
```

### Endpoints

```
POST   /api/v1/alerts     -> Create alert (body: product_id, direction, threshold_usd)
GET    /api/v1/alerts     -> List org's alerts (both active and triggered)
DELETE /api/v1/alerts/:id  -> Delete alert (owner check)
```

---

## 8. Market Activity Feed (SSE)

### Concept

A real-time Server-Sent Events stream that merges two channels:
1. **Public events** — visible to all (new listings, price crossings, trade matches)
2. **Org-specific events** — visible only to the org (order outbid, alert triggered)

### How SSE Works

The frontend opens an `EventSource` connection to `/api/v1/stream/activity`.
The server holds the connection open and pushes events as they happen:

```
Client -> GET /stream/activity?token=eyJ...
Server -> HTTP 200, Content-Type: text/event-stream

data: {"event": "new_listing", "data": {"fuel_type": "VLSFO", "quantity_mt": 5000}}

data: {"event": "trade_matched", "data": {"price": 520.50, "quantity_mt": 1000}}
```

### Auth via Query Parameter

**Problem:** The `EventSource` API does not support custom headers (no `Authorization`).
**Solution:** Token is passed as a query parameter: `?token=<JWT>`.

**File:** `app/routers/activity.py`

The endpoint accepts an optional `token` query param. If present, it's decoded
inline using `jose.jwt.decode()`. If absent or invalid, the user gets only
public events (no org-specific channel).

### Event Types

| Event | Channel | Description |
|-------|---------|-------------|
| `new_listing` | public | New order posted on orderbook |
| `price_crossing` | public | Price crossed a notable threshold |
| `trade_matched` | public | Trade was matched and confirmed |
| `order_outbid` | org | Your resting order was outbid |
| `price_alert_triggered` | org | One of your price alerts fired |

### Publishing Events

**File:** `app/services/activity.py`

Events are published via the existing `event_bus` (Sprint 1):

```python
from app.events import event_bus

async def publish_new_listing(order):
    await event_bus.publish("activity:public", {
        "event": "new_listing",
        "data": {"fuel_type": order.fuel_type, "quantity_mt": order.quantity_mt}
    })
```

---

## 9. Frontend Components

### ForwardCurve (`src/components/ForwardCurve.tsx`)

- **Recharts ComposedChart**: Bars for bid/ask, line for mid price
- **Product selector dropdown**: Fetches from `/catalog/products`
- **30-second auto-refresh**: `setInterval(fetchCurve, 30_000)`
- **Tier-gated export**: Shows "Upgrade" button (amber lock icon) for free tier,
  "Export CSV" button (blue download icon) for standard+

### ActivityFeed (`src/components/ActivityFeed.tsx`)

- **EventSource** with exponential backoff reconnection
- **Event type icons**: Configured in `EVENT_CONFIG` lookup table
- **Participant highlighting**: Org-specific events get a colored left border
- **Relative timestamps**: Updates every 30 seconds via tick state

### PriceAlertManager (`src/components/PriceAlertManager.tsx`)

- **Slide-out panel**: Triggered by bell icon in MarketTerminal header
- **Alert form**: Product selector, direction (above/below), threshold input
- **Capacity indicator**: 5 small dots showing how many alert slots are used
- **Upgrade CTA**: Amber "Upgrade for Unlimited" link for free-tier users

### MarketTerminal Integration

The `MarketTerminal.tsx` component integrates all three:
- ForwardCurve in the main chart area
- ActivityFeed in a sidebar panel
- PriceAlertManager as a slide-out triggered from the header

---

## 10. Security Hardening

### Docs/Redoc Disabled in Production

**File:** `app/main.py`

```python
_docs_url = "/docs" if os.getenv("ENVIRONMENT") != "production" else None
_redoc_url = "/redoc" if os.getenv("ENVIRONMENT") != "production" else None
```

This prevents API schema exposure in production. In development, `/docs` and
`/redoc` are still available for testing.

### Source Maps Disabled

**File:** `vite.config.ts`

```typescript
build: { sourcemap: false }
```

Production builds don't include source maps, preventing code inspection.

### Rate Limiting

All data endpoints are rate-limited via `slowapi`:

| Endpoint | Limit |
|----------|-------|
| `/prices` | 60/minute |
| `/prices/reference` | 30/minute |
| `/prices/reference/export` | 10/minute |
| `/curves/forward` | 60/minute |
| `/curves/forward/export` | 10/minute |
| `/alerts` (POST) | 30/minute |

### Ownership Checks

Alert deletion verifies `org_id` matches the caller's org — you can't delete
another org's alerts even if you know the UUID.

---

## 11. Common Operations

### Adding a New Product

```python
# In app/seeds/catalog_seed.py, add to PRODUCTS list:
{"name": "MGO", "fuel_type": "MGO", "spec_standard": "ISO 8217:2017"}

# Then run the seed:
python -m app.seeds.catalog_seed
```

### Adding a New Delivery Point

```python
# In app/seeds/catalog_seed.py, add to DELIVERY_POINTS list:
{"name": "Rotterdam (RDAM)", "region": "ARA", "port": "Rotterdam", "tz": "Europe/Amsterdam"}
```

### Upgrading an Org's Subscription

```bash
# Via API (admin only):
curl -X PUT /api/v1/subscriptions/<sub_id> \
  -H "Authorization: Bearer <admin_token>" \
  -d '{"tier": "standard"}'

# Or directly in DB:
UPDATE subscriptions SET tier = 'standard' WHERE org_id = '<org_uuid>';
```

### Adding a New Event Type to the Activity Feed

1. **Backend:** Add a publish function in `app/services/activity.py`
2. **Frontend:** Add entry to `EVENT_CONFIG` in `ActivityFeed.tsx`:

```typescript
EVENT_CONFIG["your_event"] = {
    icon: "!",
    color: "var(--sonar, #0066FF)",
    participantOnly: false  // true = org-only
};
```

3. Add a case to `describeEvent()` for human-readable description

### Running Tests

```bash
cd /home/verdaxis-prod/verdaxis-backend

# All tests
pytest tests/ -v

# Sprint 5 tests only
pytest tests/unit/test_curves.py tests/unit/test_subscription.py \
       tests/unit/test_price_alerts.py tests/unit/test_activity_service.py \
       tests/unit/test_reference_price_export.py -v

# Specific test
pytest tests/unit/test_curves.py::test_forward_curve_basic -v
```

---

## 12. Troubleshooting

### "Forward curve returns empty data"

**Cause:** No active orders exist for the selected product.
**Fix:** Place at least one BID and one ASK order for the product to see bid/ask/mid.

### "SSE connection keeps reconnecting"

**Cause:** Server-side timeout or network interruption.
**Check:** The ActivityFeed component uses exponential backoff (1s -> 2s -> 4s -> ... -> 30s max).
The green/red dot in the header shows connection status. If permanently red, check:
1. Is the backend running? `curl http://localhost:8000/health`
2. Is the token valid? Expired JWTs cause immediate disconnect.

### "CSV export returns 403"

**Cause:** Organization is on the free tier.
**Fix:** Upgrade the subscription to "standard" or higher (see Common Operations above).

### "Alert not firing"

**Check:**
1. Is `triggered_at` NULL? Already-triggered alerts won't fire again.
2. Does the product_id match? Alerts are product-specific.
3. Is the direction correct? "above" triggers when price >= threshold.
4. Is the delivery_point_id correct? NULL dp alerts match any dp, but
   specific dp alerts only match that exact dp.

### "Migration fails with FK constraint error"

**Cause:** Existing orders reference product_id/delivery_point_id values that
don't exist in the products/delivery_points tables.
**Fix:** The FK migration deletes all demo data first. If you've added real data,
you'll need to map it to existing catalog entries before running the migration.

### "Subscription not found"

The system uses **auto-create**: `get_or_create_subscription()` creates a free-tier
subscription on first access. If an org has no subscription record, one is created
automatically. No manual setup needed.

---

## Appendix: File Map

| File | Purpose |
|------|---------|
| `app/models/catalog.py` | Product + DeliveryPoint ORM models |
| `app/models/subscription.py` | Subscription ORM + SubscriptionTier enum |
| `app/models/alerts.py` | PriceAlert ORM model |
| `app/seeds/catalog_seed.py` | Deterministic seed data (uuid5) |
| `app/schemas/curves.py` | ForwardCurvePoint + ForwardCurveResponse |
| `app/schemas/alerts.py` | AlertCreate + AlertResponse |
| `app/schemas/subscription.py` | SubscriptionResponse + SubscriptionUpdate |
| `app/routers/curves.py` | Forward curve API + CSV export |
| `app/routers/alerts.py` | Price alert CRUD |
| `app/routers/activity.py` | SSE activity feed endpoint |
| `app/routers/subscriptions.py` | Subscription management endpoints |
| `app/routers/price_discovery.py` | VWAP reference prices + CSV export |
| `app/middleware/subscription.py` | `require_tier()` dependency factory |
| `app/services/activity.py` | Event publishing + alert checking |
| `src/components/ForwardCurve.tsx` | Recharts forward curve chart |
| `src/components/ActivityFeed.tsx` | SSE real-time activity feed |
| `src/components/PriceAlertManager.tsx` | Alert CRUD slide-out panel |
| `src/types.ts` | TypeScript interfaces |
| `src/services/api.ts` | API client methods |

---

*Sprint 5 — Data Products & Monetization — Verdaxis Exchange*
*327 tests passing | 10 commits backend | 4 commits frontend*

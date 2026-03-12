# Sprint 5: Data Products & Monetization — Design Document

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:writing-plans to create the implementation plan from this design.

**Goal:** Build the monetization layer that turns Verdaxis from a trading venue into a data business.

**Architecture:** Data Layer First — normalize models → curve/export endpoints → activity feed → subscription gating. Each layer builds on the previous.

**Tech Stack:** FastAPI + SQLAlchemy async (backend), React + TypeScript + Vite (frontend), SSE event bus (real-time)

**Sprint 4 Status:** Shipped on feature branches (`feat/anonymous-trade-tape`, `feat/orderbook-component`). Not merged to main. Sprint 4 features: orderbook depth, crossing detection, quantity presets, VWAP internal/external split, anonymous trade tape.

**Intelligence Source:** Venetian Platform (Zuma Labs / Clarksons) recon — maritime FFA/commodities exchange serving 7 tenants via white-label. Key patterns extracted: forward curve as primary data product, CSV export for bulk data, subscription-based access tiers, market activity feeds beyond trade tape.

---

## Design Decisions Summary

| Decision | Choice | Alternatives Considered |
|----------|--------|------------------------|
| Product/DeliveryPoint normalization | Full FK migration (no real data = zero risk) | Parallel catalog layer; skip normalization |
| Subscription tiers | Soft gating on new endpoints only | Full gating on all endpoints; no gating |
| Forward Curve API | Forward curve + enhanced historical VWAP | Historical VWAP only; single combined endpoint |
| Market Activity Feed | Public signals + participant-relevant events | Public only; skip for Sprint 5 |
| Security audit | Quick check (source maps + /docs) | Full hardening pass |
| Build order | Data Layer First (models → curves → activity → subscriptions) | Revenue Path First; parallel streams |

---

## Component 1: Product + DeliveryPoint Models (FK Migration)

### New Models

```python
class Product(Base):
    __tablename__ = "products"
    id = Column(UUID, primary_key=True, default=uuid4)
    name = Column(String, unique=True, nullable=False)       # "Bio-VLSFO 0.5%S"
    fuel_type = Column(String, nullable=False)                # "VLSFO", "MGO", "LNG", etc.
    fuel_grade = Column(String, nullable=False)               # "CONVENTIONAL", "GREEN", "BIO"
    unit = Column(String, default="MT")                       # metric tonnes
    min_lot_size = Column(Numeric, default=100)               # minimum order quantity
    spec_description = Column(String, nullable=True)          # optional spec notes
    is_active = Column(Boolean, default=True)
    created_at = Column(DateTime, server_default=func.now())

class DeliveryPoint(Base):
    __tablename__ = "delivery_points"
    id = Column(UUID, primary_key=True, default=uuid4)
    name = Column(String, unique=True, nullable=False)        # "ARA", "Singapore", "Fujairah"
    region = Column(String, nullable=False)                   # broader region grouping
    timezone = Column(String, nullable=True)                  # "Europe/Amsterdam"
    is_active = Column(Boolean, default=True)
    created_at = Column(DateTime, server_default=func.now())
```

### Migration Impact

**OrderBookOrder:**
- Remove: `fuel_type`, `fuel_grade`, `region` string columns
- Add: `product_id` FK → Product, `delivery_point_id` FK → DeliveryPoint
- Matching engine: string equality → FK equality

**Trade:**
- No direct changes (trades reference orders which reference products)
- `build_trade_response()` denormalizes product/delivery point from the order

**Schemas:**
- `OrderCreate` takes `product_id` + `delivery_point_id` (UUIDs)
- `OrderResponse` includes denormalized `product_name`, `fuel_type`, `fuel_grade`, `delivery_point_name`, `region`
- New: `GET /products` and `GET /delivery-points` for frontend dropdowns

### Seed Data

**Products:**

| name | fuel_type | fuel_grade |
|------|-----------|------------|
| VLSFO Conventional | VLSFO | CONVENTIONAL |
| VLSFO Green | VLSFO | GREEN |
| MGO Conventional | MGO | CONVENTIONAL |
| MGO Bio | MGO | BIO |
| LNG Conventional | LNG | CONVENTIONAL |
| Methanol Green | METHANOL | GREEN |
| Ammonia Green | AMMONIA | GREEN |
| Hydrogen Green | HYDROGEN | GREEN |
| Biofuel Bio | BIOFUEL | BIO |

**Delivery Points:**

| name | region |
|------|--------|
| ARA | Europe |
| Singapore | Asia |
| Fujairah | Middle East |
| Houston | Americas |
| Rotterdam | Europe |

### Frontend Impact

- OrderPlaceModal: dropdowns from API-fetched product/delivery point lists
- TypeScript types: `product_id`, `delivery_point_id` + denormalized display fields
- MarketTerminal filters: product/delivery point selects instead of string enums

---

## Component 2: Forward Curve API

### Endpoints

**Forward Curve by Delivery Window:**
```
GET /api/curves/forward
  ?product_id=<uuid>
  &delivery_point_id=<uuid>     (optional)

Returns: [
  { availability_window, mid_price, best_bid, best_ask, spread, volume_mt, order_count }
]
```

Built from live orderbook data — best bid/ask per availability window, mid-price = (best_bid + best_ask) / 2.

**Forward Curve CSV Export:**
```
GET /api/curves/forward/export
  ?format=csv
  &product_id=<uuid>
  &delivery_point_id=<uuid>     (optional)

Returns: Content-Type: text/csv, Content-Disposition: attachment
```

**Historical VWAP (enhanced existing endpoint):**
```
GET /api/prices/reference
  ?product_id=<uuid>            (replaces fuel_type string param)
  &delivery_point_id=<uuid>     (replaces region string param)
  &from=2026-01-01
  &to=2026-03-12

Returns: [{ date, vwap_usd, total_volume_mt, trade_count, visibility }]
```

**Historical VWAP CSV Export (new):**
```
GET /api/prices/reference/export
  ?format=csv
  &product_id=<uuid>
  &from=...&to=...

Returns: Content-Type: text/csv, Content-Disposition: attachment
```

### Data Sources

- Forward curve: live OrderBookOrder records (status=OPEN), grouped by availability_window
- Historical VWAP: confirmed+ Trade records (CONFIRMED, DELIVERED, PAID), grouped by date

### Subscription Gating

- Forward curve mid-price: Free
- Forward curve full depth (bid/ask/spread/volume): Standard+
- Historical VWAP (7 days): Free
- Historical VWAP (full range): Standard+
- All CSV exports: Standard+

---

## Component 3: Market Activity Feed

### SSE Channel Architecture

New channel: `/stream/activity`

**Public layer** (broadcast to all):
- `new_listing` — anonymized new order ("New ASK: Bio-VLSFO @ ARA, 500 MT")
- `listing_cancelled` — order removed
- `price_crossing` — BID/ASK crossed on a product/delivery point
- `vwap_movement` — VWAP moved ±2% within 1-hour window (threshold-triggered)
- `volume_spike` — volume exceeds 2x 7-day rolling average (threshold-triggered)

**Participant layer** (per-user, authenticated):
- `order_outbid` — user's BID was outbid
- `price_alert` — user's price alert threshold hit
- `match_available` — new order matches user's open order

### Implementation

```python
# Public events → "activity" channel
await event_bus.publish("activity", "new_listing", {...})

# Participant events → "activity:{org_id}" channel
await event_bus.publish(f"activity:{org_id}", "order_outbid", {...})
```

SSE endpoint merges both streams for authenticated users. Unauthenticated users get public events only.

### PriceAlert Model

```python
class PriceAlert(Base):
    __tablename__ = "price_alerts"
    id = Column(UUID, primary_key=True)
    org_id = Column(UUID, ForeignKey("organizations.id"))
    product_id = Column(UUID, ForeignKey("products.id"))
    delivery_point_id = Column(UUID, ForeignKey("delivery_points.id"), nullable=True)
    direction = Column(String)     # "above" or "below"
    threshold_usd = Column(Numeric)
    is_active = Column(Boolean, default=True)
    triggered_at = Column(DateTime, nullable=True)
```

CRUD: `POST/GET/DELETE /api/alerts`

### Threshold Constants (Sprint 5)

- VWAP movement: ±2% within 1-hour window
- Volume spike: 2x 7-day rolling average
- User-configurable market signal thresholds deferred to future sprint

### Subscription Gating

- Public signals: Free (all tiers)
- Participant events (outbid, match available): Free (own orders)
- Price alerts: Free = 5 max, Standard+ = unlimited

---

## Component 4: Subscription Model + Soft Gating

### Model

```python
class SubscriptionTier(str, Enum):
    FREE = "free"
    STANDARD = "standard"
    ENTERPRISE = "enterprise"

class Subscription(Base):
    __tablename__ = "subscriptions"
    id = Column(UUID, primary_key=True)
    org_id = Column(UUID, ForeignKey("organizations.id"), unique=True)
    tier = Column(String, default=SubscriptionTier.FREE)
    started_at = Column(DateTime, server_default=func.now())
    expires_at = Column(DateTime, nullable=True)
    is_active = Column(Boolean, default=True)
```

Every org gets FREE on creation. No Stripe — admin-managed upgrades.

### Entitlement Matrix

| Feature | Free | Standard | Enterprise |
|---------|------|----------|------------|
| Forward curve (mid-price) | Yes | Yes | Yes |
| Forward curve (full depth) | No | Yes | Yes |
| Historical VWAP (7 days) | Yes | Yes | Yes |
| Historical VWAP (full range) | No | Yes | Yes |
| CSV export | No | Yes | Yes |
| Price alerts | 5 max | Unlimited | Unlimited |
| SSE activity feed (public) | Yes | Yes | Yes |
| SSE activity feed (participant) | Yes | Yes | Yes |
| API rate limit | 30/min | 120/min | 600/min |

### Gating Middleware

```python
async def get_subscription(org_id: UUID, db: AsyncSession) -> Subscription:
    # Returns subscription or creates FREE default

def require_tier(minimum: SubscriptionTier):
    # FastAPI Depends — 403 with upgrade message if tier too low
```

Applied only to new Sprint 5 endpoints. Existing endpoints untouched.

### Admin Endpoints

```
PUT /api/admin/subscriptions/{org_id}   — set tier + expiry
GET /api/admin/subscriptions            — list all subscriptions
```

---

## Component 5: Security Quick Check

1. **Source maps:** Verify `vite.config.ts` has `build.sourcemap: false`. Confirm no `.js.map` files in production build output.
2. **/docs endpoint:** Disable FastAPI Swagger UI and ReDoc in production (`docs_url=None, redoc_url=None`).

---

## Design Rationale

### Technical Decisions

**Why normalize to Product/DeliveryPoint FKs now?**
String enums (`fuel_type`, `fuel_grade`, `region`) break down when you need product specs, delivery terms, or cross-product curve comparisons. No real data in the database means the migration cost is zero. Deferring means every new feature built on strings needs a second rewrite later.

**Why soft gating instead of full gating?**
Full gating requires retrofitting auth checks onto ~10 existing endpoints, building upgrade/downgrade UX, and handling error states — all before any paying customer exists. Soft gating proves the entitlement pattern on new endpoints. Extending to existing endpoints later is mechanical.

**Why two curve types (forward + historical VWAP)?**
Forward curve by delivery window is the primary data product — what Venetian charges for and what traders price decisions on. Historical VWAP already exists at `GET /prices/reference`. Rather than duplicating under `/curves/history`, we enhance the existing endpoint with product FK filtering and CSV export.

**Why forward curve from live orderbook, not from trades?**
Trade-derived curves need sufficient volume across all delivery windows — early exchanges rarely have this. Orderbook-derived curves produce meaningful data even with sparse activity. Trade-based overlays can be added later without changing the endpoint contract.

**Why activity feed with participant events?**
Public signals alone make Verdaxis look like a data terminal. Participant events ("your order was outbid") make it feel like an active exchange working for you. The per-user filtering uses existing event bus patterns (per-org channels exist for notifications).

**Why server-side threshold constants?**
User-configurable market signal thresholds need settings UI, storage, and per-user evaluation — significant scope for marginal Sprint 5 value. Fixed thresholds (±2% VWAP, 2x volume) are sensible defaults. Price alerts cover personalization.

**Why "Data Layer First" build order?**
FK migration must precede curve endpoints (query by product). Curves must exist before subscription gating (gating gates curves). Activity feed depends on Product models for meaningful descriptions. Each layer foundations the next.

### Commercial Decisions

**Why three tiers (Free / Standard / Enterprise)?**
Free exists to grow the network. An exchange is worthless without participants — gating basic access kills liquidity before it starts. Free gives: orderbook visibility, mid-price curve, 7-day VWAP, activity feed. Enough to trade, not enough to build a data business on.

Standard is the monetization target: full-depth curves, unlimited VWAP history, CSV exports, unlimited alerts. Priced at $1K-5K/seat/month (Sprint 3 target). Exact pricing is a sales conversation, not an engineering decision.

Enterprise is a placeholder for custom deals — higher rate limits, future features (webhooks, API keys, white-label widgets). Not built in Sprint 5, exists as a tier definition for strategic partners.

**Why gate CSV export but not JSON?**
CSV is a bulk extraction — download once and cancel. JSON with rate limits is a continuous relationship — keep coming back because data updates. Gating CSV behind Standard ensures bulk extraction requires ongoing commitment.

**Why gate curve depth but not mid-price?**
Mid-price is a marketing tool — shows free users Verdaxis has live pricing. Full depth (bid/ask/spread/volume) is actionable intelligence — where liquidity is, which direction prices move. The gap between "see the price" and "trade on the price" is what Standard monetizes.

**Why 7-day free VWAP window?**
7 days verifies VWAP aligns with trader intuition — builds credibility. Not enough to backtest strategies, build pricing models, or satisfy compliance audits. Those need months/years, which is Standard. 7 days is shorter than any standard reporting period (monthly, quarterly).

**Why 5 price alert cap on Free?**
5 covers one product × one delivery point × one direction — enough for a primary market. Trading desks covering multiple products/regions need 20-50 alerts. The cap creates organic upgrade pressure without crippling Free.

**Why no self-serve upgrade (no Stripe)?**
Early data subscribers are strategic relationships. Manual upgrade means: talk to every subscriber, understand their use case, price-discriminate between small traders and major houses, offer trials without trial infrastructure. Self-serve is for scale — Sprint 5 is for product-market fit.

**Why not per-API-call pricing?**
Usage-based punishes power users — the users who provide the most liquidity. A desk hitting curves 1,000 times/day is more valuable than one checking weekly. Tiers reward engagement. Usage-based also requires metering infrastructure for uncertain revenue.

**Competitive positioning vs Venetian/Argus:**
Venetian gates by tenant (white-label). Argus gates by subscription product (annual data contracts). Verdaxis gates by tier within a single platform — simpler, more transparent, aligned with the exchange model. Verdaxis is not a data vendor (Argus) or SaaS platform (Venetian) — it's a transparent exchange that produces valuable data as a byproduct of trading.

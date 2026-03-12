# Sprint 5: Data Products & Monetization — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build the monetization layer — normalized product models, forward curve API, market activity feed, subscription tiers with soft gating.

**Architecture:** Data Layer First — FK migration → curve endpoints → activity feed → subscription gating. Each layer builds on the previous. TDD throughout.

**Tech Stack:** FastAPI + SQLAlchemy async, React + TypeScript + Vite, SSE event bus, Alembic migrations

**Design Doc:** `docs/plans/2026-03-12-data-products-design.md`

**Codebase Entry Points:**
- Backend: `/home/verdaxis-prod/verdaxis-backend/`
- Frontend: `/home/verdaxis-prod/verdaxis-frontend/`
- Models: `app/models/` (exported via `app/models/__init__.py`)
- Routers: `app/routers/` (registered in `app/main.py` lines 121-143)
- Schemas: `app/schemas/`
- Services: `app/services/`
- Tests: `tests/unit/` and `tests/integration/`

**Branch Strategy:** Create `feat/data-products` from current `feat/anonymous-trade-tape` (backend) and `feat/orderbook-component` (frontend) which contain Sprint 4 work.

---

## Task 1: Create Sprint 5 Branch + Merge Sprint 4

**Files:**
- No code changes — git operations only

**Steps:**

1. Backend: checkout main, merge Sprint 4 branch, create Sprint 5 branch:
```bash
cd /home/verdaxis-prod/verdaxis-backend
git checkout main
git merge feat/anonymous-trade-tape --no-ff -m "merge: Sprint 4 anonymous trade tape"
git checkout -b feat/data-products
```

2. Frontend: same pattern:
```bash
cd /home/verdaxis-prod/verdaxis-frontend
git checkout main
git merge feat/orderbook-component --no-ff -m "merge: Sprint 4 orderbook + crossing detection + quantity presets + VWAP split"
git checkout -b feat/data-products
```

3. Verify both repos are on `feat/data-products` and tests pass:
```bash
cd /home/verdaxis-prod/verdaxis-backend && pytest tests/ -x -q
```

---

## Task 2: Product + DeliveryPoint Models

**Files:**
- Create: `app/models/catalog.py`
- Modify: `app/models/__init__.py`
- Create: `tests/unit/test_catalog_models.py`
- Create: Alembic migration via `alembic revision`

**Step 1: Write failing tests for Product and DeliveryPoint models**

```python
# tests/unit/test_catalog_models.py
import pytest
from uuid import uuid4
from decimal import Decimal
from sqlalchemy.ext.asyncio import create_async_engine, async_sessionmaker, AsyncSession
from sqlalchemy import select
from app.database import Base
from app.models.catalog import Product, DeliveryPoint


@pytest.fixture(scope="module")
def event_loop():
    import asyncio
    loop = asyncio.new_event_loop()
    yield loop
    loop.close()


@pytest.fixture(scope="module")
async def async_engine():
    engine = create_async_engine("sqlite+aiosqlite://", echo=False)
    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all)
    yield engine
    await engine.dispose()


@pytest.fixture
async def db(async_engine):
    session_factory = async_sessionmaker(async_engine, class_=AsyncSession, expire_on_commit=False)
    async with session_factory() as session:
        yield session
        await session.rollback()


@pytest.mark.asyncio
async def test_create_product(db):
    product = Product(
        name="VLSFO Conventional",
        fuel_type="VLSFO",
        fuel_grade="CONVENTIONAL",
        unit="MT",
        min_lot_size=Decimal("100"),
    )
    db.add(product)
    await db.flush()
    assert product.id is not None
    assert product.is_active is True


@pytest.mark.asyncio
async def test_product_name_unique(db):
    p1 = Product(name="MGO Bio", fuel_type="MGO", fuel_grade="BIO")
    p2 = Product(name="MGO Bio", fuel_type="MGO", fuel_grade="BIO")
    db.add(p1)
    await db.flush()
    db.add(p2)
    with pytest.raises(Exception):  # IntegrityError
        await db.flush()


@pytest.mark.asyncio
async def test_create_delivery_point(db):
    dp = DeliveryPoint(name="ARA", region="Europe", timezone="Europe/Amsterdam")
    db.add(dp)
    await db.flush()
    assert dp.id is not None
    assert dp.is_active is True


@pytest.mark.asyncio
async def test_delivery_point_name_unique(db):
    dp1 = DeliveryPoint(name="Singapore", region="Asia")
    dp2 = DeliveryPoint(name="Singapore", region="Asia")
    db.add(dp1)
    await db.flush()
    db.add(dp2)
    with pytest.raises(Exception):
        await db.flush()
```

**Step 2: Run tests — expect FAIL (module not found)**

```bash
cd /home/verdaxis-prod/verdaxis-backend
pytest tests/unit/test_catalog_models.py -v
```

**Step 3: Implement Product and DeliveryPoint models**

```python
# app/models/catalog.py
from uuid import uuid4
from sqlalchemy import Column, String, Numeric, Boolean, DateTime, func
from sqlalchemy.dialects.postgresql import UUID
from sqlalchemy.orm import relationship
from app.database import Base


class Product(Base):
    __tablename__ = "products"

    id = Column(UUID(as_uuid=True), primary_key=True, default=uuid4)
    name = Column(String, unique=True, nullable=False)
    fuel_type = Column(String, nullable=False)
    fuel_grade = Column(String, nullable=False)
    unit = Column(String, default="MT")
    min_lot_size = Column(Numeric(12, 2), default=100)
    spec_description = Column(String, nullable=True)
    is_active = Column(Boolean, default=True)
    created_at = Column(DateTime(timezone=True), server_default=func.now())

    orders = relationship("OrderBookOrder", back_populates="product")

    def __repr__(self):
        return f"<Product {self.name}>"


class DeliveryPoint(Base):
    __tablename__ = "delivery_points"

    id = Column(UUID(as_uuid=True), primary_key=True, default=uuid4)
    name = Column(String, unique=True, nullable=False)
    region = Column(String, nullable=False)
    timezone = Column(String, nullable=True)
    is_active = Column(Boolean, default=True)
    created_at = Column(DateTime(timezone=True), server_default=func.now())

    orders = relationship("OrderBookOrder", back_populates="delivery_point")

    def __repr__(self):
        return f"<DeliveryPoint {self.name} ({self.region})>"
```

**Step 4: Register in models __init__.py**

Add to `app/models/__init__.py`:
```python
from app.models.catalog import Product, DeliveryPoint
```

**Step 5: Run tests — expect PASS**

```bash
pytest tests/unit/test_catalog_models.py -v
```

**Step 6: Generate Alembic migration**

```bash
cd /home/verdaxis-prod/verdaxis-backend
alembic revision --autogenerate -m "add product and delivery_point tables"
```

Review the generated migration, then:
```bash
alembic upgrade head
```

**Step 7: Create seed data script**

```python
# app/seeds/catalog_seed.py
from uuid import uuid4
from decimal import Decimal
from app.models.catalog import Product, DeliveryPoint

PRODUCTS = [
    Product(id=uuid4(), name="VLSFO Conventional", fuel_type="VLSFO", fuel_grade="CONVENTIONAL", min_lot_size=Decimal("100")),
    Product(id=uuid4(), name="VLSFO Green", fuel_type="VLSFO", fuel_grade="GREEN", min_lot_size=Decimal("100")),
    Product(id=uuid4(), name="MGO Conventional", fuel_type="MGO", fuel_grade="CONVENTIONAL", min_lot_size=Decimal("50")),
    Product(id=uuid4(), name="MGO Bio", fuel_type="MGO", fuel_grade="BIO", min_lot_size=Decimal("50")),
    Product(id=uuid4(), name="LNG Conventional", fuel_type="LNG", fuel_grade="CONVENTIONAL", min_lot_size=Decimal("500")),
    Product(id=uuid4(), name="Methanol Green", fuel_type="METHANOL", fuel_grade="GREEN", min_lot_size=Decimal("100")),
    Product(id=uuid4(), name="Ammonia Green", fuel_type="AMMONIA", fuel_grade="GREEN", min_lot_size=Decimal("200")),
    Product(id=uuid4(), name="Hydrogen Green", fuel_type="HYDROGEN", fuel_grade="GREEN", min_lot_size=Decimal("50")),
    Product(id=uuid4(), name="Biofuel Bio", fuel_type="BIOFUEL", fuel_grade="BIO", min_lot_size=Decimal("100")),
]

DELIVERY_POINTS = [
    DeliveryPoint(id=uuid4(), name="ARA", region="Europe", timezone="Europe/Amsterdam"),
    DeliveryPoint(id=uuid4(), name="Singapore", region="Asia", timezone="Asia/Singapore"),
    DeliveryPoint(id=uuid4(), name="Fujairah", region="Middle East", timezone="Asia/Dubai"),
    DeliveryPoint(id=uuid4(), name="Houston", region="Americas", timezone="America/Chicago"),
    DeliveryPoint(id=uuid4(), name="Rotterdam", region="Europe", timezone="Europe/Amsterdam"),
]
```

Add a seed runner function and wire it into the app startup or a CLI command.

**Step 8: Commit**

```bash
git add app/models/catalog.py app/models/__init__.py app/seeds/ tests/unit/test_catalog_models.py alembic/versions/
git commit -m "feat: add Product and DeliveryPoint catalog models with seed data"
```

---

## Task 3: FK Migration — OrderBookOrder Uses Product/DeliveryPoint

**Files:**
- Modify: `app/models/orderbook.py` — replace string fields with FKs
- Modify: `app/schemas/orderbook.py` — update OrderCreate, OrderResponse
- Modify: `app/routers/orderbook.py` — update queries and creation
- Modify: `app/services/matching_engine.py` — match by product_id + delivery_point_id
- Modify: `app/routers/trades.py` — update build_trade_response denormalization
- Modify: `app/routers/price_discovery.py` — update VWAP queries to use product FK
- Modify: `tests/unit/test_matching_engine.py` — update test helpers
- Modify: `tests/unit/test_orderbook_schemas.py` — update schema tests
- Create: Alembic migration

**Step 1: Write failing test for FK-based order creation**

Add to existing test file or create new:
```python
# tests/unit/test_orderbook_fk_migration.py
@pytest.mark.asyncio
async def test_order_requires_product_id(db, buyer_org, sample_product):
    order = OrderBookOrder(
        organization_id=buyer_org.id,
        side=OrderSide.BID,
        product_id=sample_product.id,
        delivery_point_id=None,  # optional
        quantity_mt=Decimal("1000"),
        remaining_quantity_mt=Decimal("1000"),
        price_per_mt_usd=Decimal("550"),
        availability_window=AvailabilityWindow.SPOT,
        status=OrderBookStatus.OPEN,
    )
    db.add(order)
    await db.flush()
    assert order.product_id == sample_product.id
    assert order.product.fuel_type == "VLSFO"


@pytest.mark.asyncio
async def test_matching_engine_matches_by_product_id(db, buyer_org, seller_org, sample_product, sample_delivery_point):
    """Orders only match when product_id AND delivery_point_id are equal."""
    ask = _make_order(seller_org.id, OrderSide.ASK, product_id=sample_product.id,
                      delivery_point_id=sample_delivery_point.id, price=Decimal("500"))
    bid = _make_order(buyer_org.id, OrderSide.BID, product_id=sample_product.id,
                      delivery_point_id=sample_delivery_point.id, price=Decimal("550"))
    db.add_all([ask, bid])
    await db.flush()
    trades = await match_order(db, bid)
    assert len(trades) == 1
    assert trades[0].price_per_mt_usd == Decimal("500")  # resting order price
```

**Step 2: Modify OrderBookOrder model**

In `app/models/orderbook.py`, replace:
```python
fuel_type = Column(String, nullable=False)
fuel_grade = Column(String, nullable=False)
region = Column(String, nullable=False)
```

With:
```python
product_id = Column(UUID(as_uuid=True), ForeignKey("products.id"), nullable=False)
delivery_point_id = Column(UUID(as_uuid=True), ForeignKey("delivery_points.id"), nullable=True)

product = relationship("Product", back_populates="orders", lazy="joined")
delivery_point = relationship("DeliveryPoint", back_populates="orders", lazy="joined")
```

**Step 3: Update OrderCreate schema**

In `app/schemas/orderbook.py`, replace `fuel_type`, `fuel_grade`, `region` string fields with:
```python
product_id: UUID
delivery_point_id: Optional[UUID] = None
```

**Step 4: Update OrderResponse schema**

Add denormalized display fields:
```python
product_id: UUID
product_name: str
fuel_type: str
fuel_grade: str
delivery_point_id: Optional[UUID] = None
delivery_point_name: Optional[str] = None
region: Optional[str] = None
```

**Step 5: Update matching engine**

In `app/services/matching_engine.py`, change the filter from:
```python
.filter(OrderBookOrder.fuel_type == new_order.fuel_type)
.filter(OrderBookOrder.region == new_order.region)
```
To:
```python
.filter(OrderBookOrder.product_id == new_order.product_id)
.filter(OrderBookOrder.delivery_point_id == new_order.delivery_point_id)
```

**Step 6: Update orderbook router**

In `app/routers/orderbook.py`:
- Update create endpoint to accept `product_id`/`delivery_point_id`
- Update filter params from `fuel_type`/`region` strings to `product_id`/`delivery_point_id` UUIDs
- Add eager loading of `product` and `delivery_point` relationships
- Add `GET /products` and `GET /delivery-points` catalog endpoints

**Step 7: Update price_discovery router**

In `app/routers/price_discovery.py`:
- Change query params from `fuel_type`/`region` to `product_id`/`delivery_point_id`
- Update VWAP grouping to use product_id + delivery_point_id
- Update ReferencePriceItem to include product_name and delivery_point_name

**Step 8: Update trades router**

In `app/routers/trades.py`:
- Update `build_trade_response()` to denormalize from `trade.bid_order.product` or `trade.ask_order.product`
- Update SSE event payloads to include product_name instead of fuel_type string

**Step 9: Update all existing tests**

- `tests/unit/test_matching_engine.py` — update `_make_order()` helper to use product_id/delivery_point_id
- `tests/unit/test_orderbook_schemas.py` — update schema test data
- `tests/integration/test_orderbook.py` — update API call payloads
- All other tests referencing fuel_type/region on orders

**Step 10: Generate migration and run tests**

```bash
alembic revision --autogenerate -m "migrate orderbook from string enums to product and delivery_point FKs"
alembic upgrade head
pytest tests/ -x -q
```

**Step 11: Update seed data**

Update the existing order/trade seed scripts to reference Product and DeliveryPoint records by ID instead of string values.

**Step 12: Commit**

```bash
git add -A
git commit -m "feat: migrate OrderBookOrder from string enums to Product/DeliveryPoint FKs"
```

---

## Task 4: Frontend — Product/DeliveryPoint Integration

**Files:**
- Modify: `src/types.ts` — update OrderBookOrder, add Product/DeliveryPoint types
- Modify: `src/services/api.ts` — add catalog endpoints, update order creation
- Modify: `src/components/OrderPlaceModal.tsx` — fetch products/delivery points for dropdowns
- Modify: `src/components/MarketTerminal.tsx` — update filters
- Modify: `src/components/Marketplace.tsx` — update filters
- Modify: `src/components/OrderBook.tsx` — update display fields

**Step 1: Add Product and DeliveryPoint types to types.ts**

```typescript
export interface Product {
  id: string;
  name: string;
  fuel_type: string;
  fuel_grade: string;
  unit: string;
  min_lot_size: number;
  spec_description?: string;
  is_active: boolean;
}

export interface DeliveryPoint {
  id: string;
  name: string;
  region: string;
  timezone?: string;
  is_active: boolean;
}
```

Update `OrderBookOrder` to use `product_id`, `product_name`, `delivery_point_id`, `delivery_point_name` instead of raw `fuel_type`/`region`.

**Step 2: Add catalog API endpoints**

```typescript
// In api.ts
catalog: {
  products: () => fetchApi('/products'),
  deliveryPoints: () => fetchApi('/delivery-points'),
},
```

**Step 3: Update OrderPlaceModal**

- Fetch products and delivery points on mount
- Replace fuel_type/fuel_grade/region text inputs with Product dropdown and DeliveryPoint dropdown
- Send `product_id` and `delivery_point_id` in order creation payload

**Step 4: Update MarketTerminal filters**

- Replace fuel_type/region string selects with product/delivery point selects
- Filter orderbook by product_id/delivery_point_id

**Step 5: Update display components**

- OrderBook, Marketplace, MyTrades: display `product_name` and `delivery_point_name` from denormalized response fields

**Step 6: Run frontend tests and commit**

```bash
cd /home/verdaxis-prod/verdaxis-frontend
npm test -- --run
git add -A
git commit -m "feat: frontend product and delivery point integration"
```

---

## Task 5: Forward Curve API

**Files:**
- Create: `app/routers/curves.py`
- Create: `app/schemas/curves.py`
- Modify: `app/main.py` — register curves router
- Create: `tests/unit/test_curves.py`

**Step 1: Write failing tests**

```python
# tests/unit/test_curves.py
@pytest.mark.asyncio
async def test_forward_curve_returns_data_by_window(client, seed_orders):
    """Forward curve returns mid_price, best_bid, best_ask per availability_window."""
    resp = await client.get(f"/api/v1/curves/forward?product_id={PRODUCT_ID}")
    assert resp.status_code == 200
    data = resp.json()
    assert isinstance(data["curve"], list)
    for point in data["curve"]:
        assert "availability_window" in point
        assert "mid_price" in point
        assert "best_bid" in point
        assert "best_ask" in point
        assert "spread" in point
        assert "volume_mt" in point
        assert "order_count" in point


@pytest.mark.asyncio
async def test_forward_curve_csv_export(client, seed_orders):
    """CSV export returns text/csv with correct headers."""
    resp = await client.get(f"/api/v1/curves/forward/export?format=csv&product_id={PRODUCT_ID}")
    assert resp.status_code == 200
    assert "text/csv" in resp.headers["content-type"]
    assert "attachment" in resp.headers["content-disposition"]
    lines = resp.text.strip().split("\n")
    assert lines[0] == "availability_window,mid_price,best_bid,best_ask,spread,volume_mt,order_count"


@pytest.mark.asyncio
async def test_forward_curve_empty_when_no_orders(client):
    """Returns empty curve array when no orders exist for product."""
    resp = await client.get(f"/api/v1/curves/forward?product_id={uuid4()}")
    assert resp.status_code == 200
    assert resp.json()["curve"] == []
```

**Step 2: Implement schemas**

```python
# app/schemas/curves.py
from pydantic import BaseModel
from typing import Optional
from uuid import UUID
from datetime import datetime


class ForwardCurvePoint(BaseModel):
    availability_window: str
    mid_price: Optional[float] = None
    best_bid: Optional[float] = None
    best_ask: Optional[float] = None
    spread: Optional[float] = None
    volume_mt: float = 0
    order_count: int = 0


class ForwardCurveResponse(BaseModel):
    product_id: UUID
    product_name: str
    delivery_point_id: Optional[UUID] = None
    delivery_point_name: Optional[str] = None
    curve: list[ForwardCurvePoint]
    generated_at: datetime
```

**Step 3: Implement curves router**

```python
# app/routers/curves.py
from fastapi import APIRouter, Depends, Query
from fastapi.responses import StreamingResponse
from sqlalchemy import select, func
from sqlalchemy.ext.asyncio import AsyncSession
from uuid import UUID
from datetime import datetime, timezone
from typing import Optional
import csv, io

from app.database import get_db
from app.models.orderbook import OrderBookOrder, OrderSide, OrderBookStatus
from app.models.catalog import Product, DeliveryPoint
from app.schemas.curves import ForwardCurvePoint, ForwardCurveResponse

router = APIRouter(prefix="/curves", tags=["curves"])


@router.get("/forward", response_model=ForwardCurveResponse)
async def get_forward_curve(
    product_id: UUID,
    delivery_point_id: Optional[UUID] = None,
    db: AsyncSession = Depends(get_db),
):
    # Fetch product
    product = await db.get(Product, product_id)
    if not product:
        raise HTTPException(404, "Product not found")

    # Build query: open orders grouped by availability_window
    query = (
        select(
            OrderBookOrder.availability_window,
            OrderBookOrder.side,
            func.min(OrderBookOrder.price_per_mt_usd).label("min_price"),
            func.max(OrderBookOrder.price_per_mt_usd).label("max_price"),
            func.sum(OrderBookOrder.remaining_quantity_mt).label("total_volume"),
            func.count().label("order_count"),
        )
        .where(OrderBookOrder.product_id == product_id)
        .where(OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]))
        .group_by(OrderBookOrder.availability_window, OrderBookOrder.side)
    )
    if delivery_point_id:
        query = query.where(OrderBookOrder.delivery_point_id == delivery_point_id)

    result = await db.execute(query)
    rows = result.all()

    # Aggregate by window
    windows = {}
    for row in rows:
        w = row.availability_window
        if w not in windows:
            windows[w] = {"best_bid": None, "best_ask": None, "volume_mt": 0, "order_count": 0}
        if row.side == OrderSide.BID:
            windows[w]["best_bid"] = float(row.max_price)  # highest bid
        else:
            windows[w]["best_ask"] = float(row.min_price)  # lowest ask
        windows[w]["volume_mt"] += float(row.total_volume)
        windows[w]["order_count"] += row.order_count

    # Build curve points
    curve = []
    for window, data in sorted(windows.items(), key=lambda x: x[0]):
        bid, ask = data["best_bid"], data["best_ask"]
        mid = ((bid + ask) / 2) if bid and ask else None
        spread = (ask - bid) if bid and ask else None
        curve.append(ForwardCurvePoint(
            availability_window=window,
            mid_price=mid,
            best_bid=bid,
            best_ask=ask,
            spread=spread,
            volume_mt=data["volume_mt"],
            order_count=data["order_count"],
        ))

    dp = await db.get(DeliveryPoint, delivery_point_id) if delivery_point_id else None
    return ForwardCurveResponse(
        product_id=product_id,
        product_name=product.name,
        delivery_point_id=delivery_point_id,
        delivery_point_name=dp.name if dp else None,
        curve=curve,
        generated_at=datetime.now(timezone.utc),
    )


@router.get("/forward/export")
async def export_forward_curve(
    product_id: UUID,
    delivery_point_id: Optional[UUID] = None,
    format: str = Query("csv"),
    db: AsyncSession = Depends(get_db),
):
    curve_resp = await get_forward_curve(product_id, delivery_point_id, db)
    output = io.StringIO()
    writer = csv.writer(output)
    writer.writerow(["availability_window", "mid_price", "best_bid", "best_ask", "spread", "volume_mt", "order_count"])
    for p in curve_resp.curve:
        writer.writerow([p.availability_window, p.mid_price, p.best_bid, p.best_ask, p.spread, p.volume_mt, p.order_count])
    output.seek(0)
    return StreamingResponse(
        output,
        media_type="text/csv",
        headers={"Content-Disposition": f"attachment; filename=forward_curve_{product_id}.csv"},
    )
```

**Step 4: Register router in main.py**

Add to `app/main.py`:
```python
from app.routers import curves
app.include_router(curves.router, prefix=settings.API_V1_STR)
```

**Step 5: Run tests, commit**

```bash
pytest tests/unit/test_curves.py -v
git add app/routers/curves.py app/schemas/curves.py app/main.py tests/unit/test_curves.py
git commit -m "feat: forward curve API with CSV export"
```

---

## Task 6: Historical VWAP Enhancement + CSV Export

**Files:**
- Modify: `app/routers/price_discovery.py` — update params to product_id, add CSV export
- Modify: `app/schemas/orderbook.py` — update ReferencePriceItem
- Create: `tests/unit/test_reference_price_export.py`

**Step 1: Write failing test for CSV export**

```python
@pytest.mark.asyncio
async def test_reference_price_csv_export(client, seed_trades):
    resp = await client.get(f"/api/v1/prices/reference/export?format=csv&product_id={PRODUCT_ID}")
    assert resp.status_code == 200
    assert "text/csv" in resp.headers["content-type"]
    lines = resp.text.strip().split("\n")
    assert "date" in lines[0]
    assert "vwap_usd" in lines[0]
```

**Step 2: Update reference price endpoint params**

Change `fuel_type: Optional[str]` and `region: Optional[str]` to `product_id: Optional[UUID]` and `delivery_point_id: Optional[UUID]`. Update the SQLAlchemy query to join through `OrderBookOrder.product_id` and `OrderBookOrder.delivery_point_id`.

**Step 3: Add CSV export endpoint**

```python
@router.get("/prices/reference/export")
async def export_reference_prices(
    product_id: Optional[UUID] = None,
    delivery_point_id: Optional[UUID] = None,
    from_date: Optional[date] = None,
    to_date: Optional[date] = None,
    format: str = Query("csv"),
    db: AsyncSession = Depends(get_db),
):
    # Reuse existing reference price logic, stream as CSV
    ...
```

**Step 4: Update ReferencePriceItem schema**

Add `product_id`, `product_name`, `delivery_point_id`, `delivery_point_name` to the response model. Keep `fuel_type` and `region` as denormalized display fields for backward compat.

**Step 5: Run tests, commit**

```bash
pytest tests/unit/test_reference_price_export.py tests/unit/test_price_discovery_*.py -v
git add app/routers/price_discovery.py app/schemas/orderbook.py tests/
git commit -m "feat: enhance reference price API with product FK + CSV export"
```

---

## Task 7: Subscription Model + Soft Gating Middleware

**Files:**
- Create: `app/models/subscription.py`
- Create: `app/schemas/subscription.py`
- Create: `app/routers/subscriptions.py`
- Create: `app/middleware/subscription.py`
- Modify: `app/models/__init__.py`
- Modify: `app/main.py`
- Create: `tests/unit/test_subscription.py`
- Create: Alembic migration

**Step 1: Write failing tests**

```python
# tests/unit/test_subscription.py
@pytest.mark.asyncio
async def test_default_subscription_is_free(db, org):
    sub = await get_or_create_subscription(db, org.id)
    assert sub.tier == SubscriptionTier.FREE
    assert sub.is_active is True


@pytest.mark.asyncio
async def test_require_tier_blocks_free_on_standard_endpoint():
    """Free tier user gets 403 on Standard-gated endpoint."""
    ...


@pytest.mark.asyncio
async def test_require_tier_allows_standard_on_standard_endpoint():
    """Standard tier user gets 200 on Standard-gated endpoint."""
    ...


@pytest.mark.asyncio
async def test_admin_can_upgrade_subscription(admin_client, org):
    resp = await admin_client.put(f"/api/v1/admin/subscriptions/{org.id}", json={"tier": "standard"})
    assert resp.status_code == 200
    assert resp.json()["tier"] == "standard"
```

**Step 2: Implement Subscription model**

```python
# app/models/subscription.py
import enum
from uuid import uuid4
from sqlalchemy import Column, String, Boolean, DateTime, ForeignKey, func
from sqlalchemy.dialects.postgresql import UUID
from app.database import Base


class SubscriptionTier(str, enum.Enum):
    FREE = "free"
    STANDARD = "standard"
    ENTERPRISE = "enterprise"


class Subscription(Base):
    __tablename__ = "subscriptions"

    id = Column(UUID(as_uuid=True), primary_key=True, default=uuid4)
    org_id = Column(UUID(as_uuid=True), ForeignKey("organizations.id"), unique=True, nullable=False)
    tier = Column(String, default=SubscriptionTier.FREE, nullable=False)
    started_at = Column(DateTime(timezone=True), server_default=func.now())
    expires_at = Column(DateTime(timezone=True), nullable=True)
    is_active = Column(Boolean, default=True)
```

**Step 3: Implement gating middleware**

```python
# app/middleware/subscription.py
from fastapi import Depends, HTTPException
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select
from uuid import UUID

from app.database import get_db
from app.models.subscription import Subscription, SubscriptionTier


TIER_ORDER = {SubscriptionTier.FREE: 0, SubscriptionTier.STANDARD: 1, SubscriptionTier.ENTERPRISE: 2}


async def get_or_create_subscription(db: AsyncSession, org_id: UUID) -> Subscription:
    result = await db.execute(select(Subscription).where(Subscription.org_id == org_id))
    sub = result.scalar_one_or_none()
    if not sub:
        sub = Subscription(org_id=org_id, tier=SubscriptionTier.FREE)
        db.add(sub)
        await db.flush()
    return sub


def require_tier(minimum: SubscriptionTier):
    async def dependency(org_id: UUID, db: AsyncSession = Depends(get_db)):
        sub = await get_or_create_subscription(db, org_id)
        if TIER_ORDER.get(sub.tier, 0) < TIER_ORDER[minimum]:
            raise HTTPException(
                status_code=403,
                detail=f"This feature requires a {minimum.value} subscription. Current tier: {sub.tier}. Contact sales to upgrade.",
            )
        return sub
    return dependency
```

**Step 4: Implement admin endpoints**

```python
# app/routers/subscriptions.py
router = APIRouter(prefix="/admin/subscriptions", tags=["admin"])

@router.get("/")
async def list_subscriptions(db = Depends(get_db), current_user = Depends(require_admin)):
    ...

@router.put("/{org_id}")
async def update_subscription(org_id: UUID, data: SubscriptionUpdate, db = Depends(get_db), current_user = Depends(require_admin)):
    ...
```

**Step 5: Apply gating to curve and export endpoints**

In `app/routers/curves.py`:
- Forward curve full depth: `require_tier(SubscriptionTier.STANDARD)` — but free tier still gets mid_price only
- CSV export: `require_tier(SubscriptionTier.STANDARD)`

In `app/routers/price_discovery.py`:
- Reference price CSV export: `require_tier(SubscriptionTier.STANDARD)`
- Reference price full date range (>7 days): `require_tier(SubscriptionTier.STANDARD)`

**Step 6: Generate migration, run tests, commit**

```bash
alembic revision --autogenerate -m "add subscriptions table"
alembic upgrade head
pytest tests/unit/test_subscription.py -v
pytest tests/ -x -q
git add -A
git commit -m "feat: subscription model with tier-based soft gating"
```

---

## Task 8: Market Activity Feed — Backend

**Files:**
- Create: `app/models/alerts.py`
- Create: `app/routers/alerts.py`
- Create: `app/routers/activity.py`
- Create: `app/services/activity.py`
- Modify: `app/routers/orderbook.py` — publish activity events on order create/cancel
- Modify: `app/routers/trades.py` — publish activity events on trade match
- Modify: `app/models/__init__.py`
- Modify: `app/main.py`
- Create: `tests/unit/test_activity_feed.py`
- Create: `tests/unit/test_price_alerts.py`
- Create: Alembic migration

**Step 1: Write failing tests for PriceAlert CRUD**

```python
# tests/unit/test_price_alerts.py
@pytest.mark.asyncio
async def test_create_price_alert(auth_client, product):
    resp = await auth_client.post("/api/v1/alerts", json={
        "product_id": str(product.id),
        "direction": "above",
        "threshold_usd": 600,
    })
    assert resp.status_code == 201
    assert resp.json()["direction"] == "above"
    assert resp.json()["threshold_usd"] == 600


@pytest.mark.asyncio
async def test_free_tier_limited_to_5_alerts(auth_client, product):
    for i in range(5):
        resp = await auth_client.post("/api/v1/alerts", json={
            "product_id": str(product.id), "direction": "above", "threshold_usd": 500 + i,
        })
        assert resp.status_code == 201
    resp = await auth_client.post("/api/v1/alerts", json={
        "product_id": str(product.id), "direction": "above", "threshold_usd": 600,
    })
    assert resp.status_code == 403  # limit reached
```

**Step 2: Implement PriceAlert model**

```python
# app/models/alerts.py
class PriceAlert(Base):
    __tablename__ = "price_alerts"
    id = Column(UUID(as_uuid=True), primary_key=True, default=uuid4)
    org_id = Column(UUID(as_uuid=True), ForeignKey("organizations.id"), nullable=False)
    product_id = Column(UUID(as_uuid=True), ForeignKey("products.id"), nullable=False)
    delivery_point_id = Column(UUID(as_uuid=True), ForeignKey("delivery_points.id"), nullable=True)
    direction = Column(String, nullable=False)  # "above" or "below"
    threshold_usd = Column(Numeric(10, 2), nullable=False)
    is_active = Column(Boolean, default=True)
    triggered_at = Column(DateTime(timezone=True), nullable=True)
    created_at = Column(DateTime(timezone=True), server_default=func.now())
```

**Step 3: Implement alerts CRUD router**

```python
# app/routers/alerts.py
@router.post("/", status_code=201)
async def create_alert(data: AlertCreate, current_user = Depends(get_current_user), db = Depends(get_db)):
    # Check alert count limit for free tier (5 max)
    ...

@router.get("/")
async def list_alerts(current_user = Depends(get_current_user), db = Depends(get_db)):
    ...

@router.delete("/{alert_id}")
async def delete_alert(alert_id: UUID, current_user = Depends(get_current_user), db = Depends(get_db)):
    ...
```

**Step 4: Implement activity service**

```python
# app/services/activity.py
from app.services.event_bus import event_bus

async def publish_new_listing(order, product, delivery_point):
    await event_bus.publish("activity", "new_listing", {
        "product_name": product.name,
        "delivery_point_name": delivery_point.name if delivery_point else None,
        "side": order.side.value,
        "quantity_mt": str(order.quantity_mt),
    })

async def publish_price_crossing(product, delivery_point):
    await event_bus.publish("activity", "price_crossing", {
        "product_name": product.name,
        "delivery_point_name": delivery_point.name if delivery_point else None,
    })

async def publish_order_outbid(org_id, order, new_bid_price):
    await event_bus.publish(f"activity:{org_id}", "order_outbid", {
        "order_id": str(order.id),
        "your_price": str(order.price_per_mt_usd),
        "new_best_price": str(new_bid_price),
    })

async def check_price_alerts(db, product_id, delivery_point_id, current_price):
    """Check all active alerts for this product/delivery point and trigger if threshold crossed."""
    ...
```

**Step 5: Implement activity SSE endpoint**

```python
# app/routers/activity.py
@router.get("/stream/activity")
async def stream_activity(request: Request, current_user = Depends(get_current_user_optional)):
    # Subscribe to "activity" channel (public)
    # If authenticated, also subscribe to f"activity:{current_user.organization_id}"
    # Merge both queues into single SSE stream
    ...
```

**Step 6: Wire activity events into existing routers**

In `app/routers/orderbook.py` — after order creation:
```python
await publish_new_listing(new_order, product, delivery_point)
```

After order cancellation:
```python
await event_bus.publish("activity", "listing_cancelled", {...})
```

In matching engine — after trade auto-match:
```python
await publish_price_crossing(product, delivery_point)
await check_price_alerts(db, product_id, delivery_point_id, trade_price)
```

**Step 7: Generate migration, run tests, commit**

```bash
alembic revision --autogenerate -m "add price_alerts table"
alembic upgrade head
pytest tests/unit/test_price_alerts.py tests/unit/test_activity_feed.py -v
pytest tests/ -x -q
git add -A
git commit -m "feat: market activity feed with price alerts and SSE stream"
```

---

## Task 9: Frontend — Forward Curve, Activity Feed, Subscription UI

**Files:**
- Create: `src/components/ForwardCurve.tsx` — chart component
- Create: `src/components/ActivityFeed.tsx` — real-time event feed
- Create: `src/components/PriceAlertManager.tsx` — alert CRUD
- Modify: `src/services/api.ts` — add curves, alerts, subscription endpoints
- Modify: `src/types.ts` — add ForwardCurvePoint, PriceAlert, Subscription types
- Modify: `src/components/MarketTerminal.tsx` — integrate forward curve + activity feed
- Modify: `src/hooks/useSSE.ts` — support multi-channel merge if needed

**Step 1: Add types**

```typescript
export interface ForwardCurvePoint {
  availability_window: string;
  mid_price: number | null;
  best_bid: number | null;
  best_ask: number | null;
  spread: number | null;
  volume_mt: number;
  order_count: number;
}

export interface ForwardCurveResponse {
  product_id: string;
  product_name: string;
  delivery_point_id?: string;
  delivery_point_name?: string;
  curve: ForwardCurvePoint[];
  generated_at: string;
}

export interface PriceAlert {
  id: string;
  product_id: string;
  delivery_point_id?: string;
  direction: 'above' | 'below';
  threshold_usd: number;
  is_active: boolean;
  triggered_at?: string;
}

export interface Subscription {
  id: string;
  org_id: string;
  tier: 'free' | 'standard' | 'enterprise';
  started_at: string;
  expires_at?: string;
  is_active: boolean;
}

export interface ActivityEvent {
  event: string;
  data: Record<string, unknown>;
  timestamp: string;
}
```

**Step 2: Add API endpoints**

```typescript
curves: {
  forward: (productId: string, deliveryPointId?: string) =>
    fetchApi(`/curves/forward?product_id=${productId}${deliveryPointId ? `&delivery_point_id=${deliveryPointId}` : ''}`),
  exportForward: (productId: string) =>
    `${API_URL}/curves/forward/export?format=csv&product_id=${productId}`,
},
alerts: {
  list: () => fetchApi('/alerts', { headers: getHeaders() }),
  create: (data: Partial<PriceAlert>) => fetchApi('/alerts', { method: 'POST', headers: getHeaders(), body: JSON.stringify(data) }),
  delete: (id: string) => fetchApi(`/alerts/${id}`, { method: 'DELETE', headers: getHeaders() }),
},
prices: {
  ...existing,
  exportReference: (productId: string) =>
    `${API_URL}/prices/reference/export?format=csv&product_id=${productId}`,
},
```

**Step 3: Build ForwardCurve component**

Use Recharts (already in the project from MarketTerminal's forward curve chart) to display:
- Bar chart: best_bid (green) and best_ask (red) per availability_window
- Line overlay: mid_price connecting the midpoints
- Tooltip with spread, volume, order count

Include "Export CSV" button linking to the export endpoint (disabled for free tier with upgrade CTA).

**Step 4: Build ActivityFeed component**

- Subscribe to `/stream/activity` via useSSE hook
- Render events as a scrollable list with icons per event type
- `new_listing` → blue dot, `price_crossing` → amber warning, `vwap_movement` → green/red arrow
- Participant events (order_outbid, price_alert, match_available) highlighted differently

**Step 5: Build PriceAlertManager component**

- List existing alerts with delete button
- "Add Alert" form: product dropdown, delivery point dropdown, direction (above/below), threshold price
- Show alert count vs limit for free tier ("3/5 alerts used")
- Upgrade CTA when at limit

**Step 6: Integrate into MarketTerminal**

- Add ForwardCurve as a tab/section alongside existing price chart
- Add ActivityFeed as a sidebar or collapsible panel
- Add PriceAlertManager accessible from a bell/alert icon

**Step 7: Frontend design skill application**

Use the `frontend-design` skill for ForwardCurve, ActivityFeed, and PriceAlertManager to ensure they match the Deep Ocean design system (abyss/ocean/sonar/bio/amber palette).

**Step 8: Run tests, commit**

```bash
npm test -- --run
git add -A
git commit -m "feat: forward curve chart, activity feed, and price alert manager"
```

---

## Task 10: Security Quick Check

**Files:**
- Modify: `vite.config.ts` (if needed)
- Modify: `app/main.py` (if needed)

**Step 1: Check source maps**

```bash
cd /home/verdaxis-prod/verdaxis-frontend
grep -i sourcemap vite.config.ts
# If sourcemap is enabled, set build.sourcemap: false
```

```bash
# Check production build output
ls dist/assets/*.map 2>/dev/null
# Should return nothing — no .map files
```

**Step 2: Check /docs endpoint**

```bash
curl -s -o /dev/null -w "%{http_code}" https://api.verdaxis.exchange/docs
# Should return 404 or 401, not 200
```

If `/docs` is publicly accessible, disable in production:
```python
# app/main.py — conditional docs
import os
docs_url = "/docs" if os.getenv("ENVIRONMENT") != "production" else None
redoc_url = "/redoc" if os.getenv("ENVIRONMENT") != "production" else None
app = FastAPI(title="Verdaxis Exchange API", docs_url=docs_url, redoc_url=redoc_url)
```

**Step 3: Commit if changes made**

```bash
git add -A
git commit -m "fix: disable source maps and /docs in production"
```

---

## Quality Gates (After All Tasks)

### Gate 1: Code Review
Run `requesting-code-review` skill on the full `feat/data-products` diff against main.

### Gate 2: Demand Elegance
Run `demand-elegance` skill — challenge the implementation:
- Is the subscription gating pattern DRY?
- Is the activity feed event schema consistent?
- Are the curve calculations correct?

### Gate 3: Dogfood (Visual UAT)
Run `dogfood` skill with these user stories:
1. Log in as buyer → place a BID using product/delivery point dropdowns → verify order appears in orderbook
2. Log in as seller → place crossing ASK → verify auto-match and activity feed shows crossing event
3. View forward curve chart → verify mid-price, bid/ask bars render
4. Click "Export CSV" → verify download works (or shows upgrade CTA for free tier)
5. Create a price alert → verify it appears in alert list
6. Check MarketTerminal → verify activity feed shows real-time events

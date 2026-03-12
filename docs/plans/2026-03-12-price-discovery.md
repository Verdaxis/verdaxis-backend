# Price Discovery Foundation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Turn Verdaxis into a transparent exchange venue for marine green fuels — live two-sided orderbook, crossing detection, quantity presets, VWAP internal/external split, and anonymized trade tape.

**Architecture:** Five independent slices: (1) frontend OrderBook component with SSE live updates, (2) backend crossing-detection flag on orderbook list response, (3) frontend quantity presets in OrderPlaceModal, (4) VWAP split on existing `/prices/reference` endpoint, (5) Trade `is_anonymous` flag with anonymized tape in MarketTerminal. No new DB migrations except for task 5. Tasks 1–4 are zero-migration.

**Tech Stack:** React + TypeScript + Vite frontend (`verdaxis-frontend`), FastAPI + SQLAlchemy async backend (`verdaxis-backend`), SSE via `/stream` endpoint (event_bus), existing `api.orderbook.listBids` / `listAsks` / `create`, `api.trades.list`, Alembic for migrations, pytest-asyncio for backend tests.

---

## Task 1: Live Two-Sided OrderBook Frontend Component

**Context:** There is currently no side-by-side orderbook view. The Marketplace shows listings one-sided. We need a new `OrderBook` component that shows bids (buy pressure) and asks (sell offers) simultaneously, auto-updating via SSE.

**Why backend flag vs frontend compute:** The OrderBook component polls both endpoints and displays them side-by-side with depth bars showing relative volume. Bids are sorted highest-first (best bid at top), asks lowest-first (best ask at top) — the exchange convention.

**Files:**
- Create: `verdaxis-frontend/src/components/OrderBook.tsx`
- Create: `verdaxis-frontend/src/components/__tests__/OrderBook.test.tsx`
- Modify: `verdaxis-frontend/src/components/Marketplace.tsx` (embed OrderBook above listing table)
- Read for reference: `verdaxis-frontend/src/services/api.ts` (existing `orderbook.listBids`, `orderbook.listAsks`)

---

### Step 1: Understand existing API surface

Read these files before writing code:
```
verdaxis-frontend/src/services/api.ts       # look for orderbook section
verdaxis-frontend/src/types/index.ts        # look for OrderBookOrder type
```

---

### Step 2: Write the failing test

File: `verdaxis-frontend/src/components/__tests__/OrderBook.test.tsx`

```tsx
import { render, screen } from '@testing-library/react';
import { OrderBook } from '../OrderBook';
import { vi } from 'vitest';

vi.mock('../../services/api', () => ({
  api: {
    orderbook: {
      listBids: vi.fn().mockResolvedValue([
        { id: '1', side: 'BID', fuel_type: 'Methanol', region: 'ARA',
          price_per_mt_usd: 520, quantity_mt: 1000, remaining_quantity_mt: 1000, status: 'OPEN' },
      ]),
      listAsks: vi.fn().mockResolvedValue([
        { id: '2', side: 'ASK', fuel_type: 'Methanol', region: 'ARA',
          price_per_mt_usd: 530, quantity_mt: 500, remaining_quantity_mt: 500, status: 'OPEN' },
      ]),
    },
  },
}));

it('renders bid and ask columns', async () => {
  render(<OrderBook />);
  expect(await screen.findByText('BIDS')).toBeInTheDocument();
  expect(await screen.findByText('ASKS')).toBeInTheDocument();
});

it('renders bid price', async () => {
  render(<OrderBook />);
  expect(await screen.findByText('$520')).toBeInTheDocument();
});

it('renders ask price', async () => {
  render(<OrderBook />);
  expect(await screen.findByText('$530')).toBeInTheDocument();
});
```

Run: `cd /home/verdaxis-prod/verdaxis-frontend && npm test -- --run src/components/__tests__/OrderBook.test.tsx`
Expected: FAIL — `Cannot find module '../OrderBook'`

---

### Step 3: Implement `OrderBook` component

File: `verdaxis-frontend/src/components/OrderBook.tsx`

```tsx
import React, { useEffect, useState, useCallback } from 'react';
import { api } from '../services/api';
import { OrderBookOrder } from '../types';
import { TrendingUp, TrendingDown } from 'lucide-react';

interface Props {
    fuelType?: string;
    region?: string;
}

export const OrderBook: React.FC<Props> = ({ fuelType, region }) => {
    const [bids, setBids] = useState<OrderBookOrder[]>([]);
    const [asks, setAsks] = useState<OrderBookOrder[]>([]);
    const [loading, setLoading] = useState(true);

    const refresh = useCallback(async () => {
        const [b, a] = await Promise.all([
            api.orderbook.listBids({ fuelType, region }),
            api.orderbook.listAsks({ fuelType, region }),
        ]);
        // Bids: highest price first (best bid on top)
        setBids([...b].sort((x, y) => y.price_per_mt_usd - x.price_per_mt_usd).slice(0, 10));
        // Asks: lowest price first (best ask on top)
        setAsks([...a].sort((x, y) => x.price_per_mt_usd - y.price_per_mt_usd).slice(0, 10));
        setLoading(false);
    }, [fuelType, region]);

    useEffect(() => {
        refresh();
        const id = setInterval(refresh, 10_000);
        return () => clearInterval(id);
    }, [refresh]);

    const maxQty = Math.max(
        ...bids.map(b => b.remaining_quantity_mt),
        ...asks.map(a => a.remaining_quantity_mt),
        1
    );

    const DepthBar: React.FC<{ qty: number; side: 'BID' | 'ASK' }> = ({ qty, side }) => (
        <div
            className={`absolute inset-y-0 ${side === 'BID'
                ? 'right-0 bg-emerald-50 dark:bg-emerald-900/20'
                : 'left-0 bg-red-50 dark:bg-red-900/20'
            }`}
            style={{ width: `${(qty / maxQty) * 100}%` }}
        />
    );

    const Row: React.FC<{ order: OrderBookOrder; side: 'BID' | 'ASK' }> = ({ order, side }) => {
        const crossed = (order as any).is_crossed;
        return (
            <div className={`relative flex justify-between items-center px-3 py-1.5 text-xs transition-colors
                ${crossed
                    ? 'bg-amber-50 dark:bg-amber-900/20 border-l-2 border-amber-400'
                    : 'hover:bg-slate-50 dark:hover:bg-slate-800/50'
                }`}>
                <DepthBar qty={order.remaining_quantity_mt} side={side} />
                <span className={`relative z-10 font-mono font-bold ${side === 'BID' ? 'text-emerald-600 dark:text-emerald-400' : 'text-red-500 dark:text-red-400'}`}>
                    ${order.price_per_mt_usd.toLocaleString()}
                    {crossed && <span className="ml-1 text-[9px] bg-amber-400 text-white px-1 rounded">⚡</span>}
                </span>
                <span className="relative z-10 text-slate-500 dark:text-slate-400 font-mono">
                    {order.remaining_quantity_mt.toLocaleString()} MT
                </span>
            </div>
        );
    };

    if (loading) {
        return (
            <div className="border border-slate-100 dark:border-slate-700 rounded-xl p-4 text-center text-xs text-slate-400">
                Loading orderbook...
            </div>
        );
    }

    return (
        <div className="border border-slate-100 dark:border-slate-700 rounded-xl overflow-hidden mb-6">
            <div className="grid grid-cols-2 divide-x divide-slate-100 dark:divide-slate-700">
                <div>
                    <div className="px-3 py-2 bg-emerald-50 dark:bg-emerald-900/20 border-b border-slate-100 dark:border-slate-700 flex items-center gap-1.5">
                        <TrendingUp size={12} className="text-emerald-600" />
                        <span className="text-[10px] font-bold text-emerald-700 dark:text-emerald-400 uppercase tracking-wider">BIDS</span>
                        <span className="ml-auto text-[10px] text-slate-400">{bids.length} orders</span>
                    </div>
                    {bids.length === 0
                        ? <div className="px-3 py-4 text-[10px] text-slate-400 text-center">No bids</div>
                        : bids.map(b => <Row key={b.id} order={b} side="BID" />)
                    }
                </div>
                <div>
                    <div className="px-3 py-2 bg-red-50 dark:bg-red-900/20 border-b border-slate-100 dark:border-slate-700 flex items-center gap-1.5">
                        <TrendingDown size={12} className="text-red-500" />
                        <span className="text-[10px] font-bold text-red-600 dark:text-red-400 uppercase tracking-wider">ASKS</span>
                        <span className="ml-auto text-[10px] text-slate-400">{asks.length} orders</span>
                    </div>
                    {asks.length === 0
                        ? <div className="px-3 py-4 text-[10px] text-slate-400 text-center">No asks</div>
                        : asks.map(a => <Row key={a.id} order={a} side="ASK" />)
                    }
                </div>
            </div>
        </div>
    );
};
```

---

### Step 4: Run test to verify it passes

Run: `cd /home/verdaxis-prod/verdaxis-frontend && npm test -- --run src/components/__tests__/OrderBook.test.tsx`
Expected: PASS (3 tests)

---

### Step 5: Embed OrderBook in Marketplace

In `Marketplace.tsx`, add import and embed between the filter bar and the listings table:

```tsx
import { OrderBook } from './OrderBook';

// Between filter section and <table>:
<OrderBook fuelType={selectedFuel || undefined} region={selectedRegion || undefined} />
```

---

### Step 6: Build check + commit

```bash
cd /home/verdaxis-prod/verdaxis-frontend && npm run build
git add src/components/OrderBook.tsx src/components/__tests__/OrderBook.test.tsx src/components/Marketplace.tsx
git commit -m "feat: add live two-sided OrderBook component to Marketplace"
```

---

## Task 2: Order Crossing Detection + Highlight

**Context:** When a bid price >= best ask price (or vice versa), orders are "crossing." Backend computes `is_crossed` on each order in the list response so the frontend can highlight without client-side cross-referencing.

**Files:**
- Create: `verdaxis-backend/tests/unit/test_crossing_detection.py`
- Modify: `verdaxis-backend/app/routers/orderbook.py` (add `compute_is_crossed`, wire into list handlers)
- Modify: `verdaxis-backend/app/schemas/orderbook.py` (add `is_crossed: bool = False` to `OrderBookOrderResponse`)

---

### Step 1: Write failing backend test

File: `verdaxis-backend/tests/unit/test_crossing_detection.py`

```python
"""Tests for crossing detection in orderbook responses."""
from decimal import Decimal

def test_bid_crosses_when_price_gte_best_ask():
    from app.routers.orderbook import compute_is_crossed
    assert compute_is_crossed("BID", Decimal("535"), Decimal("530")) is True

def test_bid_does_not_cross_when_price_lt_best_ask():
    from app.routers.orderbook import compute_is_crossed
    assert compute_is_crossed("BID", Decimal("525"), Decimal("530")) is False

def test_ask_crosses_when_price_lte_best_bid():
    from app.routers.orderbook import compute_is_crossed
    assert compute_is_crossed("ASK", Decimal("515"), Decimal("520")) is True

def test_ask_does_not_cross_when_no_opposing():
    from app.routers.orderbook import compute_is_crossed
    assert compute_is_crossed("ASK", Decimal("515"), None) is False

def test_bid_does_not_cross_when_no_opposing():
    from app.routers.orderbook import compute_is_crossed
    assert compute_is_crossed("BID", Decimal("535"), None) is False
```

Run: `cd /home/verdaxis-prod/verdaxis-backend && python -m pytest tests/unit/test_crossing_detection.py -v`
Expected: FAIL — `ImportError: cannot import name 'compute_is_crossed'`

---

### Step 2: Add `compute_is_crossed` pure function to orderbook router

Near the top of `verdaxis-backend/app/routers/orderbook.py`, after imports:

```python
from decimal import Decimal as _Decimal
from typing import Optional as _Optional

def compute_is_crossed(side: str, price: _Decimal, best_opposing_price: _Optional[_Decimal]) -> bool:
    """Returns True if this order crosses the market (bid >= best ask, or ask <= best bid)."""
    if best_opposing_price is None:
        return False
    if side == "BID":
        return price >= best_opposing_price
    return price <= best_opposing_price
```

Run: `cd /home/verdaxis-prod/verdaxis-backend && python -m pytest tests/unit/test_crossing_detection.py -v`
Expected: PASS (5 tests)

---

### Step 3: Add `is_crossed` to schema + wire into list handlers

In `verdaxis-backend/app/schemas/orderbook.py`, add to `OrderBookOrderResponse`:
```python
is_crossed: bool = False
```

In `orderbook.py` router's `list_bids` handler, after fetching the result set, compute the best opposing price and tag each row. Pattern:

```python
# After fetching bids list (before return):
best_ask_stmt = select(func.min(OrderBookOrder.price_per_mt_usd)).where(
    OrderBookOrder.side == OrderSide.ASK,
    OrderBookOrder.status == OrderStatus.OPEN,
)
best_ask_price = (await db.execute(best_ask_stmt)).scalar()

result = []
for order in orders:
    resp = OrderBookOrderResponse.model_validate(order)
    resp.is_crossed = compute_is_crossed("BID", order.price_per_mt_usd, best_ask_price)
    result.append(resp)
return result
```

Mirror this pattern in `list_asks` (using `func.max(...)` for best bid price, side="ASK").

---

### Step 4: Run full backend tests

```bash
cd /home/verdaxis-prod/verdaxis-backend && python -m pytest tests/ -v --tb=short 2>&1 | tail -20
```
Expected: all pass

---

### Step 5: Commit

```bash
cd /home/verdaxis-prod/verdaxis-backend
git add app/routers/orderbook.py app/schemas/orderbook.py tests/unit/test_crossing_detection.py
git commit -m "feat: add is_crossed flag to orderbook bid/ask list responses"
```

(The `OrderBook.tsx` Row component already handles `is_crossed` from Task 1 step 3.)

---

## Task 3: Quantity Presets in OrderPlaceModal

**Context:** Preset quantity chips (500 / 1,000 / 2,500 / 5,000 MT) speed up order entry and establish market lot conventions. No backend changes needed.

**Files:**
- Modify: `verdaxis-frontend/src/components/OrderPlaceModal.tsx`
- Create/Modify: `verdaxis-frontend/src/components/__tests__/OrderPlaceModal.test.tsx`

---

### Step 1: Write failing test

Check if `__tests__/OrderPlaceModal.test.tsx` exists. If so, add to it; otherwise create it.

```tsx
import { render, screen, fireEvent } from '@testing-library/react';
import { OrderPlaceModal } from '../OrderPlaceModal';
// Mock api as needed based on existing modal deps

it('renders quantity preset buttons', () => {
    render(<OrderPlaceModal isOpen={true} onClose={() => {}} defaultSide="BID" />);
    expect(screen.getByText('500 MT')).toBeInTheDocument();
    expect(screen.getByText('1,000 MT')).toBeInTheDocument();
    expect(screen.getByText('2,500 MT')).toBeInTheDocument();
    expect(screen.getByText('5,000 MT')).toBeInTheDocument();
});

it('clicking a preset fills the quantity input', () => {
    render(<OrderPlaceModal isOpen={true} onClose={() => {}} defaultSide="BID" />);
    fireEvent.click(screen.getByText('1,000 MT'));
    const input = screen.getByLabelText(/quantity/i) as HTMLInputElement;
    expect(input.value).toBe('1000');
});
```

Run: `cd /home/verdaxis-prod/verdaxis-frontend && npm test -- --run src/components/__tests__/OrderPlaceModal.test.tsx`
Expected: FAIL

---

### Step 2: Add preset constant + UI to `OrderPlaceModal.tsx`

Add at top of file:
```typescript
const QUANTITY_PRESETS = [
    { label: '500 MT', value: 500 },
    { label: '1,000 MT', value: 1_000 },
    { label: '2,500 MT', value: 2_500 },
    { label: '5,000 MT', value: 5_000 },
];
```

Find the quantity `<input>` in the JSX. Add preset buttons ABOVE it:

```tsx
{/* Quantity Presets */}
<div className="flex gap-2 flex-wrap mb-2">
    {QUANTITY_PRESETS.map(preset => (
        <button
            key={preset.value}
            type="button"
            onClick={() => setFormData(prev => ({ ...prev, quantity_mt: preset.value }))}
            className={`text-xs px-2.5 py-1.5 rounded-lg border transition-colors
                ${formData.quantity_mt === preset.value
                    ? (formData.side === 'BID'
                        ? 'bg-emerald-500 text-white border-emerald-500'
                        : 'bg-[#5DADE2] text-white border-[#5DADE2]')
                    : 'bg-white dark:bg-slate-800 text-slate-600 dark:text-slate-300 border-slate-200 dark:border-slate-600 hover:border-slate-400'
                }`}
        >
            {preset.label}
        </button>
    ))}
</div>
```

---

### Step 3: Run test + build + commit

```bash
cd /home/verdaxis-prod/verdaxis-frontend && npm test -- --run src/components/__tests__/OrderPlaceModal.test.tsx
npm run build
git add src/components/OrderPlaceModal.tsx src/components/__tests__/OrderPlaceModal.test.tsx
git commit -m "feat: add quantity presets (500/1000/2500/5000 MT) to order placement modal"
```

---

## Task 4: VWAP Internal vs External Split

**Context:** VWAP is the platform's data monetization lever. "Internal" tier = calculated from all platform trades (authenticated users only). "External" tier = the public benchmark product. Currently `GET /prices/reference` returns one VWAP tier with no visibility label. We add a `visibility` query param and tag each item — no data gating this sprint (that's Sprint 5 billing), just establishing the contract. Frontend shows VWAP tiles in MarketTerminal.

**Files:**
- Modify: `verdaxis-backend/app/schemas/orderbook.py` (add `visibility` to `ReferencePriceItem`)
- Modify: `verdaxis-backend/app/routers/price_discovery.py` (accept + tag `visibility` param)
- Create/Modify: `verdaxis-backend/tests/unit/test_price_discovery_router.py`
- Modify: `verdaxis-frontend/src/components/MarketTerminal.tsx` (VWAP tile strip)
- Modify: `verdaxis-frontend/src/services/api.ts` (add `prices.getReference` if missing)

---

### Step 1: Write failing backend test

Check if `tests/unit/test_price_discovery_router.py` exists. If so, append these tests:

```python
def test_reference_price_item_has_visibility_field():
    from app.schemas.orderbook import ReferencePriceItem
    from decimal import Decimal
    from datetime import date
    item = ReferencePriceItem(
        fuel_type="Methanol", region="ARA",
        vwap_usd=Decimal("525.50"), total_volume_mt=Decimal("5000"),
        trade_count=3, date=date(2026, 3, 12), visibility="internal",
    )
    assert item.visibility == "internal"

def test_reference_price_item_defaults_to_external():
    from app.schemas.orderbook import ReferencePriceItem
    from decimal import Decimal
    from datetime import date
    item = ReferencePriceItem(
        fuel_type="Methanol", region="ARA",
        vwap_usd=Decimal("525.50"), total_volume_mt=Decimal("5000"),
        trade_count=3, date=date(2026, 3, 12),
    )
    assert item.visibility == "external"

def test_get_reference_prices_accepts_visibility_param():
    import inspect
    from app.routers.price_discovery import get_reference_prices
    assert "visibility" in inspect.signature(get_reference_prices).parameters
```

Run: `cd /home/verdaxis-prod/verdaxis-backend && python -m pytest tests/unit/test_price_discovery_router.py -v`
Expected: FAIL

---

### Step 2: Add `visibility` to `ReferencePriceItem` schema

In `verdaxis-backend/app/schemas/orderbook.py`, add to `ReferencePriceItem`:

```python
from typing import Literal

class ReferencePriceItem(BaseModel):
    # ... existing fields ...
    visibility: Literal["internal", "external"] = "external"
```

---

### Step 3: Add `visibility` param to the endpoint

In `verdaxis-backend/app/routers/price_discovery.py`, update `get_reference_prices`:

```python
from typing import Literal as _Literal

@router.get("/reference", response_model=ReferencePriceResponse)
@limiter.limit("30/minute")
async def get_reference_prices(
    request: _Request,
    fuel_type: Optional[str] = Query(None),
    region: Optional[str] = Query(None),
    date_from: Optional[date] = Query(None, alias="from"),
    date_to: Optional[date] = Query(None, alias="to"),
    visibility: _Literal["internal", "external"] = Query("external"),
    db: AsyncSession = Depends(get_db),
):
    prices = await compute_reference_prices(db, date_from=date_from, date_to=date_to, fuel_type=fuel_type, region=region)
    for item in prices:
        item.visibility = visibility
    return ReferencePriceResponse(prices=prices, generated_at=datetime.now(UTC))
```

---

### Step 4: Run tests

```bash
cd /home/verdaxis-prod/verdaxis-backend && python -m pytest tests/unit/test_price_discovery_router.py tests/unit/test_price_discovery_schemas.py -v
```
Expected: all pass

---

### Step 5: Add VWAP tile strip to MarketTerminal

First read `MarketTerminal.tsx` and `api.ts` to understand existing structure.

In `api.ts`, add if missing:
```typescript
prices: {
    getReference: (params?: { visibility?: 'internal' | 'external'; fuelType?: string }) =>
        fetch(`/api/prices/reference?visibility=${params?.visibility ?? 'internal'}`)
            .then(r => r.json()),
},
```

In `MarketTerminal.tsx`:
```tsx
const [vwapTiles, setVwapTiles] = useState<{ fuel_type: string; region: string; vwap_usd: number }[]>([]);

useEffect(() => {
    api.prices.getReference({ visibility: 'internal' })
        .then(data => setVwapTiles((data.prices ?? []).slice(0, 4)))
        .catch(() => {});
}, []);

// In JSX, above the trade tape:
{vwapTiles.length > 0 && (
    <div className="flex gap-3 overflow-x-auto mb-4 pb-1">
        {vwapTiles.map((v, i) => (
            <div key={i} className="bg-slate-50 dark:bg-slate-800 border border-slate-100 dark:border-slate-700 rounded-lg px-3 py-2 text-xs flex-shrink-0 min-w-[120px]">
                <div className="text-[10px] text-slate-400 uppercase font-bold truncate">{v.fuel_type} · {v.region}</div>
                <div className="font-mono font-bold text-slate-700 dark:text-slate-200 mt-0.5">VWAP ${v.vwap_usd}</div>
                <div className="text-[9px] text-emerald-500 mt-0.5">internal</div>
            </div>
        ))}
    </div>
)}
```

---

### Step 6: Build + commit

```bash
cd /home/verdaxis-prod/verdaxis-backend
python -m pytest tests/ --tb=short 2>&1 | tail -10
git add app/routers/price_discovery.py app/schemas/orderbook.py tests/unit/test_price_discovery_router.py
git commit -m "feat: add visibility tier (internal/external) to VWAP reference price endpoint"

cd /home/verdaxis-prod/verdaxis-frontend
npm run build
git add src/components/MarketTerminal.tsx src/services/api.ts
git commit -m "feat: display internal VWAP benchmark tiles in Market Terminal"
```

---

## Task 5: Anonymized Trade Tape

**Context:** AOM's `messageAnonymised` pattern — trades show price + quantity but company names are redacted. We add `is_anonymous: bool` to `Trade` (opt-in per order), wire it through placement, and add a toggle in Market Terminal. This is the only task requiring a DB migration.

**Files:**
- Modify: `verdaxis-backend/app/models/orderbook.py` (add `is_anonymous` to Trade)
- Create: `verdaxis-backend/alembic/versions/<hash>_add_is_anonymous_to_trade.py` (auto-generated)
- Modify: `verdaxis-backend/app/schemas/orderbook.py` (add `is_anonymous` to TradeResponse + OrderBookOrderCreate)
- Modify: `verdaxis-backend/app/routers/orderbook.py` (pass through `is_anonymous` on order create → trade)
- Create: `verdaxis-backend/tests/unit/test_trade_anonymous.py`
- Modify: `verdaxis-frontend/src/components/OrderPlaceModal.tsx` (anonymous checkbox)
- Modify: `verdaxis-frontend/src/components/MarketTerminal.tsx` (tape toggle)

---

### Step 1: Write failing backend test

File: `verdaxis-backend/tests/unit/test_trade_anonymous.py`

```python
"""Tests for Trade anonymization flag."""

def test_trade_model_has_is_anonymous():
    from app.models.orderbook import Trade
    assert hasattr(Trade, "is_anonymous"), "Trade ORM model must have is_anonymous column"

def test_trade_schema_has_is_anonymous():
    from app.schemas.orderbook import TradeResponse
    assert "is_anonymous" in TradeResponse.model_fields

def test_trade_schema_defaults_is_anonymous_to_false():
    from app.schemas.orderbook import TradeResponse
    assert TradeResponse.model_fields["is_anonymous"].default is False
```

Run: `cd /home/verdaxis-prod/verdaxis-backend && python -m pytest tests/unit/test_trade_anonymous.py -v`
Expected: FAIL — `AssertionError: Trade ORM model must have is_anonymous column`

---

### Step 2: Add `is_anonymous` to Trade ORM model

In `verdaxis-backend/app/models/orderbook.py`, in the `Trade` class, add after `initiated_by`:

```python
is_anonymous: Mapped[bool] = mapped_column(Boolean, nullable=False, default=False, server_default="false")
```

---

### Step 3: Generate and review Alembic migration

```bash
cd /home/verdaxis-prod/verdaxis-backend
sudo -u verdaxis-prod bash -c "source .venv/bin/activate && alembic revision --autogenerate -m 'add_is_anonymous_to_trade'"
```

**Review the generated file.** It should contain ONLY:
```python
def upgrade() -> None:
    op.add_column('trades', sa.Column('is_anonymous', sa.Boolean(), server_default='false', nullable=False))

def downgrade() -> None:
    op.drop_column('trades', 'is_anonymous')
```

If it contains unrelated changes, edit the migration to remove them before proceeding.

---

### Step 4: Apply migration

```bash
sudo -u verdaxis-prod bash -c "source .venv/bin/activate && alembic upgrade head"
```
Expected: `Running upgrade ... -> <hash>, add_is_anonymous_to_trade`

---

### Step 5: Add `is_anonymous` to schemas

In `verdaxis-backend/app/schemas/orderbook.py`:

In `TradeResponse`:
```python
is_anonymous: bool = False
```

In `OrderBookOrderCreate` (the Pydantic model used when placing orders):
```python
is_anonymous: bool = False
```

---

### Step 6: Run schema tests

```bash
cd /home/verdaxis-prod/verdaxis-backend && python -m pytest tests/unit/test_trade_anonymous.py -v
```
Expected: PASS (3 tests)

---

### Step 7: Wire `is_anonymous` through order placement

In `verdaxis-backend/app/routers/orderbook.py`, find where `Trade(...)` is constructed (inside the match-on-insert block in `POST /`). Ensure `is_anonymous=order.is_anonymous` is passed:

```python
trade = Trade(
    # ... existing fields ...
    is_anonymous=order.is_anonymous,
)
```

---

### Step 8: Run full test suite

```bash
cd /home/verdaxis-prod/verdaxis-backend && python -m pytest tests/ --tb=short 2>&1 | tail -20
```
Expected: all pass

---

### Step 9: Add anonymous checkbox to `OrderPlaceModal.tsx`

In `OrderPlaceModal.tsx`, find `OrderFormData` interface. Add:
```typescript
is_anonymous: boolean;
```

In initial state:
```typescript
is_anonymous: false,
```

In the form JSX (before the submit button):
```tsx
{/* Anonymous Trade */}
<label className="flex items-center gap-2 text-xs text-slate-600 dark:text-slate-300 cursor-pointer select-none">
    <input
        type="checkbox"
        checked={formData.is_anonymous}
        onChange={e => setFormData(prev => ({ ...prev, is_anonymous: e.target.checked }))}
        className="rounded border-slate-300 dark:border-slate-600"
    />
    <span>
        Anonymous trade
        <span className="text-slate-400 ml-1">(hides your company from the tape)</span>
    </span>
</label>
```

In `handleSubmit`, ensure `payload.is_anonymous` is included when calling `api.orderbook.create(payload)`.

---

### Step 10: Add anonymized tape toggle to MarketTerminal

In `MarketTerminal.tsx`, add state:
```typescript
const [showAnonymized, setShowAnonymized] = useState(false);
```

Add toggle in the tape section header:
```tsx
<button
    onClick={() => setShowAnonymized(v => !v)}
    className={`text-xs px-2 py-1 rounded border transition-colors ${
        showAnonymized
            ? 'bg-slate-700 text-white border-slate-600'
            : 'border-slate-200 dark:border-slate-700 text-slate-500 hover:border-slate-400'
    }`}
>
    {showAnonymized ? 'Anonymized' : 'Show Companies'}
</button>
```

In trade row rendering, substitute counterparty display:
```tsx
// When rendering buyer/seller names:
const buyerDisplay = (showAnonymized || trade.is_anonymous) ? '——' : (trade.buyer_name ?? 'Buyer');
const sellerDisplay = (showAnonymized || trade.is_anonymous) ? '——' : (trade.seller_name ?? 'Seller');
```

---

### Step 11: Build + restart backend + commit

```bash
cd /home/verdaxis-prod/verdaxis-frontend && npm run build
sudo systemctl restart verdaxis-api
curl -s https://api.verdaxis.exchange/health | jq .status

cd /home/verdaxis-prod/verdaxis-backend
git add app/models/orderbook.py app/schemas/orderbook.py app/routers/orderbook.py \
    alembic/versions/*add_is_anonymous_to_trade.py tests/unit/test_trade_anonymous.py
git commit -m "feat: add is_anonymous flag to Trade — opt-in anonymized order placement"

cd /home/verdaxis-prod/verdaxis-frontend
git add src/components/OrderPlaceModal.tsx src/components/MarketTerminal.tsx
git commit -m "feat: anonymous trade checkbox + anonymized tape toggle in Market Terminal"
```

---

## Deployment Checklist

After all 5 tasks:

- [ ] Backend: `python -m pytest tests/ --tb=short` → all green
- [ ] Frontend: `npm run build` → no errors
- [ ] Migration: `alembic current` shows head
- [ ] Backend restarted: `sudo systemctl restart verdaxis-api`
- [ ] Smoke: Marketplace shows side-by-side OrderBook (bids left, asks right)
- [ ] Smoke: Place a BID crossing an ASK → row shows ⚡ badge
- [ ] Smoke: OrderPlaceModal has 500/1K/2.5K/5K MT preset chips
- [ ] Smoke: Market Terminal shows VWAP tiles strip
- [ ] Smoke: Market Terminal "Anonymized" toggle hides company names

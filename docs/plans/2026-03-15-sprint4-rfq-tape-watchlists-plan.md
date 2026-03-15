# Sprint 4: Argus/Venetian Feature Parity — RFQ + Trade Tape + Watchlists

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add three key features that both Argus and Venetian have: bilateral RFQ negotiation, a public trade tape for market transparency, and personal watchlists — bringing Verdaxis from "orderbook-only" to a hybrid marketplace that supports both anonymous matching and direct negotiation.

**Architecture:** Three new backend models (RFQ, RFQQuote, Watchlist + WatchlistEntry) with dedicated routers and Pydantic schemas. Frontend gets three new components (RFQPanel, TradeTape, WatchlistPanel) integrated into the unified Marketplace page. RFQ coexists alongside the orderbook — users choose their trading style.

**Tech Stack:** FastAPI, SQLAlchemy async (PostgreSQL), Alembic, React + TypeScript + Tailwind, SSE for live trade tape

---

### Task 1: RFQ Data Model + Migration

**Files:**
- Create: `app/models/rfq.py`
- Modify: `app/models/__init__.py`
- Create: `alembic/versions/rfq_2026_03_add_rfq_tables.py`
- Test: `tests/unit/test_rfq_model.py`

**Step 1: Write the failing test**

Create `tests/unit/test_rfq_model.py`:

```python
"""Unit tests for RFQ model."""
import pytest
from app.models.rfq import RFQ, RFQQuote, RFQStatus, QuoteStatus


class TestRFQStatus:
    def test_status_values(self):
        assert RFQStatus.OPEN == "OPEN"
        assert RFQStatus.QUOTED == "QUOTED"
        assert RFQStatus.ACCEPTED == "ACCEPTED"
        assert RFQStatus.EXPIRED == "EXPIRED"
        assert RFQStatus.CANCELLED == "CANCELLED"


class TestQuoteStatus:
    def test_status_values(self):
        assert QuoteStatus.PENDING == "PENDING"
        assert QuoteStatus.ACCEPTED == "ACCEPTED"
        assert QuoteStatus.DECLINED == "DECLINED"
        assert QuoteStatus.WITHDRAWN == "WITHDRAWN"


class TestRFQModel:
    def test_rfq_has_required_columns(self):
        from sqlalchemy import inspect
        mapper = inspect(RFQ)
        columns = {c.key for c in mapper.column_attrs}
        expected = {
            "id", "buyer_org_id", "product_id", "delivery_point_id",
            "quantity_mt", "target_price_per_mt",
            "availability_window", "notes",
            "status", "expires_at", "created_at",
        }
        assert expected.issubset(columns)


class TestRFQQuoteModel:
    def test_quote_has_required_columns(self):
        from sqlalchemy import inspect
        mapper = inspect(RFQQuote)
        columns = {c.key for c in mapper.column_attrs}
        expected = {
            "id", "rfq_id", "seller_org_id",
            "price_per_mt_usd", "notes",
            "status", "created_at",
        }
        assert expected.issubset(columns)
```

**Step 2: Run test to verify it fails**

Run: `cd /home/verdaxis-prod/verdaxis-backend && sudo -u verdaxis-prod .venv/bin/python -m pytest tests/unit/test_rfq_model.py -v`
Expected: FAIL with `ModuleNotFoundError: No module named 'app.models.rfq'`

**Step 3: Create the RFQ models**

Create `app/models/rfq.py`:

```python
"""RFQ (Request for Quote) models — bilateral negotiation alongside the orderbook."""
import enum
import uuid
from datetime import datetime, UTC
from decimal import Decimal

from sqlalchemy import ForeignKey, Enum, String, Numeric, DateTime, Text, Boolean
from sqlalchemy.dialects.postgresql import UUID
from sqlalchemy.orm import Mapped, mapped_column, relationship

from app.database import Base
from app.models.orderbook import AvailabilityWindow


class RFQStatus(str, enum.Enum):
    OPEN = "OPEN"
    QUOTED = "QUOTED"        # at least one quote received
    ACCEPTED = "ACCEPTED"    # buyer accepted a quote → trade created
    EXPIRED = "EXPIRED"
    CANCELLED = "CANCELLED"


class QuoteStatus(str, enum.Enum):
    PENDING = "PENDING"
    ACCEPTED = "ACCEPTED"
    DECLINED = "DECLINED"
    WITHDRAWN = "WITHDRAWN"


class RFQ(Base):
    __tablename__ = "rfqs"

    id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), primary_key=True, default=uuid.uuid4
    )
    buyer_org_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("organizations.id"), nullable=False
    )
    product_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("products.id"), nullable=False
    )
    delivery_point_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("delivery_points.id"), nullable=True
    )
    quantity_mt: Mapped[Decimal] = mapped_column(Numeric(12, 2), nullable=False)
    target_price_per_mt: Mapped[Decimal | None] = mapped_column(
        Numeric(10, 2), nullable=True, comment="Optional indicative price"
    )
    availability_window: Mapped[AvailabilityWindow] = mapped_column(
        Enum(AvailabilityWindow, native_enum=False), default=AvailabilityWindow.SPOT
    )
    notes: Mapped[str | None] = mapped_column(Text, nullable=True)
    is_anonymous: Mapped[bool] = mapped_column(Boolean, default=False)
    status: Mapped[RFQStatus] = mapped_column(
        Enum(RFQStatus, native_enum=False), default=RFQStatus.OPEN, nullable=False
    )
    expires_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), nullable=False)
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=lambda: datetime.now(UTC)
    )

    # Relationships
    quotes: Mapped[list["RFQQuote"]] = relationship(back_populates="rfq", cascade="all, delete-orphan")


class RFQQuote(Base):
    __tablename__ = "rfq_quotes"

    id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), primary_key=True, default=uuid.uuid4
    )
    rfq_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("rfqs.id"), nullable=False
    )
    seller_org_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("organizations.id"), nullable=False
    )
    price_per_mt_usd: Mapped[Decimal] = mapped_column(Numeric(10, 2), nullable=False)
    notes: Mapped[str | None] = mapped_column(Text, nullable=True)
    status: Mapped[QuoteStatus] = mapped_column(
        Enum(QuoteStatus, native_enum=False), default=QuoteStatus.PENDING, nullable=False
    )
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=lambda: datetime.now(UTC)
    )

    rfq: Mapped["RFQ"] = relationship(back_populates="quotes")
```

**Step 4: Update models __init__.py**

Add: `from app.models.rfq import RFQ, RFQQuote, RFQStatus, QuoteStatus`

**Step 5: Create Alembic migration**

Create `alembic/versions/rfq_2026_03_add_rfq_tables.py`:

```python
"""add rfq and rfq_quotes tables

Revision ID: rfq_2026_03
Revises: ref_2026_03
Create Date: 2026-03-15
"""
from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects.postgresql import UUID

revision = "rfq_2026_03"
down_revision = "ref_2026_03"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "rfqs",
        sa.Column("id", UUID(as_uuid=True), nullable=False),
        sa.Column("buyer_org_id", UUID(as_uuid=True), nullable=False),
        sa.Column("product_id", UUID(as_uuid=True), nullable=False),
        sa.Column("delivery_point_id", UUID(as_uuid=True), nullable=True),
        sa.Column("quantity_mt", sa.Numeric(12, 2), nullable=False),
        sa.Column("target_price_per_mt", sa.Numeric(10, 2), nullable=True),
        sa.Column("availability_window", sa.String(), nullable=False, server_default="Spot"),
        sa.Column("notes", sa.Text(), nullable=True),
        sa.Column("is_anonymous", sa.Boolean(), nullable=False, server_default="false"),
        sa.Column("status", sa.String(), nullable=False, server_default="OPEN"),
        sa.Column("expires_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("created_at", sa.DateTime(timezone=True), server_default=sa.text("now()"), nullable=False),
        sa.ForeignKeyConstraint(["buyer_org_id"], ["organizations.id"]),
        sa.ForeignKeyConstraint(["product_id"], ["products.id"]),
        sa.ForeignKeyConstraint(["delivery_point_id"], ["delivery_points.id"]),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index("ix_rfqs_buyer_org_id", "rfqs", ["buyer_org_id"])
    op.create_index("ix_rfqs_status", "rfqs", ["status"])

    op.create_table(
        "rfq_quotes",
        sa.Column("id", UUID(as_uuid=True), nullable=False),
        sa.Column("rfq_id", UUID(as_uuid=True), nullable=False),
        sa.Column("seller_org_id", UUID(as_uuid=True), nullable=False),
        sa.Column("price_per_mt_usd", sa.Numeric(10, 2), nullable=False),
        sa.Column("notes", sa.Text(), nullable=True),
        sa.Column("status", sa.String(), nullable=False, server_default="PENDING"),
        sa.Column("created_at", sa.DateTime(timezone=True), server_default=sa.text("now()"), nullable=False),
        sa.ForeignKeyConstraint(["rfq_id"], ["rfqs.id"]),
        sa.ForeignKeyConstraint(["seller_org_id"], ["organizations.id"]),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index("ix_rfq_quotes_rfq_id", "rfq_quotes", ["rfq_id"])


def downgrade() -> None:
    op.drop_index("ix_rfq_quotes_rfq_id", table_name="rfq_quotes")
    op.drop_table("rfq_quotes")
    op.drop_index("ix_rfqs_status", table_name="rfqs")
    op.drop_index("ix_rfqs_buyer_org_id", table_name="rfqs")
    op.drop_table("rfqs")
```

**Step 6: Run tests**

Run: `cd /home/verdaxis-prod/verdaxis-backend && sudo -u verdaxis-prod .venv/bin/python -m pytest tests/unit/test_rfq_model.py -v`
Expected: 4 PASSED

**Step 7: Run migration**

Run: `cd /home/verdaxis-prod/verdaxis-backend && sudo -u verdaxis-prod alembic upgrade head`

**Step 8: Commit**

```bash
cd /home/verdaxis-prod/verdaxis-backend
sudo -u verdaxis-prod git add app/models/rfq.py app/models/__init__.py \
    alembic/versions/rfq_2026_03_add_rfq_tables.py tests/unit/test_rfq_model.py
sudo -u verdaxis-prod git commit -m "feat: RFQ data model + Alembic migration (rfqs + rfq_quotes tables)"
```

---

### Task 2: RFQ Schemas + Router (6 Endpoints)

**Files:**
- Create: `app/schemas/rfq.py`
- Create: `app/routers/rfq.py`
- Modify: `app/main.py`
- Test: `tests/unit/test_rfq_schemas.py`

**Step 1: Write schemas**

Create `app/schemas/rfq.py`:

```python
"""Pydantic schemas for RFQ endpoints."""
from datetime import datetime
from decimal import Decimal
from typing import Optional
from uuid import UUID
from pydantic import BaseModel, Field


class RFQCreateRequest(BaseModel):
    product_id: UUID
    delivery_point_id: Optional[UUID] = None
    quantity_mt: Decimal = Field(gt=0, le=100000)
    target_price_per_mt: Optional[Decimal] = Field(None, gt=0)
    availability_window: str = "Spot"
    notes: Optional[str] = Field(None, max_length=500)
    is_anonymous: bool = False
    expires_in_hours: int = Field(default=24, ge=1, le=168)  # 1h-7d


class RFQQuoteRequest(BaseModel):
    price_per_mt_usd: Decimal = Field(gt=0)
    notes: Optional[str] = Field(None, max_length=500)


class RFQQuoteResponse(BaseModel):
    id: UUID
    seller_org_id: UUID
    seller_org_name: Optional[str] = None
    price_per_mt_usd: Decimal
    notes: Optional[str] = None
    status: str
    created_at: datetime

    class Config:
        from_attributes = True


class RFQResponse(BaseModel):
    id: UUID
    buyer_org_id: UUID
    buyer_org_name: Optional[str] = None
    product_id: UUID
    product_name: Optional[str] = None
    delivery_point_id: Optional[UUID] = None
    delivery_point_name: Optional[str] = None
    quantity_mt: Decimal
    target_price_per_mt: Optional[Decimal] = None
    availability_window: str
    notes: Optional[str] = None
    is_anonymous: bool
    status: str
    expires_at: datetime
    created_at: datetime
    quote_count: int = 0
    quotes: list[RFQQuoteResponse] = []

    class Config:
        from_attributes = True


class RFQListResponse(BaseModel):
    items: list[RFQResponse]
    total: int
```

**Step 2: Create the RFQ router**

Create `app/routers/rfq.py` with 6 endpoints:

1. `POST /rfq` — create RFQ (buyer only)
2. `GET /rfq` — list RFQs (buyers see own, suppliers see open RFQs for their product categories)
3. `GET /rfq/{id}` — get RFQ detail with quotes
4. `POST /rfq/{id}/quote` — submit quote (supplier only)
5. `POST /rfq/{id}/accept/{quote_id}` — accept quote → creates Trade (buyer only)
6. `POST /rfq/{id}/cancel` — cancel RFQ (buyer only)

The implementer should read the existing patterns in `app/routers/trades.py` and `app/routers/orderbook.py` for auth, error handling, and response patterns.

Key implementation details:
- Auth: `get_current_user` dependency from `auth_simple.py`
- Accept quote creates a Trade via `api.trades.initiate` pattern
- Expired RFQ check: if `expires_at < now`, return 400
- Supplier can only quote if their org sells the RFQ's product type
- Buyer cannot quote their own RFQ
- When a quote is accepted, all other quotes on the RFQ are set to DECLINED

**Step 3: Mount the router in main.py**

```python
from app.routers.rfq import router as rfq_router
app.include_router(rfq_router, prefix=settings.API_V1_STR)
```

**Step 4: Write schema tests, run all tests, commit**

---

### Task 3: Trade Tape Model + API

**Files:**
- Create: `app/routers/trade_tape.py`
- Create: `app/schemas/trade_tape.py`
- Modify: `app/main.py`
- Test: `tests/unit/test_trade_tape.py`

The trade tape is a READ-ONLY view of confirmed trades, anonymized for public consumption. No new data model needed — it queries the existing `trades` table with anonymization.

**Step 1: Create schemas**

Create `app/schemas/trade_tape.py`:

```python
"""Schemas for the public trade tape (anonymized confirmed trades)."""
from datetime import datetime
from decimal import Decimal
from typing import Optional
from pydantic import BaseModel


class TradeTapeEntry(BaseModel):
    id: str  # shortened UUID (first 8 chars)
    fuel_type: str
    fuel_grade: str
    region: str
    quantity_mt: Decimal
    price_per_mt_usd: Decimal
    total_usd: Decimal
    confirmed_at: datetime
    availability_window: str


class TradeTapeResponse(BaseModel):
    items: list[TradeTapeEntry]
    total: int
    market_hours: bool  # true if within 08:00-18:00 UTC
```

**Step 2: Create the trade tape router**

Create `app/routers/trade_tape.py` with 1 endpoint:

`GET /trade-tape` — returns last 24h of confirmed trades, anonymized (no counterparty names, only fuel type + region + price + volume). Optional filters: `fuel_type`, `region`, `limit`, `offset`.

During market hours (08:00-18:00 UTC): real-time. After hours: delayed by 1 hour.

**Step 3: Mount, test, commit**

---

### Task 4: Watchlist Model + API

**Files:**
- Create: `app/models/watchlist.py`
- Create: `app/schemas/watchlist.py`
- Create: `app/routers/watchlists.py`
- Create: `alembic/versions/wl_2026_03_add_watchlists.py`
- Modify: `app/models/__init__.py`
- Modify: `app/main.py`
- Test: `tests/unit/test_watchlist_model.py`

**Step 1: Create watchlist model**

```python
class Watchlist(Base):
    __tablename__ = "watchlists"
    id, user_id, name, created_at

class WatchlistEntry(Base):
    __tablename__ = "watchlist_entries"
    id, watchlist_id, product_id, delivery_point_id, created_at
```

**Step 2: Create router with 5 CRUD endpoints**

1. `GET /watchlists` — list user's watchlists
2. `POST /watchlists` — create watchlist (name)
3. `POST /watchlists/{id}/entries` — add product to watchlist
4. `DELETE /watchlists/{id}/entries/{entry_id}` — remove entry
5. `DELETE /watchlists/{id}` — delete watchlist

**Step 3: Migration, tests, mount, commit**

---

### Task 5: Frontend — RFQ Panel Component

**Files:**
- Create: `/home/verdaxis-prod/verdaxis-frontend/src/components/RFQPanel.tsx`
- Modify: `src/components/Marketplace.tsx` (add RFQ tab/section)
- Modify: `src/services/api.ts` (add rfq.* endpoints)
- Modify: `src/types.ts` (add RFQ types)

Build a tabbed panel in the Marketplace that shows "Orderbook" (current) and "RFQ" (new). The RFQ tab shows:
- Buyer: "Create RFQ" button + list of my RFQs with quote status
- Supplier: list of open RFQs matching my product types + "Submit Quote" button

The implementer should follow the existing Marketplace visual design (v-card, emerald accents, dark mode support).

---

### Task 6: Frontend — Trade Tape Component

**Files:**
- Create: `/home/verdaxis-prod/verdaxis-frontend/src/components/TradeTape.tsx`
- Modify: `src/components/Marketplace.tsx` (add trade tape section below orderbook)
- Modify: `src/services/api.ts` (add tradeTape.* endpoint)

A compact, real-time scrolling ticker of recent trades. Shown below the OrderBook in the Marketplace. Each entry shows: fuel type badge + quantity + price + time ago. Auto-refreshes every 30s.

---

### Task 7: Frontend — Watchlist Panel

**Files:**
- Create: `/home/verdaxis-prod/verdaxis-frontend/src/components/WatchlistPanel.tsx`
- Modify: `src/components/layout/Sidebar.tsx` (add Watchlist nav item)
- Modify: `src/App.tsx` (add WATCHLIST page route)
- Modify: `src/services/api.ts` (add watchlist.* endpoints)
- Modify: `src/types.ts` (add Watchlist types)

A dedicated page (or sidebar panel) showing saved products with current best bid/ask prices. Users can add products from the Marketplace via a "Watch" button on each listing row.

---

### Task 8: Integration Tests + Deploy

**Step 1: Run all backend tests**

Run: `cd /home/verdaxis-prod/verdaxis-backend && sudo -u verdaxis-prod .venv/bin/python -m pytest tests/unit/ -v`

**Step 2: Run migration on production**

Run: `cd /home/verdaxis-prod/verdaxis-backend && sudo -u verdaxis-prod alembic upgrade head`

**Step 3: Restart backend**

Run: `sudo systemctl restart verdaxis-backend`

**Step 4: Build frontend**

Run: `cd /home/verdaxis-prod/verdaxis-frontend && sudo -u verdaxis-prod npm run build`

**Step 5: Visual verification**

Dogfood buyer and supplier flows for RFQ, trade tape, and watchlists.

**Step 6: Final commit**

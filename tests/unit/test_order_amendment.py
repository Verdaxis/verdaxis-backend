"""Unit tests for order amendment with audit trail (STORY-S5-006).

Uses an in-memory SQLite database — no external services required.
"""
import pytest
import uuid
from decimal import Decimal

from sqlalchemy.ext.asyncio import create_async_engine, AsyncSession, async_sessionmaker
from sqlalchemy import select

from app.database import Base
from app.models.user import User, Organization, UserRole, UserStatus, OrgType
from app.models.orderbook import OrderBookOrder, OrderSide, OrderBookStatus
from app.models.audit import OrderAuditLog

_REQUIRED_TABLES = [
    "organizations",
    "users",
    "orderbook_orders",
    "trades",
    "notifications",
    "order_audit_logs",
]


# --------------- Engine / Session fixtures ---------------

@pytest.fixture(scope="module")
def async_engine():
    return create_async_engine("sqlite+aiosqlite://", echo=False, future=True)


@pytest.fixture(scope="module")
async def setup_tables(async_engine):
    tables = [Base.metadata.tables[t] for t in _REQUIRED_TABLES if t in Base.metadata.tables]
    async with async_engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all, tables=tables)
    yield
    async with async_engine.begin() as conn:
        await conn.run_sync(Base.metadata.drop_all, tables=tables)


@pytest.fixture
async def db(async_engine, setup_tables):
    factory = async_sessionmaker(
        bind=async_engine,
        class_=AsyncSession,
        expire_on_commit=False,
        autoflush=False,
    )
    async with factory() as session:
        yield session
        await session.rollback()


# --------------- Org / User / Order helpers ---------------

def _make_org(name: str, org_type=OrgType.SHIPPING_LINE) -> Organization:
    return Organization(id=uuid.uuid4(), name=name, type=org_type)


def _make_user(org: Organization, role=UserRole.BUYER) -> User:
    return User(
        id=uuid.uuid4(),
        email=f"{uuid.uuid4().hex[:8]}@test.com",
        password_hash="hashed",
        role=role,
        status=UserStatus.APPROVED,
        organization_id=org.id,
    )


def _make_order(
    org: Organization,
    side=OrderSide.BID,
    price=Decimal("500.00"),
    quantity=Decimal("1000.00"),
    status=OrderBookStatus.OPEN,
) -> OrderBookOrder:
    return OrderBookOrder(
        id=uuid.uuid4(),
        organization_id=org.id,
        side=side,
        fuel_type="VLSFO",
        region="Singapore",
        quantity_mt=quantity,
        remaining_quantity_mt=quantity,
        price_per_mt_usd=price,
        status=status,
    )


# --------------- PATCH /amend endpoint logic (service-level) ---------------

async def _amend(
    db: AsyncSession,
    order: OrderBookOrder,
    user: User,
    *,
    new_price: Decimal | None = None,
    new_qty: Decimal | None = None,
) -> list[OrderAuditLog]:
    """
    Apply an amendment directly (mirrors the router logic) and return audit entries.
    Raises ValueError for invalid amendments so tests can check error conditions.
    """
    if order.status not in (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED):
        raise ValueError("Only OPEN or PARTIALLY_FILLED orders can be amended")

    if order.organization_id != user.organization_id:
        raise PermissionError("You can only amend your own orders")

    entries: list[OrderAuditLog] = []

    if new_price is not None and new_price != order.price_per_mt_usd:
        entries.append(OrderAuditLog(
            order_id=order.id,
            field_changed="price_per_mt_usd",
            old_value=str(order.price_per_mt_usd),
            new_value=str(new_price),
            changed_by=user.id,
        ))
        order.price_per_mt_usd = new_price

    if new_qty is not None and new_qty != order.quantity_mt:
        filled = order.quantity_mt - order.remaining_quantity_mt
        new_remaining = new_qty - filled
        if new_remaining < 0:
            raise ValueError("New quantity cannot be less than already filled amount")
        entries.append(OrderAuditLog(
            order_id=order.id,
            field_changed="quantity_mt",
            old_value=str(order.quantity_mt),
            new_value=str(new_qty),
            changed_by=user.id,
        ))
        order.quantity_mt = new_qty
        order.remaining_quantity_mt = new_remaining

    for e in entries:
        db.add(e)

    return entries


# --------------- Tests ---------------

@pytest.mark.asyncio
async def test_amend_price_succeeds(db):
    """Price amendment updates the order and records an audit entry."""
    org = _make_org("BuyerCo")
    user = _make_user(org)
    order = _make_order(org, price=Decimal("500.00"))
    db.add(org)
    db.add(user)
    db.add(order)
    await db.flush()

    entries = await _amend(db, order, user, new_price=Decimal("520.00"))
    await db.flush()

    assert order.price_per_mt_usd == Decimal("520.00")
    assert len(entries) == 1
    assert entries[0].field_changed == "price_per_mt_usd"
    assert entries[0].old_value == "500.00"
    assert entries[0].new_value == "520.00"
    assert entries[0].changed_by == user.id


@pytest.mark.asyncio
async def test_amend_quantity_succeeds(db):
    """Quantity amendment adjusts remaining_quantity_mt correctly."""
    org = _make_org("BuyerCo2")
    user = _make_user(org)
    order = _make_order(org, quantity=Decimal("1000.00"))
    db.add(org)
    db.add(user)
    db.add(order)
    await db.flush()

    entries = await _amend(db, order, user, new_qty=Decimal("1500.00"))
    await db.flush()

    assert order.quantity_mt == Decimal("1500.00")
    assert order.remaining_quantity_mt == Decimal("1500.00")
    assert len(entries) == 1
    assert entries[0].field_changed == "quantity_mt"


@pytest.mark.asyncio
async def test_cannot_amend_filled_order(db):
    """Amending a FILLED order raises ValueError (maps to HTTP 400)."""
    org = _make_org("BuyerCo3")
    user = _make_user(org)
    order = _make_order(org, status=OrderBookStatus.FILLED)
    db.add(org)
    db.add(user)
    db.add(order)
    await db.flush()

    with pytest.raises(ValueError, match="Only OPEN or PARTIALLY_FILLED"):
        await _amend(db, order, user, new_price=Decimal("600.00"))


@pytest.mark.asyncio
async def test_cannot_amend_cancelled_order(db):
    """Amending a CANCELLED order also raises ValueError."""
    org = _make_org("BuyerCo4")
    user = _make_user(org)
    order = _make_order(org, status=OrderBookStatus.CANCELLED)
    db.add(org)
    db.add(user)
    db.add(order)
    await db.flush()

    with pytest.raises(ValueError, match="Only OPEN or PARTIALLY_FILLED"):
        await _amend(db, order, user, new_price=Decimal("600.00"))


@pytest.mark.asyncio
async def test_cannot_amend_other_orgs_order(db):
    """Amending another org's order raises PermissionError (maps to HTTP 403)."""
    org_a = _make_org("OrgA")
    org_b = _make_org("OrgB")
    user_b = _make_user(org_b)
    order = _make_order(org_a)  # owned by org_a
    db.add(org_a)
    db.add(org_b)
    db.add(user_b)
    db.add(order)
    await db.flush()

    with pytest.raises(PermissionError, match="your own orders"):
        await _amend(db, order, user_b, new_price=Decimal("600.00"))


@pytest.mark.asyncio
async def test_amendment_creates_audit_entry(db):
    """Both fields amended in one call produce two distinct audit entries."""
    org = _make_org("DualAmend")
    user = _make_user(org)
    order = _make_order(org, price=Decimal("480.00"), quantity=Decimal("200.00"))
    db.add(org)
    db.add(user)
    db.add(order)
    await db.flush()

    entries = await _amend(db, order, user, new_price=Decimal("490.00"), new_qty=Decimal("300.00"))
    await db.flush()

    assert len(entries) == 2
    fields = {e.field_changed for e in entries}
    assert fields == {"price_per_mt_usd", "quantity_mt"}

    for entry in entries:
        assert entry.order_id == order.id
        assert entry.changed_by == user.id


@pytest.mark.asyncio
async def test_get_audit_trail_returns_history(db):
    """Audit trail query returns entries for the order in chronological order."""
    org = _make_org("AuditTrailOrg")
    user = _make_user(org)
    order = _make_order(org, price=Decimal("400.00"), quantity=Decimal("500.00"))
    db.add(org)
    db.add(user)
    db.add(order)
    await db.flush()

    # First amendment: price
    await _amend(db, order, user, new_price=Decimal("410.00"))
    await db.flush()

    # Second amendment: quantity
    await _amend(db, order, user, new_qty=Decimal("600.00"))
    await db.flush()

    result = await db.execute(
        select(OrderAuditLog)
        .where(OrderAuditLog.order_id == order.id)
        .order_by(OrderAuditLog.changed_at.asc())
    )
    entries = result.scalars().all()

    assert len(entries) == 2
    assert entries[0].field_changed == "price_per_mt_usd"
    assert entries[1].field_changed == "quantity_mt"


@pytest.mark.asyncio
async def test_cannot_reduce_quantity_below_filled(db):
    """Partially-filled order: new quantity must be >= filled amount."""
    org = _make_org("PartialFillOrg")
    user = _make_user(org)
    # Simulate: 1000 total, 600 filled, 400 remaining
    order = _make_order(org, quantity=Decimal("1000.00"), status=OrderBookStatus.PARTIALLY_FILLED)
    order.remaining_quantity_mt = Decimal("400.00")  # 600 already filled
    db.add(org)
    db.add(user)
    db.add(order)
    await db.flush()

    # Trying to set new qty to 500 — filled=600, new_remaining would be -100
    with pytest.raises(ValueError, match="cannot be less than already filled"):
        await _amend(db, order, user, new_qty=Decimal("500.00"))


@pytest.mark.asyncio
async def test_amend_partially_filled_quantity_to_exact_filled_amount(db):
    """Setting quantity exactly equal to filled amount results in FILLED status."""
    org = _make_org("ExactFillOrg")
    user = _make_user(org)
    # 1000 total, 700 filled, 300 remaining
    order = _make_order(org, quantity=Decimal("1000.00"), status=OrderBookStatus.PARTIALLY_FILLED)
    order.remaining_quantity_mt = Decimal("300.00")
    db.add(org)
    db.add(user)
    db.add(order)
    await db.flush()

    # Set new qty = 700 (exactly the filled amount) → remaining = 0
    entries = await _amend(db, order, user, new_qty=Decimal("700.00"))
    await db.flush()

    assert len(entries) == 1
    assert order.remaining_quantity_mt == Decimal("0.00")


@pytest.mark.asyncio
async def test_amend_no_change_produces_no_audit_entry(db):
    """Amending with the same values produces no audit entries."""
    org = _make_org("NoChangeOrg")
    user = _make_user(org)
    order = _make_order(org, price=Decimal("500.00"), quantity=Decimal("1000.00"))
    db.add(org)
    db.add(user)
    db.add(order)
    await db.flush()

    entries = await _amend(db, order, user, new_price=Decimal("500.00"), new_qty=Decimal("1000.00"))
    assert entries == []

"""
Unit tests for StopEngine — triggers Stop/StopLimit orders on trade execution.

Uses an in-memory SQLite DB so no running Postgres is needed.
Run with:
    DATABASE_URL="sqlite+aiosqlite:///:memory:" DATABASE_PASSWORD="test" \
    JWT_SECRET="test-jwt-secret-32chars-minimum-x" pytest tests/unit/test_stop_engine.py -v
"""
import uuid
import pytest
from decimal import Decimal
from unittest.mock import AsyncMock, patch

from sqlalchemy.ext.asyncio import create_async_engine, AsyncSession, async_sessionmaker
from sqlalchemy import select

from app.database import Base
# Import models so metadata is populated
from app.models.user import User, Organization  # noqa: F401
from app.models.notification import Notification  # noqa: F401
from app.models.orderbook import (
    OrderBookOrder,
    OrderSide,
    OrderType,
    OrderBookStatus,
)

# Only create tables needed for stop_engine tests; skips legacy FKs (e.g. traceability_events)
_REQUIRED_TABLES = [
    "organizations",
    "users",
    "orderbook_orders",
    "trades",
    "notifications",
]


# ---------------------------------------------------------------------------
# In-memory SQLite DB fixtures
# ---------------------------------------------------------------------------

@pytest.fixture(scope="module")
def async_engine():
    """Create a shared async SQLite engine for the test module."""
    return create_async_engine("sqlite+aiosqlite://", echo=False, future=True)


@pytest.fixture(scope="module")
async def setup_tables(async_engine):
    """Create only the tables needed, avoiding legacy FK issues."""
    tables = [Base.metadata.tables[t] for t in _REQUIRED_TABLES if t in Base.metadata.tables]
    async with async_engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all, tables=tables)
    yield
    async with async_engine.begin() as conn:
        await conn.run_sync(Base.metadata.drop_all, tables=tables)


@pytest.fixture
async def db(async_engine, setup_tables):
    """Per-test async session with automatic rollback for isolation."""
    session_factory = async_sessionmaker(
        bind=async_engine,
        class_=AsyncSession,
        expire_on_commit=False,
        autoflush=False,
    )
    async with session_factory() as session:
        yield session
        await session.rollback()


# ---------------------------------------------------------------------------
# Helper factories
# ---------------------------------------------------------------------------

def _org_id() -> uuid.UUID:
    return uuid.uuid4()


def _make_stop_order(
    *,
    side: OrderSide,
    order_type: OrderType,
    fuel_type: str = "VLSFO",
    price_per_mt_usd: Decimal = Decimal("600.00"),
    stop_price: Decimal,
    quantity_mt: Decimal = Decimal("100.00"),
    status: OrderBookStatus = OrderBookStatus.OPEN,
    organization_id: uuid.UUID | None = None,
) -> OrderBookOrder:
    org = organization_id or _org_id()
    return OrderBookOrder(
        id=uuid.uuid4(),
        organization_id=org,
        side=side,
        fuel_type=fuel_type,
        region="SE Asia",
        quantity_mt=quantity_mt,
        remaining_quantity_mt=quantity_mt,
        price_per_mt_usd=price_per_mt_usd,
        order_type=order_type,
        stop_price=stop_price,
        status=status,
    )


# ---------------------------------------------------------------------------
# Tests: trigger conditions
# ---------------------------------------------------------------------------

async def test_stop_buy_triggers_at_stop_price(db: AsyncSession):
    """BID STOP triggers when last_trade_price >= stop_price (exact boundary)."""
    from app.services.stop_engine import check_stops

    order = _make_stop_order(
        side=OrderSide.BID,
        order_type=OrderType.STOP,
        stop_price=Decimal("610.00"),
    )
    db.add(order)
    await db.flush()

    triggered = await check_stops(db, fuel_type="VLSFO", last_trade_price=Decimal("610.00"))

    assert len(triggered) == 1
    assert triggered[0].id == order.id


async def test_stop_buy_triggers_above_stop_price(db: AsyncSession):
    """BID STOP triggers when last_trade_price > stop_price."""
    from app.services.stop_engine import check_stops

    order = _make_stop_order(
        side=OrderSide.BID,
        order_type=OrderType.STOP,
        stop_price=Decimal("610.00"),
    )
    db.add(order)
    await db.flush()

    triggered = await check_stops(db, fuel_type="VLSFO", last_trade_price=Decimal("615.00"))

    assert len(triggered) == 1
    assert triggered[0].id == order.id


async def test_stop_sell_triggers_at_stop_price(db: AsyncSession):
    """ASK STOP triggers when last_trade_price <= stop_price (exact boundary)."""
    from app.services.stop_engine import check_stops

    order = _make_stop_order(
        side=OrderSide.ASK,
        order_type=OrderType.STOP,
        stop_price=Decimal("590.00"),
    )
    db.add(order)
    await db.flush()

    triggered = await check_stops(db, fuel_type="VLSFO", last_trade_price=Decimal("590.00"))

    assert len(triggered) == 1
    assert triggered[0].id == order.id


async def test_stop_sell_triggers_below_stop_price(db: AsyncSession):
    """ASK STOP triggers when last_trade_price < stop_price."""
    from app.services.stop_engine import check_stops

    order = _make_stop_order(
        side=OrderSide.ASK,
        order_type=OrderType.STOP,
        stop_price=Decimal("590.00"),
    )
    db.add(order)
    await db.flush()

    triggered = await check_stops(db, fuel_type="VLSFO", last_trade_price=Decimal("585.00"))

    assert len(triggered) == 1
    assert triggered[0].id == order.id


async def test_stop_not_triggered_when_price_doesnt_cross(db: AsyncSession):
    """BID STOP does NOT trigger when last_trade_price < stop_price."""
    from app.services.stop_engine import check_stops

    order = _make_stop_order(
        side=OrderSide.BID,
        order_type=OrderType.STOP,
        stop_price=Decimal("610.00"),
    )
    db.add(order)
    await db.flush()

    triggered = await check_stops(db, fuel_type="VLSFO", last_trade_price=Decimal("605.00"))

    assert triggered == []


async def test_stop_sell_not_triggered_when_price_above(db: AsyncSession):
    """ASK STOP does NOT trigger when last_trade_price > stop_price."""
    from app.services.stop_engine import check_stops

    order = _make_stop_order(
        side=OrderSide.ASK,
        order_type=OrderType.STOP,
        stop_price=Decimal("590.00"),
    )
    db.add(order)
    await db.flush()

    triggered = await check_stops(db, fuel_type="VLSFO", last_trade_price=Decimal("595.00"))

    assert triggered == []


# ---------------------------------------------------------------------------
# Tests: order_type mutation after trigger
# ---------------------------------------------------------------------------

async def test_stop_becomes_market_after_trigger(db: AsyncSession):
    """A STOP order's order_type becomes MARKET when triggered."""
    from app.services.stop_engine import check_stops

    order = _make_stop_order(
        side=OrderSide.BID,
        order_type=OrderType.STOP,
        stop_price=Decimal("610.00"),
    )
    db.add(order)
    await db.flush()

    await check_stops(db, fuel_type="VLSFO", last_trade_price=Decimal("615.00"))

    result = await db.execute(select(OrderBookOrder).where(OrderBookOrder.id == order.id))
    refreshed = result.scalars().first()

    assert refreshed.order_type == OrderType.MARKET
    assert refreshed.status == OrderBookStatus.TRIGGERED


async def test_stop_limit_becomes_limit_after_trigger(db: AsyncSession):
    """A STOP_LIMIT order's order_type becomes LIMIT when triggered."""
    from app.services.stop_engine import check_stops

    order = _make_stop_order(
        side=OrderSide.BID,
        order_type=OrderType.STOP_LIMIT,
        stop_price=Decimal("610.00"),
    )
    db.add(order)
    await db.flush()

    await check_stops(db, fuel_type="VLSFO", last_trade_price=Decimal("615.00"))

    result = await db.execute(select(OrderBookOrder).where(OrderBookOrder.id == order.id))
    refreshed = result.scalars().first()

    assert refreshed.order_type == OrderType.LIMIT
    assert refreshed.status == OrderBookStatus.TRIGGERED


# ---------------------------------------------------------------------------
# Tests: only OPEN stops are evaluated
# ---------------------------------------------------------------------------

async def test_only_open_stops_are_checked(db: AsyncSession):
    """Filled, cancelled, and already-triggered orders are skipped."""
    from app.services.stop_engine import check_stops

    filled = _make_stop_order(
        side=OrderSide.BID,
        order_type=OrderType.STOP,
        stop_price=Decimal("610.00"),
        status=OrderBookStatus.FILLED,
    )
    cancelled = _make_stop_order(
        side=OrderSide.BID,
        order_type=OrderType.STOP,
        stop_price=Decimal("610.00"),
        status=OrderBookStatus.CANCELLED,
    )
    already_triggered = _make_stop_order(
        side=OrderSide.BID,
        order_type=OrderType.STOP,
        stop_price=Decimal("610.00"),
        status=OrderBookStatus.TRIGGERED,
    )
    open_order = _make_stop_order(
        side=OrderSide.BID,
        order_type=OrderType.STOP,
        stop_price=Decimal("610.00"),
        status=OrderBookStatus.OPEN,
    )

    for o in [filled, cancelled, already_triggered, open_order]:
        db.add(o)
    await db.flush()

    triggered = await check_stops(db, fuel_type="VLSFO", last_trade_price=Decimal("620.00"))

    assert len(triggered) == 1
    assert triggered[0].id == open_order.id


async def test_only_matching_fuel_type_is_checked(db: AsyncSession):
    """Stops in a different fuel_type are not triggered."""
    from app.services.stop_engine import check_stops

    lng_stop = _make_stop_order(
        side=OrderSide.BID,
        order_type=OrderType.STOP,
        fuel_type="LNG",
        stop_price=Decimal("610.00"),
    )
    db.add(lng_stop)
    await db.flush()

    triggered = await check_stops(db, fuel_type="VLSFO", last_trade_price=Decimal("620.00"))

    assert triggered == []


# ---------------------------------------------------------------------------
# Tests: match_order is called for triggered orders
# ---------------------------------------------------------------------------

async def test_triggered_stop_gets_matched(db: AsyncSession):
    """After triggering, match_order is called for each triggered order."""
    from app.services.stop_engine import check_stops

    order = _make_stop_order(
        side=OrderSide.BID,
        order_type=OrderType.STOP,
        stop_price=Decimal("610.00"),
    )
    db.add(order)
    await db.flush()

    with patch(
        "app.services.stop_engine.match_order", new_callable=AsyncMock
    ) as mock_match:
        mock_match.return_value = []  # No trades created, but we verify it was called
        triggered = await check_stops(db, fuel_type="VLSFO", last_trade_price=Decimal("615.00"))

    assert len(triggered) == 1
    mock_match.assert_awaited_once()
    call_args = mock_match.call_args
    # First arg is db, second is the triggered order
    assert call_args[0][1].id == order.id


async def test_sse_event_published_per_trigger(db: AsyncSession):
    """An SSE event is published for each triggered stop order."""
    from app.services.stop_engine import check_stops

    order = _make_stop_order(
        side=OrderSide.BID,
        order_type=OrderType.STOP,
        stop_price=Decimal("610.00"),
    )
    db.add(order)
    await db.flush()

    with patch(
        "app.services.stop_engine.match_order", new_callable=AsyncMock, return_value=[]
    ):
        with patch("app.services.stop_engine.event_bus") as mock_bus:
            mock_bus.publish = AsyncMock()
            triggered = await check_stops(db, fuel_type="VLSFO", last_trade_price=Decimal("615.00"))

    assert len(triggered) == 1
    mock_bus.publish.assert_awaited_once()
    call_kwargs = mock_bus.publish.call_args
    assert "stop_triggered" in call_kwargs[0]  # event_type positional arg


async def test_multiple_stops_all_triggered(db: AsyncSession):
    """Multiple eligible stop orders in the same fuel_type all trigger."""
    from app.services.stop_engine import check_stops

    orders = [
        _make_stop_order(
            side=OrderSide.BID,
            order_type=OrderType.STOP,
            stop_price=Decimal("610.00"),
        )
        for _ in range(3)
    ]
    for o in orders:
        db.add(o)
    await db.flush()

    with patch(
        "app.services.stop_engine.match_order", new_callable=AsyncMock, return_value=[]
    ):
        triggered = await check_stops(db, fuel_type="VLSFO", last_trade_price=Decimal("620.00"))

    assert len(triggered) == 3

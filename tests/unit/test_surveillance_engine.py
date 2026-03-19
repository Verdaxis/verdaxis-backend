"""TDD tests for the SurveillanceEngine — STORIES S6-002, S6-003, S6-004.

All tests use an in-memory SQLite async session. The engine is tested
at the service layer (not via HTTP), so we instantiate SurveillanceEngine
directly and check the SurveillanceEvent objects it creates.
"""
import uuid
import pytest
from decimal import Decimal
from datetime import datetime, UTC, timedelta

from sqlalchemy.ext.asyncio import create_async_engine, AsyncSession, async_sessionmaker

from app.database import Base
from app.models.user import Organization, OrgType
from app.models.orderbook import (
    OrderBookOrder, Trade, OrderSide, OrderBookStatus, TradeStatus, Initiator,
)
from app.models.surveillance import (
    SurveillanceType,
    SurveillanceSeverity,
    SurveillanceStatus,
)

_REQUIRED_TABLES = [
    "organizations",
    "users",
    "orderbook_orders",
    "trades",
    "notifications",
    "surveillance_events",
]


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

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
    session_factory = async_sessionmaker(
        bind=async_engine,
        class_=AsyncSession,
        expire_on_commit=False,
        autoflush=False,
    )
    async with session_factory() as session:
        yield session
        await session.rollback()


def _make_org(db, org_type=OrgType.SHIPPING_LINE):
    org = Organization(id=uuid.uuid4(), name="TestOrg", type=org_type)
    db.add(org)
    return org


def _make_trade(buyer_id: uuid.UUID, seller_id: uuid.UUID, quantity: Decimal = Decimal("100")) -> Trade:
    return Trade(
        buyer_id=buyer_id,
        seller_id=seller_id,
        initiated_by=Initiator.BUYER,
        quantity_mt=quantity,
        price_per_mt_usd=Decimal("600.00"),
        status=TradeStatus.PENDING_CONFIRMATION,
        created_at=datetime.now(UTC),
    )


def _make_order(
    org_id: uuid.UUID,
    side=OrderSide.BID,
    fuel_type: str = "Methanol",
    price: Decimal = Decimal("600.00"),
    quantity: Decimal = Decimal("500"),
    status: OrderBookStatus = OrderBookStatus.OPEN,
    created_at: datetime | None = None,
    region: str = "Singapore",
) -> OrderBookOrder:
    return OrderBookOrder(
        organization_id=org_id,
        side=side,
        fuel_type=fuel_type,
        region=region,
        quantity_mt=quantity,
        remaining_quantity_mt=quantity,
        price_per_mt_usd=price,
        status=status,
        created_at=created_at or datetime.now(UTC),
        updated_at=created_at or datetime.now(UTC),
    )


# ---------------------------------------------------------------------------
# S6-002: Wash Trading
# ---------------------------------------------------------------------------

class TestWashTrading:
    @pytest.mark.asyncio
    async def test_wash_trading_detected(self, db):
        """buyer_id == seller_id → WASH_TRADING event at HIGH severity."""
        from app.services.surveillance import SurveillanceEngine

        org = _make_org(db)
        await db.flush()

        trade = _make_trade(buyer_id=org.id, seller_id=org.id)
        db.add(trade)
        await db.flush()

        engine = SurveillanceEngine(db)
        event = await engine.check_wash_trading(trade)

        assert event is not None
        assert event.type == SurveillanceType.WASH_TRADING
        assert event.severity == SurveillanceSeverity.HIGH
        assert event.status == SurveillanceStatus.OPEN
        assert event.auto_detected is True
        assert str(trade.id) in event.related_trades
        assert str(org.id) in event.participants

    @pytest.mark.asyncio
    async def test_wash_trading_not_triggered_different_orgs(self, db):
        """buyer_id != seller_id → no event."""
        from app.services.surveillance import SurveillanceEngine

        buyer_org = _make_org(db)
        seller_org = _make_org(db, OrgType.FUEL_SUPPLIER)
        await db.flush()

        trade = _make_trade(buyer_id=buyer_org.id, seller_id=seller_org.id)
        db.add(trade)
        await db.flush()

        engine = SurveillanceEngine(db)
        event = await engine.check_wash_trading(trade)

        assert event is None


# ---------------------------------------------------------------------------
# S6-003: Spoofing
# ---------------------------------------------------------------------------

class TestSpoofing:
    @pytest.mark.asyncio
    async def test_spoofing_detected(self, db):
        """3+ cancellations in 60s window → SPOOFING event."""
        from app.services.surveillance import SurveillanceEngine

        org = _make_org(db)
        await db.flush()

        now = datetime.now(UTC)
        # Insert 3 CANCELLED orders within the window
        for _ in range(3):
            o = _make_order(
                org.id,
                status=OrderBookStatus.CANCELLED,
                created_at=now - timedelta(seconds=30),
            )
            o.updated_at = now - timedelta(seconds=10)
            db.add(o)

        # The triggering cancelled order
        trigger = _make_order(org.id, status=OrderBookStatus.CANCELLED, created_at=now)
        trigger.updated_at = now
        db.add(trigger)
        await db.flush()

        engine = SurveillanceEngine(db)
        event = await engine.check_spoofing(trigger, window_seconds=60, threshold=3)

        assert event is not None
        assert event.type == SurveillanceType.SPOOFING
        assert event.severity == SurveillanceSeverity.MEDIUM
        assert event.auto_detected is True
        assert str(org.id) in event.participants

    @pytest.mark.asyncio
    async def test_spoofing_not_triggered_below_threshold(self, db):
        """2 cancellations in window → no event (threshold=3)."""
        from app.services.surveillance import SurveillanceEngine

        org = _make_org(db)
        await db.flush()

        now = datetime.now(UTC)
        for _ in range(2):
            o = _make_order(org.id, fuel_type="LNG", status=OrderBookStatus.CANCELLED)
            o.updated_at = now - timedelta(seconds=10)
            db.add(o)

        trigger = _make_order(org.id, fuel_type="LNG", status=OrderBookStatus.CANCELLED)
        trigger.updated_at = now
        db.add(trigger)
        await db.flush()

        engine = SurveillanceEngine(db)
        event = await engine.check_spoofing(trigger, window_seconds=60, threshold=3)

        assert event is None


# ---------------------------------------------------------------------------
# S6-004: Front-Running
# ---------------------------------------------------------------------------

class TestFrontRunning:
    @pytest.mark.asyncio
    async def test_front_running_detected(self, db):
        """Small order from org A filled just before large order, different user."""
        from app.services.surveillance import SurveillanceEngine

        buyer_org = _make_org(db)
        seller_org = _make_org(db, OrgType.FUEL_SUPPLIER)
        front_runner_org = _make_org(db, OrgType.FUEL_SUPPLIER)
        await db.flush()

        now = datetime.now(UTC)

        # The large trade that sets the price signal
        large_trade = _make_trade(buyer_org.id, seller_org.id, quantity=Decimal("5000"))
        large_trade.created_at = now
        db.add(large_trade)

        # Small front-running trade by different org, just before the large trade
        small_trade = _make_trade(buyer_org.id, front_runner_org.id, quantity=Decimal("50"))
        small_trade.created_at = now - timedelta(seconds=10)
        db.add(small_trade)
        await db.flush()

        engine = SurveillanceEngine(db)
        event = await engine.check_front_running(large_trade, window_seconds=60, size_ratio_threshold=10)

        assert event is not None
        assert event.type == SurveillanceType.FRONT_RUNNING
        assert event.severity == SurveillanceSeverity.HIGH
        assert event.auto_detected is True

    @pytest.mark.asyncio
    async def test_front_running_not_triggered_same_user(self, db):
        """Same org on both sides → self-trade, not front-running. No event."""
        from app.services.surveillance import SurveillanceEngine

        buyer_org = _make_org(db)
        seller_org = _make_org(db, OrgType.FUEL_SUPPLIER)
        await db.flush()

        now = datetime.now(UTC)

        large_trade = _make_trade(buyer_org.id, seller_org.id, quantity=Decimal("5000"))
        large_trade.created_at = now
        db.add(large_trade)

        # Same seller (seller_org) placed small trade too — not front-running by a third party
        small_trade = _make_trade(buyer_org.id, seller_org.id, quantity=Decimal("50"))
        small_trade.created_at = now - timedelta(seconds=10)
        db.add(small_trade)
        await db.flush()

        engine = SurveillanceEngine(db)
        event = await engine.check_front_running(large_trade, window_seconds=60, size_ratio_threshold=10)

        assert event is None


# ---------------------------------------------------------------------------
# S6-004: Layering
# ---------------------------------------------------------------------------

class TestLayering:
    @pytest.mark.asyncio
    async def test_layering_detected(self, db):
        """Multiple orders at different prices, >50% cancelled → LAYERING event."""
        from app.services.surveillance import SurveillanceEngine

        org = _make_org(db)
        await db.flush()

        now = datetime.now(UTC)
        session_start = now - timedelta(hours=2)

        # 6 orders at different price levels, 5 cancelled (>50%)
        prices = [Decimal("500"), Decimal("510"), Decimal("520"), Decimal("530"), Decimal("540"), Decimal("550")]
        for i, price in enumerate(prices):
            st = OrderBookStatus.CANCELLED if i < 5 else OrderBookStatus.OPEN
            o = _make_order(
                org.id,
                price=price,
                fuel_type="VLSFO",
                status=st,
                created_at=session_start + timedelta(minutes=i * 10),
            )
            o.updated_at = now - timedelta(minutes=10 - i)
            db.add(o)

        trigger = _make_order(
            org.id, price=Decimal("560"), fuel_type="VLSFO",
            status=OrderBookStatus.CANCELLED,
        )
        trigger.updated_at = now
        db.add(trigger)
        await db.flush()

        engine = SurveillanceEngine(db)
        event = await engine.check_layering(trigger, session_hours=4, cancel_rate_threshold=0.5, min_price_levels=3)

        assert event is not None
        assert event.type == SurveillanceType.LAYERING
        assert event.severity == SurveillanceSeverity.MEDIUM
        assert event.auto_detected is True

    @pytest.mark.asyncio
    async def test_layering_not_triggered_too_few_price_levels(self, db):
        """Only 2 distinct price levels → below min_price_levels=3, no event."""
        from app.services.surveillance import SurveillanceEngine

        org = _make_org(db)
        await db.flush()

        now = datetime.now(UTC)

        for _ in range(4):
            o = _make_order(org.id, fuel_type="HFO", price=Decimal("400"), status=OrderBookStatus.CANCELLED)
            o.updated_at = now - timedelta(seconds=30)
            db.add(o)

        trigger = _make_order(org.id, fuel_type="HFO", price=Decimal("410"), status=OrderBookStatus.CANCELLED)
        trigger.updated_at = now
        db.add(trigger)
        await db.flush()

        engine = SurveillanceEngine(db)
        event = await engine.check_layering(trigger, session_hours=4, cancel_rate_threshold=0.5, min_price_levels=3)

        assert event is None


# ---------------------------------------------------------------------------
# S6-004: Marking the Close
# ---------------------------------------------------------------------------

class TestMarkingTheClose:
    @pytest.mark.asyncio
    async def test_marking_the_close_detected(self, db):
        """Order placed in last 5 minutes of market close → MARKING_THE_CLOSE event."""
        from app.services.surveillance import SurveillanceEngine

        org = _make_org(db)
        await db.flush()

        # Market closes at 17:00; order placed at 16:57 (3 min before close)
        market_close_hour = 17
        close_time = datetime.now(UTC).replace(hour=market_close_hour, minute=0, second=0, microsecond=0)
        order_time = close_time - timedelta(minutes=3)

        order = _make_order(org.id, created_at=order_time)
        db.add(order)
        await db.flush()

        engine = SurveillanceEngine(db)
        event = await engine.check_marking_the_close(
            order, market_close_hour=market_close_hour, window_minutes=5
        )

        assert event is not None
        assert event.type == SurveillanceType.MARKING_THE_CLOSE
        assert event.severity == SurveillanceSeverity.LOW
        assert event.auto_detected is True

    @pytest.mark.asyncio
    async def test_marking_the_close_not_triggered_outside_window(self, db):
        """Order placed well before close → no event."""
        from app.services.surveillance import SurveillanceEngine

        org = _make_org(db)
        await db.flush()

        market_close_hour = 17
        close_time = datetime.now(UTC).replace(hour=market_close_hour, minute=0, second=0, microsecond=0)
        order_time = close_time - timedelta(hours=2)  # Well before close

        order = _make_order(org.id, created_at=order_time)
        db.add(order)
        await db.flush()

        engine = SurveillanceEngine(db)
        event = await engine.check_marking_the_close(
            order, market_close_hour=market_close_hour, window_minutes=5
        )

        assert event is None


# ---------------------------------------------------------------------------
# S6-004: Composite check methods
# ---------------------------------------------------------------------------

class TestCompositeChecks:
    @pytest.mark.asyncio
    async def test_run_post_trade_checks_returns_list(self, db):
        """run_post_trade_checks returns a list of events (may be empty)."""
        from app.services.surveillance import SurveillanceEngine

        buyer_org = _make_org(db)
        seller_org = _make_org(db, OrgType.FUEL_SUPPLIER)
        await db.flush()

        trade = _make_trade(buyer_org.id, seller_org.id)
        db.add(trade)
        await db.flush()

        engine = SurveillanceEngine(db)
        events = await engine.run_post_trade_checks(trade)

        assert isinstance(events, list)

    @pytest.mark.asyncio
    async def test_run_post_trade_checks_detects_wash_trade(self, db):
        """run_post_trade_checks catches wash trading."""
        from app.services.surveillance import SurveillanceEngine

        org = _make_org(db)
        await db.flush()

        trade = _make_trade(org.id, org.id)
        db.add(trade)
        await db.flush()

        engine = SurveillanceEngine(db)
        events = await engine.run_post_trade_checks(trade)

        types = [e.type for e in events]
        assert SurveillanceType.WASH_TRADING in types

    @pytest.mark.asyncio
    async def test_run_post_cancel_checks_returns_list(self, db):
        """run_post_cancel_checks returns a list of events (may be empty)."""
        from app.services.surveillance import SurveillanceEngine

        org = _make_org(db)
        await db.flush()

        order = _make_order(org.id, status=OrderBookStatus.CANCELLED)
        order.updated_at = datetime.now(UTC)
        db.add(order)
        await db.flush()

        engine = SurveillanceEngine(db)
        events = await engine.run_post_cancel_checks(order)

        assert isinstance(events, list)

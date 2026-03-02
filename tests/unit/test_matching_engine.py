"""Unit tests for the match-on-insert matching engine.

Uses an in-memory SQLite database to test the full matching logic
with real SQL queries (not mocks).
"""
import pytest
import uuid
from decimal import Decimal
from datetime import datetime, UTC, timedelta

from sqlalchemy import create_engine, event
from sqlalchemy.ext.asyncio import create_async_engine, AsyncSession, async_sessionmaker
from sqlalchemy.orm import Session

from app.database import Base
from app.models.user import User, Organization, UserRole, UserStatus, OrgType
from app.models.orderbook import (
    OrderBookOrder, Trade, OrderSide, OrderBookStatus, TradeStatus, Initiator,
)
from app.models.notification import Notification
from app.services.matching_engine import match_order

# Tables needed for our tests (avoids loading models with broken FK refs)
_REQUIRED_TABLES = [
    "organizations",
    "users",
    "orderbook_orders",
    "trades",
    "notifications",
]


# --------------- Fixtures ---------------

@pytest.fixture(scope="module")
def async_engine():
    """Create an async SQLite engine for tests."""
    engine = create_async_engine(
        "sqlite+aiosqlite://",
        echo=False,
        future=True,
    )
    return engine


@pytest.fixture(scope="module")
async def setup_tables(async_engine):
    """Create only the tables we need (avoids broken FK in traceability_events)."""
    tables = [Base.metadata.tables[t] for t in _REQUIRED_TABLES if t in Base.metadata.tables]
    async with async_engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all, tables=tables)
    yield
    async with async_engine.begin() as conn:
        await conn.run_sync(Base.metadata.drop_all, tables=tables)


@pytest.fixture
async def db(async_engine, setup_tables):
    """Provide a fresh async session that rolls back after each test."""
    session_factory = async_sessionmaker(
        bind=async_engine,
        class_=AsyncSession,
        expire_on_commit=False,
        autoflush=False,
    )
    async with session_factory() as session:
        yield session
        await session.rollback()


@pytest.fixture
def org_buyer_id():
    return uuid.uuid4()


@pytest.fixture
def org_seller_id():
    return uuid.uuid4()


@pytest.fixture
def org_seller2_id():
    return uuid.uuid4()


@pytest.fixture
async def buyer_org(db, org_buyer_id):
    """Create a buyer organization with one user."""
    org = Organization(
        id=org_buyer_id,
        name="BuyerCorp",
        type=OrgType.SHIPPING_LINE,
    )
    db.add(org)
    user = User(
        email="buyer@buyercorp.com",
        password_hash="hashed",
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        organization_id=org_buyer_id,
    )
    db.add(user)
    await db.flush()
    return org


@pytest.fixture
async def seller_org(db, org_seller_id):
    """Create a seller organization with one user."""
    org = Organization(
        id=org_seller_id,
        name="SellerCorp",
        type=OrgType.FUEL_SUPPLIER,
    )
    db.add(org)
    user = User(
        email="seller@sellercorp.com",
        password_hash="hashed",
        role=UserRole.SUPPLIER,
        status=UserStatus.APPROVED,
        organization_id=org_seller_id,
    )
    db.add(user)
    await db.flush()
    return org


@pytest.fixture
async def seller_org2(db, org_seller2_id):
    """Create a second seller organization for multi-fill tests."""
    org = Organization(
        id=org_seller2_id,
        name="SellerCorp2",
        type=OrgType.FUEL_SUPPLIER,
    )
    db.add(org)
    user = User(
        email="seller2@sellercorp2.com",
        password_hash="hashed",
        role=UserRole.SUPPLIER,
        status=UserStatus.APPROVED,
        organization_id=org_seller2_id,
    )
    db.add(user)
    await db.flush()
    return org


def _make_order(
    org_id,
    side: OrderSide,
    fuel_type: str = "Methanol",
    price: Decimal = Decimal("550.00"),
    quantity: Decimal = Decimal("1000.00"),
    region: str = "Singapore",
    created_at: datetime | None = None,
    status: OrderBookStatus = OrderBookStatus.OPEN,
) -> OrderBookOrder:
    """Helper to build an OrderBookOrder with sensible defaults."""
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
    )


# --------------- Tests ---------------


class TestBasicMatching:
    """Basic bid-ask crossing scenarios."""

    @pytest.mark.asyncio
    async def test_bid_matches_ask_when_bid_gte_ask(self, db, buyer_org, seller_org, org_buyer_id, org_seller_id):
        """A BID at $560 should match an ASK at $550 (bid >= ask)."""
        ask = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550.00"))
        db.add(ask)
        await db.flush()

        bid = _make_order(org_buyer_id, OrderSide.BID, price=Decimal("560.00"))
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)

        assert len(trades) == 1
        trade = trades[0]
        assert trade.quantity_mt == Decimal("1000.00")
        # Trade price = resting order's price (the ASK)
        assert trade.price_per_mt_usd == Decimal("550.00")
        assert trade.buyer_id == org_buyer_id
        assert trade.seller_id == org_seller_id
        assert trade.bid_order_id == bid.id
        assert trade.ask_order_id == ask.id

    @pytest.mark.asyncio
    async def test_ask_matches_bid_when_bid_gte_ask(self, db, buyer_org, seller_org, org_buyer_id, org_seller_id):
        """An ASK at $540 should match a BID at $550 (bid >= ask)."""
        bid = _make_order(org_buyer_id, OrderSide.BID, price=Decimal("550.00"))
        db.add(bid)
        await db.flush()

        ask = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("540.00"))
        db.add(ask)
        await db.flush()

        trades = await match_order(db, ask)

        assert len(trades) == 1
        trade = trades[0]
        assert trade.quantity_mt == Decimal("1000.00")
        # Trade price = resting order's price (the BID)
        assert trade.price_per_mt_usd == Decimal("550.00")
        assert trade.buyer_id == org_buyer_id
        assert trade.seller_id == org_seller_id
        assert trade.initiated_by == Initiator.SELLER

    @pytest.mark.asyncio
    async def test_exact_price_match(self, db, buyer_org, seller_org, org_buyer_id, org_seller_id):
        """A BID at $550 should match an ASK at $550 (exact price)."""
        ask = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550.00"))
        db.add(ask)
        await db.flush()

        bid = _make_order(org_buyer_id, OrderSide.BID, price=Decimal("550.00"))
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)
        assert len(trades) == 1
        assert trades[0].price_per_mt_usd == Decimal("550.00")


class TestNoMatch:
    """Scenarios where no match should occur."""

    @pytest.mark.asyncio
    async def test_no_match_fuel_type_differs(self, db, buyer_org, seller_org, org_buyer_id, org_seller_id):
        """BID for Methanol should NOT match ASK for LNG."""
        ask = _make_order(org_seller_id, OrderSide.ASK, fuel_type="LNG", price=Decimal("550.00"))
        db.add(ask)
        await db.flush()

        bid = _make_order(org_buyer_id, OrderSide.BID, fuel_type="Methanol", price=Decimal("560.00"))
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)
        assert len(trades) == 0

    @pytest.mark.asyncio
    async def test_no_match_bid_below_ask(self, db, buyer_org, seller_org, org_buyer_id, org_seller_id):
        """BID at $540 should NOT match ASK at $550."""
        ask = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550.00"))
        db.add(ask)
        await db.flush()

        bid = _make_order(org_buyer_id, OrderSide.BID, price=Decimal("540.00"))
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)
        assert len(trades) == 0

    @pytest.mark.asyncio
    async def test_no_self_trade(self, db, buyer_org, org_buyer_id):
        """Orders from the same organization should NOT match each other."""
        ask = _make_order(org_buyer_id, OrderSide.ASK, price=Decimal("550.00"))
        db.add(ask)
        await db.flush()

        bid = _make_order(org_buyer_id, OrderSide.BID, price=Decimal("560.00"))
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)
        assert len(trades) == 0

    @pytest.mark.asyncio
    async def test_no_match_against_filled_order(self, db, buyer_org, seller_org, org_buyer_id, org_seller_id):
        """A FILLED order should not be matched."""
        ask = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550.00"), status=OrderBookStatus.FILLED)
        db.add(ask)
        await db.flush()

        bid = _make_order(org_buyer_id, OrderSide.BID, price=Decimal("560.00"))
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)
        assert len(trades) == 0

    @pytest.mark.asyncio
    async def test_no_match_against_cancelled_order(self, db, buyer_org, seller_org, org_buyer_id, org_seller_id):
        """A CANCELLED order should not be matched."""
        ask = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550.00"), status=OrderBookStatus.CANCELLED)
        db.add(ask)
        await db.flush()

        bid = _make_order(org_buyer_id, OrderSide.BID, price=Decimal("560.00"))
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)
        assert len(trades) == 0


class TestPriceTimePriority:
    """Price-time priority ordering tests."""

    @pytest.mark.asyncio
    async def test_lower_ask_matches_first_for_bid(self, db, buyer_org, seller_org, seller_org2, org_buyer_id, org_seller_id, org_seller2_id):
        """When a BID crosses multiple ASKs, the lowest-priced ASK matches first."""
        now = datetime.now(UTC)

        # Higher-priced ask placed first (500 MT each so 2000 MT BID can match both)
        ask_high = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550.00"), quantity=Decimal("500.00"), created_at=now)
        db.add(ask_high)

        # Lower-priced ask placed second
        ask_low = _make_order(org_seller2_id, OrderSide.ASK, price=Decimal("540.00"), quantity=Decimal("500.00"), created_at=now + timedelta(seconds=1))
        db.add(ask_low)
        await db.flush()

        bid = _make_order(org_buyer_id, OrderSide.BID, price=Decimal("560.00"), quantity=Decimal("2000.00"))
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)

        assert len(trades) == 2
        # First match should be the lower-priced ask (price priority)
        assert trades[0].price_per_mt_usd == Decimal("540.00")
        assert trades[1].price_per_mt_usd == Decimal("550.00")

    @pytest.mark.asyncio
    async def test_time_priority_at_same_price(self, db, buyer_org, seller_org, seller_org2, org_buyer_id, org_seller_id, org_seller2_id):
        """When two ASKs have the same price, the older one matches first."""
        now = datetime.now(UTC)

        ask_old = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550.00"), quantity=Decimal("500.00"), created_at=now - timedelta(minutes=5))
        db.add(ask_old)

        ask_new = _make_order(org_seller2_id, OrderSide.ASK, price=Decimal("550.00"), quantity=Decimal("500.00"), created_at=now)
        db.add(ask_new)
        await db.flush()

        bid = _make_order(org_buyer_id, OrderSide.BID, price=Decimal("560.00"), quantity=Decimal("2000.00"))
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)

        assert len(trades) == 2
        # First trade should be against the older ask
        assert trades[0].ask_order_id == ask_old.id
        assert trades[1].ask_order_id == ask_new.id

    @pytest.mark.asyncio
    async def test_higher_bid_matches_first_for_ask(self, db, buyer_org, seller_org, seller_org2, org_buyer_id, org_seller_id, org_seller2_id):
        """When an ASK crosses multiple BIDs, the highest-priced BID matches first."""
        now = datetime.now(UTC)

        # Two buyer orgs needed — reuse seller_org2 as a second buyer for this test
        bid_low = _make_order(org_seller_id, OrderSide.BID, price=Decimal("550.00"), quantity=Decimal("500.00"), created_at=now)
        db.add(bid_low)

        bid_high = _make_order(org_seller2_id, OrderSide.BID, price=Decimal("560.00"), quantity=Decimal("500.00"), created_at=now + timedelta(seconds=1))
        db.add(bid_high)
        await db.flush()

        ask = _make_order(org_buyer_id, OrderSide.ASK, price=Decimal("540.00"), quantity=Decimal("2000.00"))
        db.add(ask)
        await db.flush()

        trades = await match_order(db, ask)

        assert len(trades) == 2
        # First match should be the higher-priced bid (best bid for seller)
        assert trades[0].price_per_mt_usd == Decimal("560.00")
        assert trades[1].price_per_mt_usd == Decimal("550.00")


class TestPartialFills:
    """Partial fill and multi-fill scenarios."""

    @pytest.mark.asyncio
    async def test_partial_fill_bid_larger(self, db, buyer_org, seller_org, org_buyer_id, org_seller_id):
        """1000 MT BID matches 500 MT ASK — BID has 500 remaining."""
        ask = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550.00"), quantity=Decimal("500.00"))
        db.add(ask)
        await db.flush()

        bid = _make_order(org_buyer_id, OrderSide.BID, price=Decimal("560.00"), quantity=Decimal("1000.00"))
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)

        assert len(trades) == 1
        assert trades[0].quantity_mt == Decimal("500.00")

        # BID should be partially filled
        assert bid.remaining_quantity_mt == Decimal("500.00")
        assert bid.status == OrderBookStatus.PARTIALLY_FILLED

        # ASK should be fully filled
        assert ask.remaining_quantity_mt == Decimal("0")
        assert ask.status == OrderBookStatus.FILLED

    @pytest.mark.asyncio
    async def test_partial_fill_ask_larger(self, db, buyer_org, seller_org, org_buyer_id, org_seller_id):
        """500 MT BID matches 1000 MT ASK — ASK has 500 remaining."""
        ask = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550.00"), quantity=Decimal("1000.00"))
        db.add(ask)
        await db.flush()

        bid = _make_order(org_buyer_id, OrderSide.BID, price=Decimal("560.00"), quantity=Decimal("500.00"))
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)

        assert len(trades) == 1
        assert trades[0].quantity_mt == Decimal("500.00")

        # BID should be fully filled
        assert bid.remaining_quantity_mt == Decimal("0")
        assert bid.status == OrderBookStatus.FILLED

        # ASK should be partially filled
        assert ask.remaining_quantity_mt == Decimal("500.00")
        assert ask.status == OrderBookStatus.PARTIALLY_FILLED

    @pytest.mark.asyncio
    async def test_multiple_fills(self, db, buyer_org, seller_org, seller_org2, org_buyer_id, org_seller_id, org_seller2_id):
        """1000 MT BID matches 300 MT + 400 MT ASKs — BID has 300 remaining."""
        now = datetime.now(UTC)

        ask1 = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("545.00"), quantity=Decimal("300.00"), created_at=now)
        db.add(ask1)

        ask2 = _make_order(org_seller2_id, OrderSide.ASK, price=Decimal("548.00"), quantity=Decimal("400.00"), created_at=now + timedelta(seconds=1))
        db.add(ask2)
        await db.flush()

        bid = _make_order(org_buyer_id, OrderSide.BID, price=Decimal("550.00"), quantity=Decimal("1000.00"))
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)

        assert len(trades) == 2
        assert trades[0].quantity_mt == Decimal("300.00")
        assert trades[0].price_per_mt_usd == Decimal("545.00")  # First ask's price (lower)
        assert trades[1].quantity_mt == Decimal("400.00")
        assert trades[1].price_per_mt_usd == Decimal("548.00")  # Second ask's price

        assert bid.remaining_quantity_mt == Decimal("300.00")
        assert bid.status == OrderBookStatus.PARTIALLY_FILLED

        assert ask1.remaining_quantity_mt == Decimal("0")
        assert ask1.status == OrderBookStatus.FILLED
        assert ask2.remaining_quantity_mt == Decimal("0")
        assert ask2.status == OrderBookStatus.FILLED

    @pytest.mark.asyncio
    async def test_exact_full_fill(self, db, buyer_org, seller_org, org_buyer_id, org_seller_id):
        """Both orders are exactly 1000 MT — both should be FILLED."""
        ask = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550.00"), quantity=Decimal("1000.00"))
        db.add(ask)
        await db.flush()

        bid = _make_order(org_buyer_id, OrderSide.BID, price=Decimal("550.00"), quantity=Decimal("1000.00"))
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)

        assert len(trades) == 1
        assert bid.remaining_quantity_mt == Decimal("0")
        assert bid.status == OrderBookStatus.FILLED
        assert ask.remaining_quantity_mt == Decimal("0")
        assert ask.status == OrderBookStatus.FILLED


class TestTradeDetails:
    """Verify trade object fields are set correctly."""

    @pytest.mark.asyncio
    async def test_auto_confirmed_status(self, db, buyer_org, seller_org, org_buyer_id, org_seller_id):
        """Auto-matched trades should have CONFIRMED status."""
        ask = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550.00"))
        db.add(ask)
        await db.flush()

        bid = _make_order(org_buyer_id, OrderSide.BID, price=Decimal("560.00"))
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)

        assert len(trades) == 1
        assert trades[0].status == TradeStatus.CONFIRMED
        assert trades[0].confirmed_at is not None

    @pytest.mark.asyncio
    async def test_initiator_buyer_when_bid_aggressor(self, db, buyer_org, seller_org, org_buyer_id, org_seller_id):
        """When a BID is the new order (aggressor), initiated_by should be BUYER."""
        ask = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550.00"))
        db.add(ask)
        await db.flush()

        bid = _make_order(org_buyer_id, OrderSide.BID, price=Decimal("560.00"))
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)
        assert trades[0].initiated_by == Initiator.BUYER

    @pytest.mark.asyncio
    async def test_initiator_seller_when_ask_aggressor(self, db, buyer_org, seller_org, org_buyer_id, org_seller_id):
        """When an ASK is the new order (aggressor), initiated_by should be SELLER."""
        bid = _make_order(org_buyer_id, OrderSide.BID, price=Decimal("560.00"))
        db.add(bid)
        await db.flush()

        ask = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550.00"))
        db.add(ask)
        await db.flush()

        trades = await match_order(db, ask)
        assert trades[0].initiated_by == Initiator.SELLER

    @pytest.mark.asyncio
    async def test_trade_price_is_resting_order_price(self, db, buyer_org, seller_org, org_buyer_id, org_seller_id):
        """Trade price should be the resting (passive) order's price, giving price improvement to aggressor."""
        ask = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("540.00"))
        db.add(ask)
        await db.flush()

        # BID at $560 is the aggressor — gets price improvement at $540
        bid = _make_order(org_buyer_id, OrderSide.BID, price=Decimal("560.00"))
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)
        assert trades[0].price_per_mt_usd == Decimal("540.00")  # Resting ASK price

    @pytest.mark.asyncio
    async def test_zero_remaining_returns_empty(self, db, buyer_org, seller_org, org_buyer_id, org_seller_id):
        """An order with 0 remaining quantity should not attempt to match."""
        ask = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550.00"))
        db.add(ask)
        await db.flush()

        bid = _make_order(org_buyer_id, OrderSide.BID, price=Decimal("560.00"), quantity=Decimal("0"))
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)
        assert len(trades) == 0


class TestNotifications:
    """Verify notifications are created for matched trades."""

    @pytest.mark.asyncio
    async def test_notifications_created_for_both_parties(self, db, buyer_org, seller_org, org_buyer_id, org_seller_id):
        """Both buyer and seller orgs should receive notifications on auto-match."""
        ask = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550.00"))
        db.add(ask)
        await db.flush()

        bid = _make_order(org_buyer_id, OrderSide.BID, price=Decimal("560.00"))
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)
        assert len(trades) == 1

        # Check that notifications were added to the session
        # (they won't be committed yet since we haven't committed)
        new_objects = [o for o in db.new if isinstance(o, Notification)]
        assert len(new_objects) >= 2  # At least one per party

    @pytest.mark.asyncio
    async def test_matches_partially_filled_resting_order(self, db, buyer_org, seller_org, org_buyer_id, org_seller_id):
        """A PARTIALLY_FILLED resting order should still be matchable."""
        ask = _make_order(
            org_seller_id, OrderSide.ASK,
            price=Decimal("550.00"),
            quantity=Decimal("1000.00"),
            status=OrderBookStatus.PARTIALLY_FILLED,
        )
        # Simulate already partially filled
        ask.remaining_quantity_mt = Decimal("600.00")
        db.add(ask)
        await db.flush()

        bid = _make_order(org_buyer_id, OrderSide.BID, price=Decimal("560.00"), quantity=Decimal("400.00"))
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)

        assert len(trades) == 1
        assert trades[0].quantity_mt == Decimal("400.00")
        assert bid.remaining_quantity_mt == Decimal("0")
        assert bid.status == OrderBookStatus.FILLED
        assert ask.remaining_quantity_mt == Decimal("200.00")
        assert ask.status == OrderBookStatus.PARTIALLY_FILLED

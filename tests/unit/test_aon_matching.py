"""Unit tests for AON (All-or-Nothing) matching logic.

AON orders must fill completely or not at all. They never partially fill.
"""
import pytest
import uuid
from decimal import Decimal
from datetime import datetime, UTC, timedelta

from sqlalchemy.ext.asyncio import create_async_engine, AsyncSession, async_sessionmaker

from app.database import Base
from app.models.user import User, Organization, UserRole, UserStatus, OrgType
from app.models.orderbook import (
    OrderBookOrder, OrderSide, OrderBookStatus, OrderType,
)
from app.services.matching_engine import match_order

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
def org_seller3_id():
    return uuid.uuid4()


@pytest.fixture
async def buyer_org(db, org_buyer_id):
    org = Organization(id=org_buyer_id, name="BuyerCorp", type=OrgType.SHIPPING_LINE)
    db.add(org)
    db.add(User(
        email="buyer@buyercorp.com",
        password_hash="hashed",
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        organization_id=org_buyer_id,
    ))
    await db.flush()
    return org


@pytest.fixture
async def seller_org(db, org_seller_id):
    org = Organization(id=org_seller_id, name="SellerCorp", type=OrgType.FUEL_SUPPLIER)
    db.add(org)
    db.add(User(
        email="seller@sellercorp.com",
        password_hash="hashed",
        role=UserRole.SUPPLIER,
        status=UserStatus.APPROVED,
        organization_id=org_seller_id,
    ))
    await db.flush()
    return org


@pytest.fixture
async def seller_org2(db, org_seller2_id):
    org = Organization(id=org_seller2_id, name="SellerCorp2", type=OrgType.FUEL_SUPPLIER)
    db.add(org)
    db.add(User(
        email="seller2@sellercorp2.com",
        password_hash="hashed",
        role=UserRole.SUPPLIER,
        status=UserStatus.APPROVED,
        organization_id=org_seller2_id,
    ))
    await db.flush()
    return org


@pytest.fixture
async def seller_org3(db, org_seller3_id):
    org = Organization(id=org_seller3_id, name="SellerCorp3", type=OrgType.FUEL_SUPPLIER)
    db.add(org)
    db.add(User(
        email="seller3@sellercorp3.com",
        password_hash="hashed",
        role=UserRole.SUPPLIER,
        status=UserStatus.APPROVED,
        organization_id=org_seller3_id,
    ))
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
    order_type: OrderType = OrderType.LIMIT,
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
        order_type=order_type,
        created_at=created_at or datetime.now(UTC),
    )


# --------------- Tests ---------------


class TestAONFill:
    """AON order fills completely when sufficient counter-volume exists."""

    @pytest.mark.asyncio
    async def test_aon_fills_when_sufficient_counter_volume(
        self, db, buyer_org, seller_org, org_buyer_id, org_seller_id
    ):
        """AON BID of 1000 MT fills against a single ASK of 1000 MT."""
        ask = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550.00"), quantity=Decimal("1000.00"))
        db.add(ask)
        await db.flush()

        aon_bid = _make_order(
            org_buyer_id, OrderSide.BID,
            price=Decimal("560.00"),
            quantity=Decimal("1000.00"),
            order_type=OrderType.AON,
        )
        db.add(aon_bid)
        await db.flush()

        trades = await match_order(db, aon_bid)

        assert len(trades) == 1
        assert trades[0].quantity_mt == Decimal("1000.00")
        assert aon_bid.status == OrderBookStatus.FILLED
        assert aon_bid.remaining_quantity_mt == Decimal("0")

    @pytest.mark.asyncio
    async def test_aon_fills_when_counter_volume_exceeds_order(
        self, db, buyer_org, seller_org, org_buyer_id, org_seller_id
    ):
        """AON BID of 800 MT fills against an ASK of 2000 MT (surplus volume is fine)."""
        ask = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550.00"), quantity=Decimal("2000.00"))
        db.add(ask)
        await db.flush()

        aon_bid = _make_order(
            org_buyer_id, OrderSide.BID,
            price=Decimal("560.00"),
            quantity=Decimal("800.00"),
            order_type=OrderType.AON,
        )
        db.add(aon_bid)
        await db.flush()

        trades = await match_order(db, aon_bid)

        assert len(trades) == 1
        assert trades[0].quantity_mt == Decimal("800.00")
        assert aon_bid.status == OrderBookStatus.FILLED
        assert ask.remaining_quantity_mt == Decimal("1200.00")


class TestAONNoFill:
    """AON order stays OPEN when insufficient counter-volume exists."""

    @pytest.mark.asyncio
    async def test_aon_stays_open_when_insufficient_volume(
        self, db, buyer_org, seller_org, org_buyer_id, org_seller_id
    ):
        """AON BID of 1000 MT should NOT match when only 500 MT ASK is available."""
        ask = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550.00"), quantity=Decimal("500.00"))
        db.add(ask)
        await db.flush()

        aon_bid = _make_order(
            org_buyer_id, OrderSide.BID,
            price=Decimal("560.00"),
            quantity=Decimal("1000.00"),
            order_type=OrderType.AON,
        )
        db.add(aon_bid)
        await db.flush()

        trades = await match_order(db, aon_bid)

        assert len(trades) == 0
        # AON stays OPEN, nothing consumed
        assert aon_bid.status == OrderBookStatus.OPEN
        assert aon_bid.remaining_quantity_mt == Decimal("1000.00")
        # Resting ASK is untouched
        assert ask.status == OrderBookStatus.OPEN
        assert ask.remaining_quantity_mt == Decimal("500.00")

    @pytest.mark.asyncio
    async def test_aon_stays_open_when_no_counter_orders(
        self, db, buyer_org, org_buyer_id
    ):
        """AON BID with zero matching ASKs stays OPEN."""
        aon_bid = _make_order(
            org_buyer_id, OrderSide.BID,
            price=Decimal("560.00"),
            quantity=Decimal("1000.00"),
            order_type=OrderType.AON,
        )
        db.add(aon_bid)
        await db.flush()

        trades = await match_order(db, aon_bid)

        assert len(trades) == 0
        assert aon_bid.status == OrderBookStatus.OPEN


class TestAONMultipleOrders:
    """AON volume check aggregates across multiple resting orders."""

    @pytest.mark.asyncio
    async def test_aon_checks_total_across_multiple_orders(
        self, db, buyer_org, seller_org, seller_org2, seller_org3,
        org_buyer_id, org_seller_id, org_seller2_id, org_seller3_id,
    ):
        """3 small ASKs (300 + 300 + 400 = 1000 MT) can fill 1 large AON BID of 1000 MT."""
        now = datetime.now(UTC)
        ask1 = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("548.00"), quantity=Decimal("300.00"), created_at=now)
        ask2 = _make_order(org_seller2_id, OrderSide.ASK, price=Decimal("549.00"), quantity=Decimal("300.00"), created_at=now + timedelta(seconds=1))
        ask3 = _make_order(org_seller3_id, OrderSide.ASK, price=Decimal("550.00"), quantity=Decimal("400.00"), created_at=now + timedelta(seconds=2))
        db.add(ask1)
        db.add(ask2)
        db.add(ask3)
        await db.flush()

        aon_bid = _make_order(
            org_buyer_id, OrderSide.BID,
            price=Decimal("560.00"),
            quantity=Decimal("1000.00"),
            order_type=OrderType.AON,
        )
        db.add(aon_bid)
        await db.flush()

        trades = await match_order(db, aon_bid)

        assert len(trades) == 3
        assert aon_bid.status == OrderBookStatus.FILLED
        assert aon_bid.remaining_quantity_mt == Decimal("0")
        # All three asks consumed
        assert ask1.status == OrderBookStatus.FILLED
        assert ask2.status == OrderBookStatus.FILLED
        assert ask3.status == OrderBookStatus.FILLED

    @pytest.mark.asyncio
    async def test_aon_stays_open_when_sum_just_short(
        self, db, buyer_org, seller_org, seller_org2,
        org_buyer_id, org_seller_id, org_seller2_id,
    ):
        """Two ASKs summing to 999 MT should NOT fill a 1000 MT AON BID."""
        ask1 = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550.00"), quantity=Decimal("500.00"))
        ask2 = _make_order(org_seller2_id, OrderSide.ASK, price=Decimal("550.00"), quantity=Decimal("499.00"))
        db.add(ask1)
        db.add(ask2)
        await db.flush()

        aon_bid = _make_order(
            org_buyer_id, OrderSide.BID,
            price=Decimal("560.00"),
            quantity=Decimal("1000.00"),
            order_type=OrderType.AON,
        )
        db.add(aon_bid)
        await db.flush()

        trades = await match_order(db, aon_bid)

        assert len(trades) == 0
        assert aon_bid.status == OrderBookStatus.OPEN
        assert ask1.remaining_quantity_mt == Decimal("500.00")
        assert ask2.remaining_quantity_mt == Decimal("499.00")


class TestNonAONRegression:
    """Ensure normal (non-AON) orders are completely unaffected."""

    @pytest.mark.asyncio
    async def test_non_aon_order_still_partial_fills(
        self, db, buyer_org, seller_org, org_buyer_id, org_seller_id
    ):
        """A LIMIT BID with more quantity than the ASK should still partially fill."""
        ask = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550.00"), quantity=Decimal("400.00"))
        db.add(ask)
        await db.flush()

        bid = _make_order(
            org_buyer_id, OrderSide.BID,
            price=Decimal("560.00"),
            quantity=Decimal("1000.00"),
            order_type=OrderType.LIMIT,
        )
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)

        assert len(trades) == 1
        assert trades[0].quantity_mt == Decimal("400.00")
        assert bid.status == OrderBookStatus.PARTIALLY_FILLED
        assert bid.remaining_quantity_mt == Decimal("600.00")
        assert ask.status == OrderBookStatus.FILLED

    @pytest.mark.asyncio
    async def test_default_order_type_still_partial_fills(
        self, db, buyer_org, seller_org, org_buyer_id, org_seller_id
    ):
        """An order with no explicit order_type (defaults to LIMIT) should still partial fill."""
        ask = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550.00"), quantity=Decimal("300.00"))
        db.add(ask)
        await db.flush()

        # _make_order defaults to LIMIT which is what OrderBookOrder defaults to
        bid = _make_order(org_buyer_id, OrderSide.BID, price=Decimal("560.00"), quantity=Decimal("1000.00"))
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)

        assert len(trades) == 1
        assert trades[0].quantity_mt == Decimal("300.00")
        assert bid.status == OrderBookStatus.PARTIALLY_FILLED


class TestAONSelfTradePrevention:
    """AON volume check must skip same-org orders (self-trade prevention)."""

    @pytest.mark.asyncio
    async def test_aon_respects_self_trade_prevention(
        self, db, buyer_org, seller_org, org_buyer_id, org_seller_id
    ):
        """Same-org ASKs must not count toward the AON volume requirement.

        Setup: buyer org has an ASK of 800 MT + seller org has an ASK of 300 MT.
        AON BID needs 1000 MT. Only the 300 MT from the other org is eligible.
        Volume check should see 300 MT < 1000 MT → no fill.
        """
        # Same org as AON BID — should be excluded from volume check
        self_ask = _make_order(org_buyer_id, OrderSide.ASK, price=Decimal("550.00"), quantity=Decimal("800.00"))
        db.add(self_ask)

        # Legitimate counter-party ask — only 300 MT
        other_ask = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550.00"), quantity=Decimal("300.00"))
        db.add(other_ask)
        await db.flush()

        aon_bid = _make_order(
            org_buyer_id, OrderSide.BID,
            price=Decimal("560.00"),
            quantity=Decimal("1000.00"),
            order_type=OrderType.AON,
        )
        db.add(aon_bid)
        await db.flush()

        trades = await match_order(db, aon_bid)

        # 300 MT eligible (self-ask excluded) < 1000 MT needed → no fill
        assert len(trades) == 0
        assert aon_bid.status == OrderBookStatus.OPEN
        assert aon_bid.remaining_quantity_mt == Decimal("1000.00")
        # Neither ASK was consumed
        assert self_ask.remaining_quantity_mt == Decimal("800.00")
        assert other_ask.remaining_quantity_mt == Decimal("300.00")

    @pytest.mark.asyncio
    async def test_aon_fills_when_sufficient_volume_excluding_self(
        self, db, buyer_org, seller_org, seller_org2,
        org_buyer_id, org_seller_id, org_seller2_id,
    ):
        """AON fills when non-self volume is sufficient, even if self-org ASK also exists."""
        # Same org as AON BID — excluded from volume check
        self_ask = _make_order(org_buyer_id, OrderSide.ASK, price=Decimal("550.00"), quantity=Decimal("500.00"))
        db.add(self_ask)

        # Two legitimate counter-party asks totalling 1200 MT
        ask1 = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("548.00"), quantity=Decimal("600.00"))
        ask2 = _make_order(org_seller2_id, OrderSide.ASK, price=Decimal("550.00"), quantity=Decimal("600.00"))
        db.add(ask1)
        db.add(ask2)
        await db.flush()

        aon_bid = _make_order(
            org_buyer_id, OrderSide.BID,
            price=Decimal("560.00"),
            quantity=Decimal("1000.00"),
            order_type=OrderType.AON,
        )
        db.add(aon_bid)
        await db.flush()

        trades = await match_order(db, aon_bid)

        # 1200 MT eligible (1000 MT needed) → should fill
        assert len(trades) == 2
        assert aon_bid.status == OrderBookStatus.FILLED
        # Self-ask is untouched (self-trade prevention)
        assert self_ask.remaining_quantity_mt == Decimal("500.00")

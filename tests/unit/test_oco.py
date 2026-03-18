"""Unit tests for OCO (One-Cancels-Other) paired order logic.

Covers:
- Schema-level validation
- Cancel-on-fill propagation via matching engine
- Manual cancel propagation
- Service-layer OCO creation helpers
"""
import pytest
import uuid
from decimal import Decimal
from datetime import datetime, UTC

from sqlalchemy.ext.asyncio import create_async_engine, AsyncSession, async_sessionmaker

from app.database import Base
from app.models.user import User, Organization, UserRole, UserStatus, OrgType
from app.models.orderbook import (
    OrderBookOrder, OrderSide, OrderBookStatus, OrderType,
)
from app.schemas.orderbook import OrderCreate, OCOCreateRequest, OrderSide as SchemaSide, OrderType as SchemaOrderType
from app.services.matching_engine import match_order
from app.services.oco import create_oco_pair, cancel_linked_order

_REQUIRED_TABLES = [
    "organizations",
    "users",
    "orderbook_orders",
    "trades",
    "notifications",
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


# --------------- Org/User fixtures ---------------

@pytest.fixture
def org_seller_id():
    return uuid.uuid4()


@pytest.fixture
def org_buyer_id():
    return uuid.uuid4()


@pytest.fixture
async def seller_org(db, org_seller_id):
    org = Organization(id=org_seller_id, name="SellerCo", type=OrgType.FUEL_SUPPLIER)
    db.add(org)
    user = User(
        email="seller@sellerco.com",
        password_hash="hashed",
        role=UserRole.SUPPLIER,
        status=UserStatus.APPROVED,
        organization_id=org_seller_id,
    )
    db.add(user)
    await db.flush()
    return org


@pytest.fixture
async def buyer_org(db, org_buyer_id):
    org = Organization(id=org_buyer_id, name="BuyerCo", type=OrgType.SHIPPING_LINE)
    db.add(org)
    user = User(
        email="buyer@buyerco.com",
        password_hash="hashed",
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        organization_id=org_buyer_id,
    )
    db.add(user)
    await db.flush()
    return org


# --------------- Helpers ---------------

def _make_order(
    org_id,
    side: OrderSide,
    price: Decimal = Decimal("550.00"),
    quantity: Decimal = Decimal("1000.00"),
    order_type: OrderType = OrderType.OCO,
    fuel_type: str = "Methanol",
    region: str = "Singapore",
    status: OrderBookStatus = OrderBookStatus.OPEN,
) -> OrderBookOrder:
    return OrderBookOrder(
        organization_id=org_id,
        side=side,
        fuel_type=fuel_type,
        region=region,
        quantity_mt=quantity,
        remaining_quantity_mt=quantity,
        price_per_mt_usd=price,
        order_type=order_type,
        status=status,
        created_at=datetime.now(UTC),
    )


# ================================================================
# Schema-level tests (no DB needed)
# ================================================================

class TestOCOSchema:
    """Validate the OCOCreateRequest Pydantic schema."""

    def test_oco_schema_accepts_valid_pair(self):
        req = OCOCreateRequest(
            order_a=OrderCreate(
                side=SchemaSide.ASK,
                fuel_type="Methanol",
                region="Singapore",
                quantity_mt=Decimal("1000"),
                price_per_mt_usd=Decimal("620"),
                order_type=SchemaOrderType.LIMIT,
            ),
            order_b=OrderCreate(
                side=SchemaSide.ASK,
                fuel_type="Methanol",
                region="Singapore",
                quantity_mt=Decimal("1000"),
                price_per_mt_usd=Decimal("560"),
                order_type=SchemaOrderType.STOP,
                stop_price=Decimal("560"),
            ),
        )
        assert req.order_a.side == SchemaSide.ASK
        assert req.order_b.side == SchemaSide.ASK

    def test_oco_schema_rejects_mismatched_sides(self):
        """order_a and order_b must be the same side."""
        import pydantic
        with pytest.raises((ValueError, pydantic.ValidationError)):
            OCOCreateRequest(
                order_a=OrderCreate(
                    side=SchemaSide.ASK,
                    fuel_type="Methanol",
                    region="Singapore",
                    quantity_mt=Decimal("1000"),
                    price_per_mt_usd=Decimal("620"),
                ),
                order_b=OrderCreate(
                    side=SchemaSide.BID,   # wrong side
                    fuel_type="Methanol",
                    region="Singapore",
                    quantity_mt=Decimal("1000"),
                    price_per_mt_usd=Decimal("560"),
                ),
            )

    def test_oco_schema_rejects_mismatched_fuel_type(self):
        """Both orders must be for the same fuel type."""
        import pydantic
        with pytest.raises((ValueError, pydantic.ValidationError)):
            OCOCreateRequest(
                order_a=OrderCreate(
                    side=SchemaSide.ASK,
                    fuel_type="Methanol",
                    region="Singapore",
                    quantity_mt=Decimal("1000"),
                    price_per_mt_usd=Decimal("620"),
                ),
                order_b=OrderCreate(
                    side=SchemaSide.ASK,
                    fuel_type="LNG",  # different fuel
                    region="Singapore",
                    quantity_mt=Decimal("1000"),
                    price_per_mt_usd=Decimal("560"),
                ),
            )


# ================================================================
# DB-backed service tests
# ================================================================

class TestOCOLinking:
    """Verify that create_oco_pair links both orders symmetrically."""

    @pytest.mark.asyncio
    async def test_oco_create_links_both_orders(self, db, seller_org, org_seller_id):
        """linked_order_id must be symmetric: A.linked == B.id, B.linked == A.id."""
        order_a, order_b = await create_oco_pair(
            db=db,
            org_id=org_seller_id,
            order_a_data=dict(
                side=OrderSide.ASK,
                fuel_type="Methanol",
                region="Singapore",
                quantity_mt=Decimal("1000"),
                remaining_quantity_mt=Decimal("1000"),
                price_per_mt_usd=Decimal("620"),
                order_type=OrderType.OCO,
            ),
            order_b_data=dict(
                side=OrderSide.ASK,
                fuel_type="Methanol",
                region="Singapore",
                quantity_mt=Decimal("1000"),
                remaining_quantity_mt=Decimal("1000"),
                price_per_mt_usd=Decimal("560"),
                order_type=OrderType.OCO,
            ),
        )

        assert order_a.linked_order_id == order_b.id
        assert order_b.linked_order_id == order_a.id
        assert order_a.order_type == OrderType.OCO
        assert order_b.order_type == OrderType.OCO

    @pytest.mark.asyncio
    async def test_oco_both_start_as_open(self, db, seller_org, org_seller_id):
        """Both orders in the OCO pair start with OPEN status."""
        order_a, order_b = await create_oco_pair(
            db=db,
            org_id=org_seller_id,
            order_a_data=dict(
                side=OrderSide.ASK,
                fuel_type="Methanol",
                region="Singapore",
                quantity_mt=Decimal("500"),
                remaining_quantity_mt=Decimal("500"),
                price_per_mt_usd=Decimal("610"),
                order_type=OrderType.OCO,
            ),
            order_b_data=dict(
                side=OrderSide.ASK,
                fuel_type="Methanol",
                region="Singapore",
                quantity_mt=Decimal("500"),
                remaining_quantity_mt=Decimal("500"),
                price_per_mt_usd=Decimal("555"),
                order_type=OrderType.OCO,
            ),
        )

        assert order_a.status == OrderBookStatus.OPEN
        assert order_b.status == OrderBookStatus.OPEN


class TestOCOCancelOnFill:
    """When one OCO order is filled via match, the linked order is cancelled."""

    @pytest.mark.asyncio
    async def test_oco_cancel_propagation_on_fill(self, db, seller_org, buyer_org, org_seller_id, org_buyer_id):
        """
        Seller has two OCO ASK orders. Buyer BID fills one.
        The other ASK must be CANCELLED automatically.
        """
        # Place the two linked OCO ASK orders
        ask_tp, ask_sl = await create_oco_pair(
            db=db,
            org_id=org_seller_id,
            order_a_data=dict(
                side=OrderSide.ASK,
                fuel_type="Methanol",
                region="Singapore",
                quantity_mt=Decimal("1000"),
                remaining_quantity_mt=Decimal("1000"),
                price_per_mt_usd=Decimal("620"),  # take-profit leg
                order_type=OrderType.OCO,
            ),
            order_b_data=dict(
                side=OrderSide.ASK,
                fuel_type="Methanol",
                region="Singapore",
                quantity_mt=Decimal("1000"),
                remaining_quantity_mt=Decimal("1000"),
                price_per_mt_usd=Decimal("540"),  # stop-loss leg (lower price, matchable)
                order_type=OrderType.OCO,
            ),
        )

        # Buyer places a BID at $560 — crosses ask_sl ($540) not ask_tp ($620)
        bid = _make_order(
            org_buyer_id,
            OrderSide.BID,
            price=Decimal("560"),
            order_type=OrderType.LIMIT,
        )
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)

        assert len(trades) == 1
        assert ask_sl.status == OrderBookStatus.FILLED

        # The linked take-profit leg must be CANCELLED
        assert ask_tp.status == OrderBookStatus.CANCELLED

    @pytest.mark.asyncio
    async def test_oco_fill_does_not_cancel_non_oco_orders(self, db, seller_org, buyer_org, org_seller_id, org_buyer_id):
        """
        A LIMIT order (no linked_order_id) should not trigger any cancel propagation.
        """
        # Plain LIMIT ask — no OCO linkage
        ask = _make_order(org_seller_id, OrderSide.ASK, price=Decimal("550"), order_type=OrderType.LIMIT)
        ask.linked_order_id = None
        db.add(ask)
        await db.flush()

        bid = _make_order(org_buyer_id, OrderSide.BID, price=Decimal("560"), order_type=OrderType.LIMIT)
        db.add(bid)
        await db.flush()

        trades = await match_order(db, bid)
        assert len(trades) == 1
        assert ask.status == OrderBookStatus.FILLED


class TestOCOManualCancel:
    """When one OCO order is manually cancelled, the linked order is also cancelled."""

    @pytest.mark.asyncio
    async def test_oco_manual_cancel_propagates(self, db, seller_org, org_seller_id):
        """
        Cancelling order_a should also cancel order_b (the linked pair).
        """
        order_a, order_b = await create_oco_pair(
            db=db,
            org_id=org_seller_id,
            order_a_data=dict(
                side=OrderSide.ASK,
                fuel_type="Methanol",
                region="Singapore",
                quantity_mt=Decimal("1000"),
                remaining_quantity_mt=Decimal("1000"),
                price_per_mt_usd=Decimal("620"),
                order_type=OrderType.OCO,
            ),
            order_b_data=dict(
                side=OrderSide.ASK,
                fuel_type="Methanol",
                region="Singapore",
                quantity_mt=Decimal("1000"),
                remaining_quantity_mt=Decimal("1000"),
                price_per_mt_usd=Decimal("560"),
                order_type=OrderType.OCO,
            ),
        )

        # Cancel order_a — should propagate to order_b
        await cancel_linked_order(db, order_a)

        assert order_a.status == OrderBookStatus.CANCELLED
        assert order_b.status == OrderBookStatus.CANCELLED

    @pytest.mark.asyncio
    async def test_oco_cancel_skips_already_terminal_linked(self, db, seller_org, org_seller_id):
        """
        If the linked order is already FILLED, cancel propagation should be a no-op
        (no error, and filled status is not overwritten).
        """
        order_a, order_b = await create_oco_pair(
            db=db,
            org_id=org_seller_id,
            order_a_data=dict(
                side=OrderSide.ASK,
                fuel_type="Methanol",
                region="Singapore",
                quantity_mt=Decimal("1000"),
                remaining_quantity_mt=Decimal("1000"),
                price_per_mt_usd=Decimal("620"),
                order_type=OrderType.OCO,
            ),
            order_b_data=dict(
                side=OrderSide.ASK,
                fuel_type="Methanol",
                region="Singapore",
                quantity_mt=Decimal("1000"),
                remaining_quantity_mt=Decimal("1000"),
                price_per_mt_usd=Decimal("560"),
                order_type=OrderType.OCO,
            ),
        )

        # Pre-fill order_b to simulate it already being terminal
        order_b.status = OrderBookStatus.FILLED
        order_b.remaining_quantity_mt = Decimal("0")
        await db.flush()

        # Cancel order_a — should not crash, should not overwrite order_b's FILLED status
        await cancel_linked_order(db, order_a)

        assert order_a.status == OrderBookStatus.CANCELLED
        assert order_b.status == OrderBookStatus.FILLED  # untouched

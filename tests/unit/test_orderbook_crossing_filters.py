"""Tests that crossed-market detection respects active orderbook filters."""
import pytest
from datetime import datetime, UTC
from decimal import Decimal
from uuid import uuid4

from sqlalchemy import delete
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.market_catalog import DELIVERY_POINTS_BY_NAME, PRODUCTS_BY_NAME
from app.models.user import Organization, OrganizationProvenance, OrgType
from app.models.catalog import Product, DeliveryPoint
from app.models.orderbook import OrderBookOrder, OrderSide
from app.routers.orderbook import list_asks, list_bids


REQUIRED_TABLES = [
    "organizations",
    "products",
    "delivery_points",
    "orderbook_orders",
    "live_slice_benchmarks",
]


@pytest.fixture(scope="module")
def async_engine():
    return create_async_engine("sqlite+aiosqlite://", echo=False, future=True)


@pytest.fixture(scope="module")
async def setup_tables(async_engine):
    tables = [Base.metadata.tables[name] for name in REQUIRED_TABLES]
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
        for table in ("live_slice_benchmarks", "orderbook_orders", "delivery_points", "products", "organizations"):
            await session.execute(delete(Base.metadata.tables[table]))
        await session.commit()


async def _make_org(db, name: str) -> Organization:
    org = Organization(
        name=name,
        type=OrgType.FUEL_SUPPLIER,
        provenance=OrganizationProvenance.REAL,
    )
    db.add(org)
    await db.flush()
    return org


async def _make_product(db, name: str, fuel_type: str, fuel_grade: str) -> Product:
    spec = PRODUCTS_BY_NAME.get(name)
    product = Product(
        id=(spec.id if spec and (spec.fuel_type, spec.fuel_grade) == (fuel_type, fuel_grade) else uuid4()),
        name=name,
        fuel_type=fuel_type,
        fuel_grade=fuel_grade,
        is_active=True,
    )
    db.add(product)
    await db.flush()
    return product


async def _make_delivery_point(db, name: str, region: str) -> DeliveryPoint:
    spec = DELIVERY_POINTS_BY_NAME.get(name)
    dp = DeliveryPoint(
        id=(spec.id if spec and spec.region == region else uuid4()),
        name=name,
        region=region,
        is_active=True,
    )
    db.add(dp)
    await db.flush()
    return dp


def _make_order(
    org_id,
    side: OrderSide,
    product_id,
    delivery_point_id,
    price: str,
    quantity: str = "1000",
    window: str = "SPOT",
    certification_scheme: str | None = "ISCC EU",
    certification_declared: bool = True,
):
    payload = dict(
        organization_id=org_id,
        provenance=OrganizationProvenance.REAL,
        side=side,
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        quantity_mt=Decimal(quantity),
        remaining_quantity_mt=Decimal(quantity),
        price_per_mt_usd=Decimal(price),
        availability_window=window,
        created_at=datetime.now(UTC),
        certification_scheme=certification_scheme,
        certification_declared=certification_declared,
    )
    if side == OrderSide.ASK:
        payload.update(
            specification_standard='ISO 8217',
            msds_available=True,
            carbon_intensity_gco2_mj=Decimal('18.50'),
            feedstock='Waste biomass',
            origin='Singapore',
        )
    return OrderBookOrder(**payload)


class TestCrossingDetectionFilters:
    @pytest.mark.asyncio
    async def test_bid_crossing_uses_same_product_delivery_point_and_window_filters(self, db):
        buyer_org = await _make_org(db, "Buyer Org")
        seller_org = await _make_org(db, "Seller Org")
        other_seller_org = await _make_org(db, "Other Seller Org")

        methanol = await _make_product(db, "Bio Methanol", "Methanol", "Bio")
        lng = await _make_product(db, "LNG Conventional", "LNG", "Conventional")

        singapore = await _make_delivery_point(db, "Singapore", "Asia")
        busan = await _make_delivery_point(db, "Busan", "Asia")
        rotterdam = await _make_delivery_point(db, "Rotterdam", "Europe")

        bid = _make_order(
            buyer_org.id,
            OrderSide.BID,
            methanol.id,
            singapore.id,
            "550",
            window="SPOT",
        )
        valid_ask = _make_order(
            seller_org.id,
            OrderSide.ASK,
            methanol.id,
            singapore.id,
            "560",
            window="SPOT",
        )
        wrong_product_ask = _make_order(
            other_seller_org.id,
            OrderSide.ASK,
            lng.id,
            singapore.id,
            "500",
            window="SPOT",
        )
        wrong_delivery_point_ask = _make_order(
            other_seller_org.id,
            OrderSide.ASK,
            methanol.id,
            busan.id,
            "480",
            window="SPOT",
        )
        wrong_window_ask = _make_order(
            other_seller_org.id,
            OrderSide.ASK,
            methanol.id,
            rotterdam.id,
            "470",
            window="2026-Q1",
        )

        db.add_all([bid, valid_ask, wrong_product_ask, wrong_delivery_point_ask, wrong_window_ask])
        await db.commit()

        result = await list_bids(
            product_id=methanol.id,
            delivery_point_id=singapore.id,
            fuel_type=None,
            region=None,
            availability_window="Spot",
            skip=0,
            limit=20,
            db=db,
        )

        assert result.total == 1
        assert len(result.items) == 1
        assert result.items[0].id == bid.id
        assert result.items[0].is_crossed is False

        await db.execute(delete(OrderBookOrder))
        await db.commit()

    @pytest.mark.asyncio
    async def test_ask_crossing_uses_fuel_type_region_and_window_filters(self, db):
        buyer_org = await _make_org(db, "Buyer Org")
        seller_org = await _make_org(db, "Seller Org")
        other_buyer_org = await _make_org(db, "Other Buyer Org")

        methanol = await _make_product(db, "Bio Methanol", "Methanol", "Bio")
        lng = await _make_product(db, "LNG Conventional", "LNG", "Conventional")

        singapore = await _make_delivery_point(db, "Singapore", "Asia")
        rotterdam = await _make_delivery_point(db, "Rotterdam", "Europe")

        ask = _make_order(
            seller_org.id,
            OrderSide.ASK,
            methanol.id,
            singapore.id,
            "550",
            window="SPOT",
        )
        valid_bid = _make_order(
            buyer_org.id,
            OrderSide.BID,
            methanol.id,
            singapore.id,
            "500",
            window="SPOT",
        )
        wrong_fuel_bid = _make_order(
            other_buyer_org.id,
            OrderSide.BID,
            lng.id,
            singapore.id,
            "700",
            window="SPOT",
        )
        wrong_region_bid = _make_order(
            other_buyer_org.id,
            OrderSide.BID,
            methanol.id,
            rotterdam.id,
            "650",
            window="SPOT",
        )
        wrong_window_bid = _make_order(
            other_buyer_org.id,
            OrderSide.BID,
            methanol.id,
            singapore.id,
            "640",
            window="2026-Q1",
        )

        db.add_all([ask, valid_bid, wrong_fuel_bid, wrong_region_bid, wrong_window_bid])
        await db.commit()

        result = await list_asks(
            product_id=None,
            delivery_point_id=None,
            fuel_type="Methanol",
            region="Asia",
            availability_window="Spot",
            skip=0,
            limit=20,
            db=db,
        )

        assert len(result.items) == 1
        assert result.items[0].id == ask.id
        assert result.items[0].is_crossed is False

        await db.execute(delete(OrderBookOrder))
        await db.commit()

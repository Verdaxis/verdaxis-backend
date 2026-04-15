import pytest
from datetime import UTC, datetime
from decimal import Decimal
from uuid import uuid4

from sqlalchemy import delete
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.models.catalog import DeliveryPoint, Product
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import OrgType, Organization
from app.routers.orderbook import list_asks, list_bids

REQUIRED_TABLES = [
    'organizations',
    'products',
    'delivery_points',
    'orderbook_orders',
]


@pytest.fixture(scope='module')
def async_engine():
    return create_async_engine('sqlite+aiosqlite://', echo=False, future=True)


@pytest.fixture(scope='module')
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
        for table in ('orderbook_orders', 'products', 'delivery_points', 'organizations'):
            await session.execute(delete(Base.metadata.tables[table]))
        await session.commit()
        yield session
        await session.rollback()
        for table in ('orderbook_orders', 'products', 'delivery_points', 'organizations'):
            await session.execute(delete(Base.metadata.tables[table]))
        await session.commit()


async def _make_org(db: AsyncSession, name: str) -> Organization:
    org = Organization(name=name, type=OrgType.FUEL_SUPPLIER)
    db.add(org)
    await db.flush()
    return org


async def _make_product(db: AsyncSession, *, name: str, fuel_type: str, fuel_grade: str) -> Product:
    product = Product(name=f'{name} {uuid4().hex[:8]}', fuel_type=fuel_type, fuel_grade=fuel_grade)
    db.add(product)
    await db.flush()
    return product


async def _make_delivery_point(db: AsyncSession, name: str, region: str) -> DeliveryPoint:
    delivery_point = DeliveryPoint(name=f'{name} {uuid4().hex[:8]}', region=region)
    db.add(delivery_point)
    await db.flush()
    return delivery_point


def _make_order(*, org_id, side: OrderSide, product_id, delivery_point_id, price: str, quantity: str = '1000') -> OrderBookOrder:
    payload = dict(
        organization_id=org_id,
        side=side,
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        quantity_mt=Decimal(quantity),
        remaining_quantity_mt=Decimal(quantity),
        price_per_mt_usd=Decimal(price),
        availability_window='SPOT',
        status=OrderBookStatus.OPEN,
        created_at=datetime.now(UTC),
        certification_scheme='ISCC EU',
        certification_declared=True,
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


class TestLiveSliceBenchmarks:
    @pytest.mark.asyncio
    async def test_ask_benchmark_uses_same_slice_average(self, db: AsyncSession):
        supplier = await _make_org(db, 'Supplier')
        singapore = await _make_delivery_point(db, 'Singapore', 'Asia')
        methanol = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')

        db.add_all([
            _make_order(org_id=supplier.id, side=OrderSide.ASK, product_id=methanol.id, delivery_point_id=singapore.id, price='1027'),
            _make_order(org_id=supplier.id, side=OrderSide.ASK, product_id=methanol.id, delivery_point_id=singapore.id, price='1045'),
        ])
        await db.commit()

        result = await list_asks(
            product_id=None,
            delivery_point_id=singapore.id,
            fuel_type=None,
            region=None,
            availability_window='SPOT',
            include_off_spec=False,
            skip=0,
            limit=20,
            db=db,
        )

        assert result.total == 2
        by_price = {item.price_per_mt_usd: item for item in result.items}
        assert by_price[Decimal('1027.00')].benchmark_price_per_mt_usd == Decimal('1036.00')
        assert by_price[Decimal('1045.00')].benchmark_price_per_mt_usd == Decimal('1036.00')
        assert by_price[Decimal('1027.00')].premium_discount_per_mt_usd == Decimal('-9.00')
        assert by_price[Decimal('1045.00')].premium_discount_per_mt_usd == Decimal('9.00')
        assert all(item.benchmark_source == 'live_slice_ask_avg' for item in result.items)

    @pytest.mark.asyncio
    async def test_bid_benchmark_uses_same_slice_average(self, db: AsyncSession):
        buyer = await _make_org(db, 'Buyer')
        singapore = await _make_delivery_point(db, 'Singapore', 'Asia')
        methanol = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')

        db.add_all([
            _make_order(org_id=buyer.id, side=OrderSide.BID, product_id=methanol.id, delivery_point_id=singapore.id, price='1000'),
            _make_order(org_id=buyer.id, side=OrderSide.BID, product_id=methanol.id, delivery_point_id=singapore.id, price='1040'),
        ])
        await db.commit()

        result = await list_bids(
            product_id=None,
            delivery_point_id=singapore.id,
            fuel_type=None,
            region=None,
            availability_window='SPOT',
            include_off_spec=False,
            skip=0,
            limit=20,
            db=db,
        )

        assert result.total == 2
        by_price = {item.price_per_mt_usd: item for item in result.items}
        assert by_price[Decimal('1000.00')].benchmark_price_per_mt_usd == Decimal('1020.00')
        assert by_price[Decimal('1040.00')].benchmark_price_per_mt_usd == Decimal('1020.00')
        assert by_price[Decimal('1000.00')].premium_discount_per_mt_usd == Decimal('-20.00')
        assert by_price[Decimal('1040.00')].premium_discount_per_mt_usd == Decimal('20.00')
        assert all(item.benchmark_source == 'live_slice_bid_avg' for item in result.items)

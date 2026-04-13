"""Tests for public marketplace filtering on approved green-fuels products."""
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
from app.routers.orderbook import list_asks, list_fuel_types


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
        yield session
        await session.rollback()


async def _make_org(db: AsyncSession, name: str) -> Organization:
    org = Organization(name=name, type=OrgType.FUEL_SUPPLIER)
    db.add(org)
    await db.flush()
    return org


async def _make_product(
    db: AsyncSession,
    *,
    name: str,
    fuel_type: str,
    fuel_grade: str,
) -> Product:
    product = Product(name=f'{name} {uuid4().hex[:8]}', fuel_type=fuel_type, fuel_grade=fuel_grade)
    db.add(product)
    await db.flush()
    return product


async def _make_delivery_point(db: AsyncSession, name: str, region: str) -> DeliveryPoint:
    delivery_point = DeliveryPoint(name=f'{name} {uuid4().hex[:8]}', region=region)
    db.add(delivery_point)
    await db.flush()
    return delivery_point


def _make_order(
    *,
    org_id,
    product_id,
    delivery_point_id,
    price: str,
    quantity: str = '1000',
) -> OrderBookOrder:
    return OrderBookOrder(
        organization_id=org_id,
        side=OrderSide.ASK,
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        quantity_mt=Decimal(quantity),
        remaining_quantity_mt=Decimal(quantity),
        price_per_mt_usd=Decimal(price),
        availability_window='SPOT',
        status=OrderBookStatus.OPEN,
        created_at=datetime.now(UTC),
    )


class TestMarketplaceFuelFiltering:
    @pytest.mark.asyncio
    async def test_list_asks_excludes_non_methanol_ethanol_products(self, db: AsyncSession):
        supplier = await _make_org(db, 'Supplier')
        singapore = await _make_delivery_point(db, 'Singapore', 'Asia')
        methanol = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')
        biofuel = await _make_product(db, name='Biofuel Bio', fuel_type='Biofuel', fuel_grade='Bio')

        db.add_all(
            [
                _make_order(
                    org_id=supplier.id,
                    product_id=methanol.id,
                    delivery_point_id=singapore.id,
                    price='1100',
                ),
                _make_order(
                    org_id=supplier.id,
                    product_id=biofuel.id,
                    delivery_point_id=singapore.id,
                    price='900',
                ),
            ]
        )
        await db.commit()

        result = await list_asks(
            product_id=None,
            delivery_point_id=None,
            fuel_type=None,
            region='Asia',
            availability_window=None,
            skip=0,
            limit=20,
            db=db,
        )

        assert result.total == 1
        assert [item.fuel_type for item in result.items] == ['Methanol']

        await db.execute(delete(OrderBookOrder))
        await db.execute(delete(Product))
        await db.execute(delete(DeliveryPoint))
        await db.execute(delete(Organization))
        await db.commit()

    @pytest.mark.asyncio
    async def test_fuel_types_endpoint_returns_only_market_fuel_families(self, db: AsyncSession):
        supplier = await _make_org(db, 'Supplier')
        singapore = await _make_delivery_point(db, 'Singapore', 'Asia')
        methanol = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')
        ethanol = await _make_product(db, name='Bio Ethanol', fuel_type='Ethanol', fuel_grade='Bio')
        ammonia = await _make_product(db, name='Ammonia Green', fuel_type='Ammonia', fuel_grade='Green')

        db.add_all(
            [
                _make_order(
                    org_id=supplier.id,
                    product_id=methanol.id,
                    delivery_point_id=singapore.id,
                    price='1100',
                ),
                _make_order(
                    org_id=supplier.id,
                    product_id=ethanol.id,
                    delivery_point_id=singapore.id,
                    price='700',
                ),
                _make_order(
                    org_id=supplier.id,
                    product_id=ammonia.id,
                    delivery_point_id=singapore.id,
                    price='800',
                ),
            ]
        )
        await db.commit()

        fuel_types = await list_fuel_types(db=db)

        assert fuel_types == ['Ethanol', 'Methanol']

    @pytest.mark.asyncio
    async def test_list_asks_supports_combined_public_filters_without_duplicate_product_join(self, db: AsyncSession):
        supplier = await _make_org(db, 'Supplier')
        singapore = await _make_delivery_point(db, 'Singapore', 'Asia')
        methanol = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')
        ethanol = await _make_product(db, name='Bio Ethanol', fuel_type='Ethanol', fuel_grade='Bio')

        db.add_all(
            [
                _make_order(
                    org_id=supplier.id,
                    product_id=methanol.id,
                    delivery_point_id=singapore.id,
                    price='1100',
                ),
                _make_order(
                    org_id=supplier.id,
                    product_id=ethanol.id,
                    delivery_point_id=singapore.id,
                    price='700',
                ),
            ]
        )
        await db.commit()

        result = await list_asks(
            product_id=None,
            delivery_point_id=None,
            fuel_type='Methanol',
            region=singapore.name,
            availability_window='SPOT',
            skip=0,
            limit=20,
            db=db,
        )

        assert result.total == 1
        assert len(result.items) == 1
        assert result.items[0].fuel_type == 'Methanol'


    @pytest.mark.asyncio
    async def test_list_asks_filters_by_market_product(self, db: AsyncSession):
        supplier = await _make_org(db, 'Supplier')
        singapore = await _make_delivery_point(db, 'Singapore', 'Asia')
        bio_methanol = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')
        e_methanol = await _make_product(db, name='e-Methanol', fuel_type='Methanol', fuel_grade='E')

        db.add_all(
            [
                _make_order(
                    org_id=supplier.id,
                    product_id=bio_methanol.id,
                    delivery_point_id=singapore.id,
                    price='1100',
                ),
                _make_order(
                    org_id=supplier.id,
                    product_id=e_methanol.id,
                    delivery_point_id=singapore.id,
                    price='1115',
                ),
            ]
        )
        await db.commit()

        result = await list_asks(
            product_id=None,
            delivery_point_id=None,
            fuel_type=None,
            region=singapore.name,
            availability_window='SPOT',
            market_product='BIO_METHANOL',
            skip=0,
            limit=20,
            db=db,
        )

        assert result.total == 1
        assert len(result.items) == 1
        assert result.items[0].market_product == 'BIO_METHANOL'

"""Tests for public marketplace filtering on approved green-fuels products."""
import pytest
from datetime import UTC, datetime, timedelta
from decimal import Decimal
from uuid import uuid4

from sqlalchemy import delete
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.market_catalog import DELIVERY_POINTS_BY_NAME, PRODUCTS_BY_NAME
from app.models.catalog import DeliveryPoint, Product
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import OrganizationProvenance, OrgType, Organization
from app.schemas.market_activity import MarketDemoStatus, MarketSourceKind
from app.routers.orderbook import (
    get_map_summary,
    list_active_products,
    list_aggregated_orderbook,
    list_asks,
    list_fuel_types,
    list_orders,
    list_orders_with_ci,
    list_product_counts,
    list_regions,
)


REQUIRED_TABLES = [
    'organizations',
    'users',
    'products',
    'delivery_points',
    'orderbook_orders',
    'live_slice_benchmarks',
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
        for table in ('live_slice_benchmarks', 'orderbook_orders', 'products', 'delivery_points', 'organizations'):
            await session.execute(delete(Base.metadata.tables[table]))
        await session.commit()
        yield session
        await session.rollback()
        for table in ('live_slice_benchmarks', 'orderbook_orders', 'products', 'delivery_points', 'organizations'):
            await session.execute(delete(Base.metadata.tables[table]))
        await session.commit()


async def _make_org(db: AsyncSession, name: str) -> Organization:
    org = Organization(
        name=name,
        type=OrgType.FUEL_SUPPLIER,
        provenance=OrganizationProvenance.REAL,
    )
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


async def _make_delivery_point(db: AsyncSession, name: str, region: str) -> DeliveryPoint:
    spec = DELIVERY_POINTS_BY_NAME.get(name)
    delivery_point = DeliveryPoint(
        id=(spec.id if spec and spec.region == region else uuid4()),
        name=name,
        region=region,
        is_active=True,
    )
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
    availability_window: str = 'SPOT',
    certification_scheme: str | None = 'ISCC EU',
    certification_declared: bool = True,
    provenance: OrganizationProvenance = OrganizationProvenance.REAL,
) -> OrderBookOrder:
    return OrderBookOrder(
        organization_id=org_id,
        provenance=provenance,
        side=OrderSide.ASK,
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        quantity_mt=Decimal(quantity),
        remaining_quantity_mt=Decimal(quantity),
        price_per_mt_usd=Decimal(price),
        availability_window=availability_window,
        status=OrderBookStatus.OPEN,
        created_at=datetime.now(UTC),
        certification_scheme=certification_scheme,
        certification_declared=certification_declared,
        specification_standard='ISO 8217',
        msds_available=True,
        carbon_intensity_gco2_mj=Decimal('18.50'),
        feedstock='Waste biomass',
        origin='Singapore',
        expires_at=(
            datetime.now(UTC) + timedelta(days=1)
            if provenance == OrganizationProvenance.DEMO
            else None
        ),
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
            include_off_spec=False,
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
    async def test_public_catalog_facets_exclude_inactive_test_and_expired_demo_rows(self, db: AsyncSession):
        supplier = await _make_org(db, 'Catalog Supplier')
        singapore = await _make_delivery_point(db, 'Singapore', 'Asia')
        retired = await _make_delivery_point(db, 'Fujairah', 'Middle East')
        retired.is_active = False
        canonical = await _make_product(
            db,
            name='Bio Methanol',
            fuel_type='Methanol',
            fuel_grade='Bio',
        )
        inactive = await _make_product(
            db,
            name='Methanol Green',
            fuel_type='Methanol',
            fuel_grade='Green',
        )
        inactive.is_active = False
        visible = _make_order(
            org_id=supplier.id,
            product_id=canonical.id,
            delivery_point_id=singapore.id,
            price='1100',
        )
        retired_port = _make_order(
            org_id=supplier.id,
            product_id=canonical.id,
            delivery_point_id=retired.id,
            price='1090',
        )
        inactive_product = _make_order(
            org_id=supplier.id,
            product_id=inactive.id,
            delivery_point_id=singapore.id,
            price='1080',
        )
        test_order = _make_order(
            org_id=supplier.id,
            product_id=canonical.id,
            delivery_point_id=singapore.id,
            price='1070',
            provenance=OrganizationProvenance.TEST,
        )
        expired_demo = _make_order(
            org_id=supplier.id,
            product_id=canonical.id,
            delivery_point_id=singapore.id,
            price='1060',
            provenance=OrganizationProvenance.DEMO,
        )
        expired_demo.expires_at = datetime.now(UTC) - timedelta(seconds=1)
        db.add_all([visible, retired_port, inactive_product, test_order, expired_demo])
        await db.commit()

        assert await list_active_products(db=db) == ['Bio Methanol']
        assert await list_regions(db=db) == ['Asia']
        assert await list_fuel_types(db=db) == ['Methanol']

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
            include_off_spec=False,
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

    @pytest.mark.asyncio
    async def test_list_asks_excludes_off_spec_by_default(self, db: AsyncSession):
        supplier = await _make_org(db, 'Supplier')
        singapore = await _make_delivery_point(db, 'Singapore', 'Asia')
        methanol = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')

        open_order = _make_order(
            org_id=supplier.id,
            product_id=methanol.id,
            delivery_point_id=singapore.id,
            price='1100',
        )
        off_spec_order = _make_order(
            org_id=supplier.id,
            product_id=methanol.id,
            delivery_point_id=singapore.id,
            price='1095',
        )
        off_spec_order.off_spec = True
        db.add_all([open_order, off_spec_order])
        await db.commit()

        result = await list_asks(
            product_id=None,
            delivery_point_id=None,
            fuel_type=None,
            market_product='BIO_METHANOL',
            region='Asia',
            availability_window=None,
            include_off_spec=False,
            skip=0,
            limit=20,
            db=db,
        )

        assert result.total == 1
        assert all(item.off_spec is False for item in result.items)

        await db.execute(delete(OrderBookOrder))
        await db.execute(delete(Product))
        await db.execute(delete(DeliveryPoint))
        await db.execute(delete(Organization))
        await db.commit()


    @pytest.mark.asyncio
    async def test_public_list_orders_excludes_off_spec_by_default(self, db: AsyncSession):
        supplier = await _make_org(db, 'Supplier')
        singapore = await _make_delivery_point(db, 'Singapore', 'Asia')
        methanol = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')

        open_order = _make_order(
            org_id=supplier.id,
            product_id=methanol.id,
            delivery_point_id=singapore.id,
            price='1100',
        )
        off_spec_order = _make_order(
            org_id=supplier.id,
            product_id=methanol.id,
            delivery_point_id=singapore.id,
            price='1095',
        )
        off_spec_order.off_spec = True
        db.add_all([open_order, off_spec_order])
        await db.commit()

        result = await list_orders(
            product_id=None,
            delivery_point_id=None,
            side=None,
            availability_window=None,
            include_off_spec=False,
            skip=0,
            limit=50,
            db=db,
        )

        assert len(result) == 1
        assert result[0].off_spec is False

    @pytest.mark.asyncio
    async def test_with_ci_excludes_off_spec_by_default(self, db: AsyncSession):
        supplier = await _make_org(db, 'Supplier')
        singapore = await _make_delivery_point(db, 'Singapore', 'Asia')
        methanol = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')

        open_order = _make_order(
            org_id=supplier.id,
            product_id=methanol.id,
            delivery_point_id=singapore.id,
            price='1100',
        )
        off_spec_order = _make_order(
            org_id=supplier.id,
            product_id=methanol.id,
            delivery_point_id=singapore.id,
            price='1095',
        )
        off_spec_order.off_spec = True
        db.add_all([open_order, off_spec_order])
        await db.commit()

        result = await list_orders_with_ci(
            product_id=None,
            delivery_point_id=None,
            side=None,
            include_off_spec=False,
            skip=0,
            limit=50,
            db=db,
        )

        assert len(result) == 1
        assert result[0].off_spec is False

    @pytest.mark.asyncio
    async def test_aggregated_excludes_off_spec_by_default(self, db: AsyncSession):
        supplier = await _make_org(db, 'Supplier')
        singapore = await _make_delivery_point(db, 'Singapore', 'Asia')
        methanol = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')

        open_order = _make_order(
            org_id=supplier.id,
            product_id=methanol.id,
            delivery_point_id=singapore.id,
            price='1100',
            quantity='1000',
        )
        off_spec_order = _make_order(
            org_id=supplier.id,
            product_id=methanol.id,
            delivery_point_id=singapore.id,
            price='1095',
            quantity='400',
        )
        off_spec_order.off_spec = True
        db.add_all([open_order, off_spec_order])
        await db.commit()

        result = await list_aggregated_orderbook(
            product_id=None,
            delivery_point_id=None,
            fuel_type=None,
            market_product=None,
            region=None,
            availability_window=None,
            include_off_spec=False,
            limit=256,
            db=db,
        )

        assert len(result) == 1
        assert result[0].total_quantity == Decimal('1000')

    @pytest.mark.asyncio
    async def test_aggregated_filters_by_market_slice(self, db: AsyncSession):
        supplier = await _make_org(db, 'Supplier')
        singapore = await _make_delivery_point(db, 'Singapore', 'Asia')
        santos = await _make_delivery_point(db, 'Santos', 'Americas')
        bio_methanol = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')
        e_methanol = await _make_product(db, name='e-Methanol', fuel_type='Methanol', fuel_grade='E')

        matching_order = _make_order(
            org_id=supplier.id,
            product_id=bio_methanol.id,
            delivery_point_id=singapore.id,
            price='1100',
            quantity='1000',
            availability_window='SPOT',
        )
        wrong_product = _make_order(
            org_id=supplier.id,
            product_id=e_methanol.id,
            delivery_point_id=singapore.id,
            price='1250',
            quantity='500',
            availability_window='SPOT',
        )
        wrong_port = _make_order(
            org_id=supplier.id,
            product_id=bio_methanol.id,
            delivery_point_id=santos.id,
            price='1110',
            quantity='300',
            availability_window='SPOT',
        )
        wrong_window = _make_order(
            org_id=supplier.id,
            product_id=bio_methanol.id,
            delivery_point_id=singapore.id,
            price='1120',
            quantity='200',
            availability_window='2026-06',
        )
        db.add_all([matching_order, wrong_product, wrong_port, wrong_window])
        await db.commit()

        result = await list_aggregated_orderbook(
            product_id=None,
            fuel_type=None,
            market_product='BIO_METHANOL',
            delivery_point_id=singapore.id,
            region=None,
            availability_window='SPOT',
            include_off_spec=False,
            limit=256,
            db=db,
        )

        assert len(result) == 1
        assert result[0].product_id == bio_methanol.id
        assert result[0].delivery_point_id == singapore.id
        assert result[0].availability_window == 'SPOT'
        assert result[0].total_quantity == Decimal('1000')

    @pytest.mark.asyncio
    async def test_legacy_collection_is_hard_bounded_before_enrichment(self, db: AsyncSession):
        supplier = await _make_org(db, 'Bounded Supplier')
        singapore = await _make_delivery_point(db, 'Singapore', 'Asia')
        product = await _make_product(
            db,
            name='Bio Methanol',
            fuel_type='Methanol',
            fuel_grade='Bio',
        )
        db.add_all(
            [
                _make_order(
                    org_id=supplier.id,
                    product_id=product.id,
                    delivery_point_id=singapore.id,
                    price=str(1000 + index),
                )
                for index in range(75)
            ]
        )
        await db.commit()

        first_page = await list_orders(
            product_id=None,
            delivery_point_id=None,
            side=None,
            availability_window=None,
            include_off_spec=False,
            skip=0,
            limit=20,
            db=db,
        )
        second_page = await list_orders(
            product_id=None,
            delivery_point_id=None,
            side=None,
            availability_window=None,
            include_off_spec=False,
            skip=20,
            limit=20,
            db=db,
        )

        assert len(first_page) == 20
        assert len(second_page) == 20
        assert {item.id for item in first_page}.isdisjoint(
            {item.id for item in second_page}
        )

    @pytest.mark.asyncio
    async def test_aggregate_uses_active_catalog_and_never_blends_real_demo(self, db: AsyncSession):
        real_org = await _make_org(db, 'Real Aggregate Supplier')
        demo_org = await _make_org(db, 'Demo Aggregate Supplier')
        demo_org.provenance = OrganizationProvenance.DEMO
        singapore = await _make_delivery_point(db, 'Singapore', 'Asia')
        inactive_port = await _make_delivery_point(db, 'Retired Port', 'Asia')
        inactive_port.is_active = False
        product = await _make_product(
            db,
            name='Bio Methanol',
            fuel_type='Methanol',
            fuel_grade='Bio',
        )
        inactive_product = Product(
            name='Methanol Green',
            fuel_type='Methanol',
            fuel_grade='Green',
            is_active=False,
        )
        db.add(inactive_product)
        await db.flush()
        db.add_all(
            [
                _make_order(
                    org_id=real_org.id,
                    product_id=product.id,
                    delivery_point_id=singapore.id,
                    price='1000',
                    quantity='100',
                ),
                _make_order(
                    org_id=demo_org.id,
                    product_id=product.id,
                    delivery_point_id=singapore.id,
                    price='900',
                    quantity='200',
                    provenance=OrganizationProvenance.DEMO,
                ),
                _make_order(
                    org_id=real_org.id,
                    product_id=inactive_product.id,
                    delivery_point_id=singapore.id,
                    price='800',
                    quantity='300',
                ),
                _make_order(
                    org_id=real_org.id,
                    product_id=product.id,
                    delivery_point_id=inactive_port.id,
                    price='700',
                    quantity='400',
                ),
            ]
        )
        await db.commit()

        result = await list_aggregated_orderbook(
            product_id=None,
            delivery_point_id=None,
            fuel_type=None,
            market_product=None,
            region=None,
            availability_window=None,
            include_off_spec=False,
            limit=256,
            db=db,
        )

        assert len(result) == 2
        assert {row.market_product for row in result} == {'BIO_METHANOL'}
        assert {row.demo_status.value for row in result} == {'REAL_ONLY', 'DEMO_ONLY'}
        assert sorted(row.total_quantity for row in result) == [Decimal('100'), Decimal('200')]
        assert all(row.product_total_order_count == 1 for row in result)

    @pytest.mark.asyncio
    async def test_product_counts_apply_listing_side_port_and_window_filters(self, db: AsyncSession):
        supplier = await _make_org(db, 'Count Supplier')
        singapore = await _make_delivery_point(db, 'Singapore', 'Asia')
        santos = await _make_delivery_point(db, 'Santos', 'Americas')
        bio_methanol = await _make_product(
            db,
            name='Bio Methanol',
            fuel_type='Methanol',
            fuel_grade='Bio',
        )
        e_methanol = await _make_product(
            db,
            name='e-Methanol',
            fuel_type='Methanol',
            fuel_grade='E',
        )
        matching_orders = [
            _make_order(
                org_id=supplier.id,
                product_id=product.id,
                delivery_point_id=singapore.id,
                price='1000',
            )
            for product in (bio_methanol, e_methanol)
        ]
        bid = _make_order(
            org_id=supplier.id,
            product_id=bio_methanol.id,
            delivery_point_id=singapore.id,
            price='950',
        )
        bid.side = OrderSide.BID
        wrong_window = _make_order(
            org_id=supplier.id,
            product_id=bio_methanol.id,
            delivery_point_id=singapore.id,
            price='1010',
            availability_window='2027-Q1',
        )
        wrong_port = _make_order(
            org_id=supplier.id,
            product_id=e_methanol.id,
            delivery_point_id=santos.id,
            price='1020',
        )
        db.add_all([*matching_orders, bid, wrong_window, wrong_port])
        await db.commit()

        asks = await list_product_counts(
            side=OrderSide.ASK,
            delivery_point_id=singapore.id,
            region=None,
            availability_window='SPOT',
            include_off_spec=False,
            db=db,
        )
        bids = await list_product_counts(
            side=OrderSide.BID,
            delivery_point_id=None,
            region='Singapore',
            availability_window='SPOT',
            include_off_spec=False,
            db=db,
        )

        assert asks.counts == {
            'BIO_METHANOL': 1,
            'E_METHANOL': 1,
            'BIO_ETHANOL': 0,
            'SYNTHETIC_ETHANOL': 0,
        }
        assert asks.total == 2
        assert bids.counts['BIO_METHANOL'] == 1
        assert bids.total == 1

    @pytest.mark.asyncio
    async def test_map_summary_uses_all_eligible_rows_and_latest_ask_per_port(self, db: AsyncSession):
        real_supplier = await _make_org(db, 'Map Real Supplier')
        demo_supplier = await _make_org(db, 'Map Demo Supplier')
        demo_supplier.provenance = OrganizationProvenance.DEMO
        singapore = await _make_delivery_point(db, 'Singapore', 'Asia')
        rotterdam = await _make_delivery_point(db, 'Rotterdam', 'Europe')
        bio_methanol = await _make_product(
            db,
            name='Bio Methanol',
            fuel_type='Methanol',
            fuel_grade='Bio',
        )
        now = datetime.now(UTC)
        older = _make_order(
            org_id=real_supplier.id,
            product_id=bio_methanol.id,
            delivery_point_id=singapore.id,
            price='1000',
            quantity='1000',
        )
        older.remaining_quantity_mt = Decimal('400')
        older.created_at = now - timedelta(minutes=2)
        latest_demo = _make_order(
            org_id=demo_supplier.id,
            product_id=bio_methanol.id,
            delivery_point_id=singapore.id,
            price='1100',
            quantity='300',
            provenance=OrganizationProvenance.DEMO,
        )
        latest_demo.created_at = now - timedelta(minutes=1)
        rotterdam_ask = _make_order(
            org_id=real_supplier.id,
            product_id=bio_methanol.id,
            delivery_point_id=rotterdam.id,
            price='1050',
            quantity='200',
        )
        rotterdam_ask.created_at = now
        excluded = _make_order(
            org_id=real_supplier.id,
            product_id=bio_methanol.id,
            delivery_point_id=singapore.id,
            price='1',
        )
        excluded.off_spec = True
        excluded.created_at = now + timedelta(minutes=1)
        db.add_all([older, latest_demo, rotterdam_ask, excluded])
        await db.commit()

        summary = await get_map_summary(db=db)

        assert len(summary.groups) == 3
        singapore_quantity = sum(
            row.total_quantity
            for row in summary.groups
            if row.delivery_point_id == singapore.id and row.side == OrderSide.ASK
        )
        assert singapore_quantity == Decimal('700')
        assert len(summary.recent_asks) == 2
        singapore_latest = next(
            row for row in summary.recent_asks if row.delivery_point_id == singapore.id
        )
        assert singapore_latest.price_per_mt_usd == Decimal('1100')
        assert singapore_latest.remaining_quantity_mt == Decimal('300')
        assert singapore_latest.evidence_class == 'DEMO'
        assert singapore_latest.source_kind == MarketSourceKind.DEMO_SEED
        assert singapore_latest.demo_status == MarketDemoStatus.DEMO_ONLY

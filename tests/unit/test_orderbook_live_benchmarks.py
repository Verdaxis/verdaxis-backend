import pytest
from contextlib import contextmanager
from datetime import UTC, datetime, timedelta
from decimal import Decimal
from uuid import uuid4

from sqlalchemy import delete, event, select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.market_catalog import DELIVERY_POINTS_BY_NAME, PRODUCTS_BY_NAME
from app.models.catalog import DeliveryPoint, Product
from app.models.live_slice_benchmark import LiveSliceBenchmark
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import OrgType, Organization, OrganizationProvenance
from app.routers.orderbook import list_asks, list_bids
from app.services.live_benchmarks import rebuild_live_slice_benchmark

REQUIRED_TABLES = [
    'organizations',
    'users',
    'products',
    'delivery_points',
    'orderbook_orders',
    'live_slice_benchmarks',
]


@contextmanager
def _count_sql_statements(async_engine):
    statements: list[str] = []

    def record_statement(_connection, _cursor, statement, _parameters, _context, _executemany):
        statements.append(statement)

    event.listen(async_engine.sync_engine, 'before_cursor_execute', record_statement)
    try:
        yield statements
    finally:
        event.remove(async_engine.sync_engine, 'before_cursor_execute', record_statement)


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


async def _make_product(db: AsyncSession, *, name: str, fuel_type: str, fuel_grade: str) -> Product:
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


def _make_order(*, org_id, side: OrderSide, product_id, delivery_point_id, price: str, quantity: str = '1000') -> OrderBookOrder:
    payload = dict(
        organization_id=org_id,
        provenance=OrganizationProvenance.REAL,
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
    async def test_ask_benchmark_uses_same_slice_vwap(self, db: AsyncSession):
        supplier = await _make_org(db, 'Supplier')
        singapore = await _make_delivery_point(db, 'Singapore', 'Asia')
        methanol = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')

        db.add_all([
            _make_order(org_id=supplier.id, side=OrderSide.ASK, product_id=methanol.id, delivery_point_id=singapore.id, price='1027', quantity='500'),
            _make_order(org_id=supplier.id, side=OrderSide.ASK, product_id=methanol.id, delivery_point_id=singapore.id, price='1045', quantity='1500'),
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
        assert by_price[Decimal('1027.00')].benchmark_price_per_mt_usd == Decimal('1040.50')
        assert by_price[Decimal('1045.00')].benchmark_price_per_mt_usd == Decimal('1040.50')
        assert by_price[Decimal('1027.00')].premium_discount_per_mt_usd == Decimal('-13.50')
        assert by_price[Decimal('1045.00')].premium_discount_per_mt_usd == Decimal('4.50')
        assert all(item.benchmark_source == 'live_slice_ask_vwap' for item in result.items)

    @pytest.mark.asyncio
    async def test_bid_benchmark_uses_same_slice_vwap(self, db: AsyncSession):
        buyer = await _make_org(db, 'Buyer')
        singapore = await _make_delivery_point(db, 'Singapore', 'Asia')
        methanol = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')

        db.add_all([
            _make_order(org_id=buyer.id, side=OrderSide.BID, product_id=methanol.id, delivery_point_id=singapore.id, price='1000', quantity='500'),
            _make_order(org_id=buyer.id, side=OrderSide.BID, product_id=methanol.id, delivery_point_id=singapore.id, price='1040', quantity='1500'),
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
        assert by_price[Decimal('1000.00')].benchmark_price_per_mt_usd == Decimal('1030.00')
        assert by_price[Decimal('1040.00')].benchmark_price_per_mt_usd == Decimal('1030.00')
        assert by_price[Decimal('1000.00')].premium_discount_per_mt_usd == Decimal('-30.00')
        assert by_price[Decimal('1040.00')].premium_discount_per_mt_usd == Decimal('10.00')
        assert all(item.benchmark_source == 'live_slice_bid_vwap' for item in result.items)

    @pytest.mark.asyncio
    async def test_rebuild_live_slice_benchmark_persists_and_removes_slice_rows(self, db: AsyncSession):
        supplier = await _make_org(db, 'Supplier')
        singapore = await _make_delivery_point(db, 'Singapore', 'Asia')
        methanol = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')

        order = _make_order(
            org_id=supplier.id,
            side=OrderSide.ASK,
            product_id=methanol.id,
            delivery_point_id=singapore.id,
            price='1030',
            quantity='800',
        )
        db.add(order)
        await db.commit()

        price = await rebuild_live_slice_benchmark(
            db,
            side=OrderSide.ASK,
            market_product='BIO_METHANOL',
            delivery_point_id=singapore.id,
            availability_window='SPOT',
        )

        row = (await db.execute(select(LiveSliceBenchmark))).scalars().one()
        assert price == Decimal('1030.00')
        assert row.benchmark_price_per_mt_usd == Decimal('1030.00')
        assert row.total_remaining_quantity_mt == Decimal('800.00')

        order.status = OrderBookStatus.CANCELLED
        await db.flush()

        price = await rebuild_live_slice_benchmark(
            db,
            side=OrderSide.ASK,
            market_product='BIO_METHANOL',
            delivery_point_id=singapore.id,
            availability_window='SPOT',
        )

        remaining = (await db.execute(select(LiveSliceBenchmark))).scalars().all()
        assert price is None
        assert remaining == []

    @pytest.mark.asyncio
    async def test_list_asks_batches_distinct_slice_benchmarks_with_bounded_queries(
        self,
        db: AsyncSession,
        async_engine,
    ):
        supplier = await _make_org(db, 'Batch Supplier')
        products = [
            await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio'),
            await _make_product(db, name='Bio Ethanol', fuel_type='Ethanol', fuel_grade='Bio'),
        ]
        delivery_points = [
            await _make_delivery_point(db, 'Dalian', 'Asia'),
            await _make_delivery_point(db, 'Busan', 'Asia'),
            await _make_delivery_point(db, 'Rotterdam', 'Europe'),
            await _make_delivery_point(db, 'Houston', 'Americas'),
        ]
        now = datetime.now(UTC)

        expected_benchmarks: dict[tuple[str, str], Decimal] = {}
        for index, (product, delivery_point) in enumerate(
            (product, delivery_point)
            for product in products
            for delivery_point in delivery_points
        ):
            base_price = Decimal('700') + Decimal(index * 25)
            expected_benchmarks[(product.market_product, str(delivery_point.id))] = base_price + Decimal('15')
            for offset, (quantity, premium) in enumerate((('100', '0'), ('300', '20'))):
                order = _make_order(
                    org_id=supplier.id,
                    side=OrderSide.ASK,
                    product_id=product.id,
                    delivery_point_id=delivery_point.id,
                    price=str(base_price + Decimal(premium)),
                    quantity=quantity,
                )
                order.certifications = ['ISCC EU']
                order.created_at = now + timedelta(seconds=index * 2 + offset)
                if index == 7:
                    order.provenance = OrganizationProvenance.DEMO
                    order.expires_at = now + timedelta(hours=1)
                db.add(order)

        off_spec = _make_order(
            org_id=supplier.id,
            side=OrderSide.ASK,
            product_id=products[0].id,
            delivery_point_id=delivery_points[0].id,
            price='999',
            quantity='50',
        )
        off_spec.availability_window = '2027-Q1'
        off_spec.off_spec = True
        off_spec.created_at = now + timedelta(minutes=1)

        uncertified = _make_order(
            org_id=supplier.id,
            side=OrderSide.ASK,
            product_id=products[0].id,
            delivery_point_id=delivery_points[0].id,
            price='1',
        )
        uncertified.certification_declared = False

        expired_demo = _make_order(
            org_id=supplier.id,
            side=OrderSide.ASK,
            product_id=products[0].id,
            delivery_point_id=delivery_points[0].id,
            price='1',
        )
        expired_demo.provenance = OrganizationProvenance.DEMO
        expired_demo.expires_at = now - timedelta(seconds=1)
        db.add_all([off_spec, uncertified, expired_demo])
        await db.commit()

        with _count_sql_statements(async_engine) as short_statements:
            short_page = await list_asks(
                product_id=None,
                delivery_point_id=None,
                fuel_type=None,
                market_product=None,
                region=None,
                availability_window=None,
                include_off_spec=True,
                sort_by='newest',
                skip=0,
                limit=4,
                db=db,
            )
        with _count_sql_statements(async_engine) as full_statements:
            full_page = await list_asks(
                product_id=None,
                delivery_point_id=None,
                fuel_type=None,
                market_product=None,
                region=None,
                availability_window=None,
                include_off_spec=True,
                sort_by='newest',
                skip=0,
                limit=100,
                db=db,
            )

        assert len(short_page.items) == 4
        assert len(full_page.items) == 17
        assert len(short_statements) == len(full_statements) == 4
        assert off_spec.id in {item.id for item in full_page.items}
        assert next(item for item in full_page.items if item.id == off_spec.id).benchmark_price_per_mt_usd is None
        assert uncertified.id not in {item.id for item in full_page.items}
        assert expired_demo.id not in {item.id for item in full_page.items}

        qualified_items = [item for item in full_page.items if item.id != off_spec.id]
        for item in qualified_items:
            assert item.benchmark_price_per_mt_usd == expected_benchmarks[
                (item.market_product, str(item.delivery_point_id))
            ]
            assert item.certifications == ['ISCC EU']
        demo_items = [item for item in qualified_items if item.is_demo_listing]
        assert len(demo_items) == 2
        assert {item.source_kind for item in demo_items} == {'DEMO_SEED'}
        assert not db.new
        assert not db.dirty

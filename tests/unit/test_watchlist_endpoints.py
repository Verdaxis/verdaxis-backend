"""Tests for the market-radar watchlist endpoints and adapters."""
from datetime import UTC, datetime, timedelta
from decimal import Decimal
from uuid import uuid4

import pytest
from sqlalchemy import delete, select, func
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.market_catalog import DELIVERY_POINTS_BY_NAME, PRODUCTS_BY_NAME
from app.models.catalog import DeliveryPoint, Product
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import OrganizationProvenance, OrgType, Organization, User, UserRole, UserStatus
from app.models.watchlist import Watchlist, WatchlistEvent, WatchlistEventType, WatchlistKind, WatchlistTarget
from fastapi import HTTPException

from app.routers.watchlists import add_watchlist_entry, create_watchlist_target, delete_watchlist_target, get_market_radar, get_watchlist_events, remove_watchlist_entry
from app.schemas.watchlist import PinTargetCreate, SliceTargetCreate, WatchlistEntryAddRequest
from app.services.watchlists import ensure_market_radar

REQUIRED_TABLES = [
    'organizations',
    'users',
    'products',
    'delivery_points',
    'orderbook_orders',
    'watchlists',
    'watchlist_entries',
    'watchlist_targets',
    'watchlist_events',
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
        for table in ('watchlist_events', 'watchlist_targets', 'watchlist_entries', 'watchlists', 'orderbook_orders', 'users', 'delivery_points', 'products', 'organizations'):
            await session.execute(delete(Base.metadata.tables[table]))
        await session.commit()


async def _make_org(db: AsyncSession, name: str, org_type: OrgType) -> Organization:
    org = Organization(
        name=f'{name}-{uuid4().hex[:6]}',
        type=org_type,
        provenance=OrganizationProvenance.REAL,
    )
    db.add(org)
    await db.flush()
    return org


async def _make_user(db: AsyncSession, org: Organization, role: UserRole) -> User:
    user = User(
        email=f'{uuid4().hex[:8]}@example.com',
        password_hash='hashed',
        role=role,
        status=UserStatus.APPROVED,
        organization_id=org.id,
    )
    db.add(user)
    await db.flush()
    return user


async def _make_product(db: AsyncSession, *, name: str, fuel_type: str, fuel_grade: str) -> Product:
    spec = PRODUCTS_BY_NAME[name]
    assert (spec.fuel_type, spec.fuel_grade) == (fuel_type, fuel_grade)
    product = Product(
        id=spec.id,
        name=spec.name,
        fuel_type=spec.fuel_type,
        fuel_grade=spec.fuel_grade,
    )
    db.add(product)
    await db.flush()
    return product


async def _make_delivery_point(db: AsyncSession, name: str) -> DeliveryPoint:
    spec = DELIVERY_POINTS_BY_NAME[name]
    delivery_point = DeliveryPoint(id=spec.id, name=spec.name, region=spec.region)
    db.add(delivery_point)
    await db.flush()
    return delivery_point


async def _make_order(
    db: AsyncSession,
    *,
    organization_id,
    product_id,
    delivery_point_id,
    side: OrderSide,
    price: str = '1000',
) -> OrderBookOrder:
    order = OrderBookOrder(
        organization_id=organization_id,
        provenance=OrganizationProvenance.REAL,
        side=side,
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        quantity_mt=Decimal('1000'),
        remaining_quantity_mt=Decimal('1000'),
        price_per_mt_usd=Decimal(price),
        availability_window='SPOT',
        status=OrderBookStatus.OPEN,
        created_at=datetime.now(UTC),
        certification_declared=True,
        certification_scheme='ISCC EU',
        specification_standard='IMPCA',
        msds_available=True,
        certifications=['ISCC EU'],
    )
    db.add(order)
    await db.flush()
    await db.refresh(order, ['product', 'delivery_point'])
    return order


class TestMarketRadarEndpoints:
    @pytest.mark.asyncio
    async def test_market_radar_auto_creates_and_returns_summary(self, db: AsyncSession):
        buyer_org = await _make_org(db, 'Buyer', OrgType.SHIPPING_LINE)
        buyer = await _make_user(db, buyer_org, UserRole.BUYER)
        singapore = await _make_delivery_point(db, 'Singapore')
        product = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')

        radar = await ensure_market_radar(db, buyer.id)
        target = WatchlistTarget(
            watchlist_id=radar.id,
            target_type='SLICE',
            market_product_code=product.market_product,
            delivery_point_id=singapore.id,
            availability_window_code='SPOT',
            snapshot_market_product=product.market_product,
            snapshot_delivery_point_name=singapore.name,
            snapshot_availability_window='SPOT',
        )
        db.add(target)
        await db.commit()

        summary = await get_market_radar(current_user=buyer, db=db)

        assert summary.kind == WatchlistKind.RADAR_DEFAULT.value
        assert len(summary.slices) == 1
        assert summary.slices[0].market_product_code == 'BIO_METHANOL'
        assert summary.slices[0].delivery_point_id == singapore.id

    @pytest.mark.asyncio
    async def test_create_pin_target_snapshots_order_identity(self, db: AsyncSession):
        supplier_org = await _make_org(db, 'Supplier', OrgType.FUEL_SUPPLIER)
        supplier = await _make_user(db, supplier_org, UserRole.SUPPLIER)
        singapore = await _make_delivery_point(db, 'Singapore')
        product = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')
        order = await _make_order(
            db,
            organization_id=supplier_org.id,
            product_id=product.id,
            delivery_point_id=singapore.id,
            side=OrderSide.ASK,
            price='1085',
        )
        radar = await ensure_market_radar(db, supplier.id)
        await db.commit()

        response = await create_watchlist_target(
            watchlist_id=radar.id,
            body=PinTargetCreate(target_type='PIN', order_id=order.id),
            current_user=supplier,
            db=db,
        )

        assert response.target_type == 'PIN'
        assert response.order_id == order.id
        assert response.market_product_code == 'BIO_METHANOL'
        assert response.snapshot_price_per_mt_usd == 1085.0
        assert response.snapshot_delivery_point_name == singapore.name

        await db.refresh(radar, ['targets'])
        assert {target.target_type.value for target in radar.targets} == {'SLICE', 'PIN'}

    @pytest.mark.asyncio
    async def test_legacy_add_entry_translates_into_market_radar_slice(self, db: AsyncSession):
        buyer_org = await _make_org(db, 'Buyer', OrgType.SHIPPING_LINE)
        buyer = await _make_user(db, buyer_org, UserRole.BUYER)
        singapore = await _make_delivery_point(db, 'Singapore')
        product = await _make_product(db, name='Bio Ethanol', fuel_type='Ethanol', fuel_grade='Bio')
        custom_watchlist = Watchlist(user_id=buyer.id, name='Legacy Book', kind=WatchlistKind.CUSTOM)
        db.add(custom_watchlist)
        await db.flush()

        response = await add_watchlist_entry(
            watchlist_id=custom_watchlist.id,
            body=WatchlistEntryAddRequest(product_id=product.id, delivery_point_id=singapore.id),
            current_user=buyer,
            db=db,
        )

        assert response.product_id == product.id
        radar = await ensure_market_radar(db, buyer.id)
        await db.refresh(radar, ['targets'])
        assert len(radar.targets) == 1
        assert radar.targets[0].market_product_code == 'BIO_ETHANOL'
        assert radar.targets[0].availability_window_code == 'SPOT'

    @pytest.mark.asyncio
    async def test_legacy_remove_entry_retracts_translated_slice(self, db: AsyncSession):
        buyer_org = await _make_org(db, 'Buyer', OrgType.SHIPPING_LINE)
        buyer = await _make_user(db, buyer_org, UserRole.BUYER)
        singapore = await _make_delivery_point(db, 'Singapore')
        product = await _make_product(db, name='Bio Ethanol', fuel_type='Ethanol', fuel_grade='Bio')
        custom_watchlist = Watchlist(user_id=buyer.id, name='Legacy Book', kind=WatchlistKind.CUSTOM)
        db.add(custom_watchlist)
        await db.flush()

        entry = await add_watchlist_entry(
            watchlist_id=custom_watchlist.id,
            body=WatchlistEntryAddRequest(product_id=product.id, delivery_point_id=singapore.id),
            current_user=buyer,
            db=db,
        )

        await remove_watchlist_entry(
            watchlist_id=custom_watchlist.id,
            entry_id=entry.id,
            current_user=buyer,
            db=db,
        )

        radar = await ensure_market_radar(db, buyer.id)
        await db.refresh(radar, ['targets'])
        assert radar.targets == []

    @pytest.mark.asyncio
    async def test_invalid_slice_window_returns_422(self, db: AsyncSession):
        buyer_org = await _make_org(db, 'Buyer', OrgType.SHIPPING_LINE)
        buyer = await _make_user(db, buyer_org, UserRole.BUYER)
        singapore = await _make_delivery_point(db, 'Singapore')
        radar = await ensure_market_radar(db, buyer.id)
        await db.commit()

        with pytest.raises(HTTPException) as exc_info:
            await create_watchlist_target(
                watchlist_id=radar.id,
                body=SliceTargetCreate(
                    target_type='SLICE',
                    market_product_code='BIO_METHANOL',
                    delivery_point_id=singapore.id,
                    availability_window_code='bad-window',
                ),
                current_user=buyer,
                db=db,
            )

        assert exc_info.value.status_code == 422

    @pytest.mark.asyncio
    async def test_invalid_event_cursor_returns_422(self, db: AsyncSession):
        buyer_org = await _make_org(db, 'Buyer', OrgType.SHIPPING_LINE)
        buyer = await _make_user(db, buyer_org, UserRole.BUYER)
        radar = await ensure_market_radar(db, buyer.id)
        await db.commit()

        with pytest.raises(HTTPException) as exc_info:
            await get_watchlist_events(
                watchlist_id=radar.id,
                cursor='not-a-valid-cursor',
                limit=20,
                current_user=buyer,
                db=db,
            )

        assert exc_info.value.status_code == 422
        assert 'cursor' in exc_info.value.detail.lower()

    @pytest.mark.asyncio
    async def test_market_radar_counts_only_execution_qualified_orders(self, db: AsyncSession):
        buyer_org = await _make_org(db, 'Buyer', OrgType.SHIPPING_LINE)
        buyer = await _make_user(db, buyer_org, UserRole.BUYER)
        supplier_org = await _make_org(db, 'Supplier', OrgType.FUEL_SUPPLIER)
        singapore = await _make_delivery_point(db, 'Singapore')
        product = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')
        radar = await ensure_market_radar(db, buyer.id)
        db.add(WatchlistTarget(
            watchlist_id=radar.id,
            target_type='SLICE',
            market_product_code='BIO_METHANOL',
            delivery_point_id=singapore.id,
            availability_window_code='SPOT',
            snapshot_market_product='BIO_METHANOL',
            snapshot_delivery_point_name=singapore.name,
            snapshot_availability_window='SPOT',
        ))
        await _make_order(
            db,
            organization_id=supplier_org.id,
            product_id=product.id,
            delivery_point_id=singapore.id,
            side=OrderSide.ASK,
        )
        unqualified = await _make_order(
            db,
            organization_id=supplier_org.id,
            product_id=product.id,
            delivery_point_id=singapore.id,
            side=OrderSide.ASK,
            price='1090',
        )
        unqualified.certification_scheme = None
        await db.commit()

        summary = await get_market_radar(current_user=buyer, db=db)

        assert summary.total_slice_count == 1
        assert summary.slices[0].active_order_count == 1

    @pytest.mark.asyncio
    async def test_get_watchlist_events_does_not_prune_old_read_events(self, db: AsyncSession):
        buyer_org = await _make_org(db, 'Buyer', OrgType.SHIPPING_LINE)
        buyer = await _make_user(db, buyer_org, UserRole.BUYER)
        singapore = await _make_delivery_point(db, 'Singapore')
        radar = await ensure_market_radar(db, buyer.id)
        target = WatchlistTarget(
            watchlist_id=radar.id,
            target_type='SLICE',
            market_product_code='BIO_METHANOL',
            delivery_point_id=singapore.id,
            availability_window_code='SPOT',
            snapshot_market_product='BIO_METHANOL',
            snapshot_delivery_point_name=singapore.name,
            snapshot_availability_window='SPOT',
        )
        db.add(target)
        await db.flush()
        db.add(WatchlistEvent(
            watchlist_id=radar.id,
            watchlist_target_id=target.id,
            event_type=WatchlistEventType.SLICE_NEW_ORDER,
            event_payload={},
            is_read=True,
            created_at=datetime.now(UTC) - timedelta(days=180),
        ))
        await db.commit()

        page = await get_watchlist_events(
            watchlist_id=radar.id,
            cursor=None,
            limit=20,
            current_user=buyer,
            db=db,
        )

        remaining = (await db.execute(select(func.count(WatchlistEvent.id)).where(WatchlistEvent.watchlist_id == radar.id))).scalar_one()
        assert len(page.items) == 1
        assert remaining == 1

    @pytest.mark.asyncio
    async def test_delete_slice_target_removes_associated_pins(self, db: AsyncSession):
        supplier_org = await _make_org(db, 'Supplier', OrgType.FUEL_SUPPLIER)
        supplier = await _make_user(db, supplier_org, UserRole.SUPPLIER)
        singapore = await _make_delivery_point(db, 'Singapore')
        product = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')
        order = await _make_order(
            db,
            organization_id=supplier_org.id,
            product_id=product.id,
            delivery_point_id=singapore.id,
            side=OrderSide.ASK,
            price='1085',
        )
        radar = await ensure_market_radar(db, supplier.id)
        await db.commit()

        slice_target = await create_watchlist_target(
            watchlist_id=radar.id,
            body=SliceTargetCreate(
                target_type='SLICE',
                market_product_code='BIO_METHANOL',
                delivery_point_id=singapore.id,
                availability_window_code='SPOT',
            ),
            current_user=supplier,
            db=db,
        )
        await create_watchlist_target(
            watchlist_id=radar.id,
            body=PinTargetCreate(target_type='PIN', order_id=order.id),
            current_user=supplier,
            db=db,
        )

        await delete_watchlist_target(
            watchlist_id=radar.id,
            target_id=slice_target.id,
            current_user=supplier,
            db=db,
        )

        radar = await ensure_market_radar(db, supplier.id)
        await db.refresh(radar, ['targets'])
        assert radar.targets == []

    @pytest.mark.asyncio
    async def test_duplicate_slice_target_returns_conflict(self, db: AsyncSession):
        buyer_org = await _make_org(db, 'Buyer', OrgType.SHIPPING_LINE)
        buyer = await _make_user(db, buyer_org, UserRole.BUYER)
        singapore = await _make_delivery_point(db, 'Singapore')
        await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')
        radar = await ensure_market_radar(db, buyer.id)
        await db.commit()

        body = SliceTargetCreate(
            target_type='SLICE',
            market_product_code='BIO_METHANOL',
            delivery_point_id=singapore.id,
            availability_window_code='SPOT',
        )
        await create_watchlist_target(watchlist_id=radar.id, body=body, current_user=buyer, db=db)

        with pytest.raises(Exception) as exc_info:
            await create_watchlist_target(watchlist_id=radar.id, body=body, current_user=buyer, db=db)

        assert '409' in str(exc_info.value) or 'Duplicate watchlist target' in str(exc_info.value)

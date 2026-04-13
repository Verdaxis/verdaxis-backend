"""Tests for watchlist event generation."""
from datetime import UTC, datetime
from decimal import Decimal
from uuid import uuid4

import pytest
from sqlalchemy import delete, select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.models.catalog import DeliveryPoint, Product
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import OrgType, Organization, User, UserRole, UserStatus
from app.models.watchlist import WatchlistEvent, WatchlistKind, WatchlistTarget, WatchlistTargetType
from app.services.watchlist_events import emit_order_created, emit_order_updated, emit_pin_updated, emit_slice_state_changed, sync_target_snapshot
from app.services.watchlists import ensure_market_radar

REQUIRED_TABLES = [
    'organizations',
    'users',
    'products',
    'delivery_points',
    'orderbook_orders',
    'watchlists',
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
    session_factory = async_sessionmaker(bind=async_engine, class_=AsyncSession, expire_on_commit=False, autoflush=False)
    async with session_factory() as session:
        yield session
        for table in ('watchlist_events', 'watchlist_targets', 'watchlists', 'orderbook_orders', 'users', 'delivery_points', 'products', 'organizations'):
            await session.execute(delete(Base.metadata.tables[table]))
        await session.commit()


async def _make_org(db: AsyncSession, name: str, org_type: OrgType) -> Organization:
    org = Organization(name=f'{name}-{uuid4().hex[:6]}', type=org_type)
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
    product = Product(name=f'{name}-{uuid4().hex[:6]}', fuel_type=fuel_type, fuel_grade=fuel_grade)
    db.add(product)
    await db.flush()
    return product


async def _make_delivery_point(db: AsyncSession, name: str) -> DeliveryPoint:
    delivery_point = DeliveryPoint(name=f'{name}-{uuid4().hex[:6]}', region='Asia')
    db.add(delivery_point)
    await db.flush()
    return delivery_point


async def _make_order(db: AsyncSession, *, org_id, product_id, delivery_point_id, price: str) -> OrderBookOrder:
    order = OrderBookOrder(
        organization_id=org_id,
        side=OrderSide.ASK,
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


@pytest.mark.asyncio
async def test_emit_order_created_creates_slice_new_order_event(db: AsyncSession):
    user_org = await _make_org(db, 'Buyer', OrgType.SHIPPING_LINE)
    user = await _make_user(db, user_org, UserRole.BUYER)
    supplier_org = await _make_org(db, 'Supplier', OrgType.FUEL_SUPPLIER)
    singapore = await _make_delivery_point(db, 'Singapore')
    product = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')
    radar = await ensure_market_radar(db, user.id)
    slice_target = WatchlistTarget(
        watchlist_id=radar.id,
        target_type=WatchlistTargetType.SLICE,
        market_product_code='BIO_METHANOL',
        delivery_point_id=singapore.id,
        availability_window_code='SPOT',
        snapshot_market_product='BIO_METHANOL',
        snapshot_delivery_point_name=singapore.name,
        snapshot_availability_window='SPOT',
    )
    db.add(slice_target)
    await db.flush()
    order = await _make_order(db, org_id=supplier_org.id, product_id=product.id, delivery_point_id=singapore.id, price='1090')

    await emit_order_created(db, order, previous_best_price=None)
    await db.commit()

    events = (await db.execute(select(WatchlistEvent).order_by(WatchlistEvent.created_at.asc()))).scalars().all()
    event_types = [event.event_type.value for event in events]
    assert 'SLICE_NEW_ORDER' in event_types
    assert 'SLICE_BEST_PRICE_MOVED' in event_types


@pytest.mark.asyncio
async def test_emit_order_updated_creates_pin_and_slice_events(db: AsyncSession):
    user_org = await _make_org(db, 'Buyer', OrgType.SHIPPING_LINE)
    user = await _make_user(db, user_org, UserRole.BUYER)
    supplier_org = await _make_org(db, 'Supplier', OrgType.FUEL_SUPPLIER)
    singapore = await _make_delivery_point(db, 'Singapore')
    product = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')
    radar = await ensure_market_radar(db, user.id)
    order = await _make_order(db, org_id=supplier_org.id, product_id=product.id, delivery_point_id=singapore.id, price='1090')

    slice_target = WatchlistTarget(
        watchlist_id=radar.id,
        target_type=WatchlistTargetType.SLICE,
        market_product_code='BIO_METHANOL',
        delivery_point_id=singapore.id,
        availability_window_code='SPOT',
        snapshot_market_product='BIO_METHANOL',
        snapshot_delivery_point_name=singapore.name,
        snapshot_availability_window='SPOT',
    )
    pin_target = WatchlistTarget(
        watchlist_id=radar.id,
        target_type=WatchlistTargetType.PIN,
        order_id=order.id,
        market_product_code='BIO_METHANOL',
        delivery_point_id=singapore.id,
        availability_window_code='SPOT',
    )
    sync_target_snapshot(pin_target, order)
    db.add_all([slice_target, pin_target])
    await db.flush()

    before = {
        'price_per_mt_usd': Decimal('1090'),
        'remaining_quantity_mt': Decimal('1000'),
        'status': OrderBookStatus.OPEN,
        'slice_best_price_per_mt_usd': Decimal('1090'),
    }
    order.price_per_mt_usd = Decimal('1115')
    order.remaining_quantity_mt = Decimal('700')
    order.status = OrderBookStatus.PARTIALLY_FILLED

    await emit_order_updated(db, before=before, order=order)
    await db.commit()

    events = (await db.execute(select(WatchlistEvent).order_by(WatchlistEvent.created_at.asc()))).scalars().all()
    event_types = [event.event_type.value for event in events]
    assert 'PIN_PRICE_CHANGED' in event_types
    assert 'PIN_PARTIALLY_FILLED' in event_types
    assert 'SLICE_BEST_PRICE_MOVED' in event_types

    refreshed_pin = await db.get(WatchlistTarget, pin_target.id)
    assert refreshed_pin.snapshot_price_per_mt_usd == 1115.0
    assert refreshed_pin.snapshot_remaining_quantity_mt == 700.0


@pytest.mark.asyncio
async def test_emit_order_updated_emits_slice_best_price_when_top_order_is_removed(db: AsyncSession):
    user_org = await _make_org(db, 'Buyer', OrgType.SHIPPING_LINE)
    user = await _make_user(db, user_org, UserRole.BUYER)
    supplier_org = await _make_org(db, 'Supplier', OrgType.FUEL_SUPPLIER)
    singapore = await _make_delivery_point(db, 'Singapore')
    product = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')
    radar = await ensure_market_radar(db, user.id)

    best_order = await _make_order(db, org_id=supplier_org.id, product_id=product.id, delivery_point_id=singapore.id, price='1090')
    await _make_order(db, org_id=supplier_org.id, product_id=product.id, delivery_point_id=singapore.id, price='1115')

    slice_target = WatchlistTarget(
        watchlist_id=radar.id,
        target_type=WatchlistTargetType.SLICE,
        market_product_code='BIO_METHANOL',
        delivery_point_id=singapore.id,
        availability_window_code='SPOT',
        snapshot_market_product='BIO_METHANOL',
        snapshot_delivery_point_name=singapore.name,
        snapshot_availability_window='SPOT',
    )
    db.add(slice_target)
    await db.flush()

    before = {
        'price_per_mt_usd': Decimal('1090'),
        'remaining_quantity_mt': Decimal('1000'),
        'status': OrderBookStatus.OPEN,
        'slice_best_price_per_mt_usd': Decimal('1090'),
    }
    best_order.status = OrderBookStatus.CANCELLED

    await emit_order_updated(db, before=before, order=best_order)
    await db.commit()

    events = (await db.execute(select(WatchlistEvent).order_by(WatchlistEvent.created_at.asc()))).scalars().all()
    move_events = [event for event in events if event.event_type.value == 'SLICE_BEST_PRICE_MOVED']
    assert move_events
    assert move_events[-1].event_payload['old_price_per_mt_usd'] == 1090.0
    assert move_events[-1].event_payload['new_price_per_mt_usd'] == 1115.0


@pytest.mark.asyncio
async def test_emit_pin_and_slice_events_for_auto_matched_resting_order(db: AsyncSession):
    user_org = await _make_org(db, 'Buyer', OrgType.SHIPPING_LINE)
    user = await _make_user(db, user_org, UserRole.BUYER)
    supplier_org = await _make_org(db, 'Supplier', OrgType.FUEL_SUPPLIER)
    singapore = await _make_delivery_point(db, 'Singapore')
    product = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')
    radar = await ensure_market_radar(db, user.id)

    best_order = await _make_order(db, org_id=supplier_org.id, product_id=product.id, delivery_point_id=singapore.id, price='1090')
    await _make_order(db, org_id=supplier_org.id, product_id=product.id, delivery_point_id=singapore.id, price='1115')

    slice_target = WatchlistTarget(
        watchlist_id=radar.id,
        target_type=WatchlistTargetType.SLICE,
        market_product_code='BIO_METHANOL',
        delivery_point_id=singapore.id,
        availability_window_code='SPOT',
        snapshot_market_product='BIO_METHANOL',
        snapshot_delivery_point_name=singapore.name,
        snapshot_availability_window='SPOT',
    )
    pin_target = WatchlistTarget(
        watchlist_id=radar.id,
        target_type=WatchlistTargetType.PIN,
        order_id=best_order.id,
        market_product_code='BIO_METHANOL',
        delivery_point_id=singapore.id,
        availability_window_code='SPOT',
    )
    sync_target_snapshot(pin_target, best_order)
    db.add_all([slice_target, pin_target])
    await db.flush()

    best_order.remaining_quantity_mt = Decimal('0')
    best_order.status = OrderBookStatus.FILLED

    await emit_pin_updated(
        db,
        before={
            'price_per_mt_usd': Decimal('1090'),
            'remaining_quantity_mt': Decimal('1000'),
            'status': OrderBookStatus.OPEN,
        },
        order=best_order,
    )
    await emit_slice_state_changed(
        db,
        market_product_code='BIO_METHANOL',
        delivery_point_id=singapore.id,
        availability_window_code='SPOT',
        side=OrderSide.ASK,
        before_best_price=Decimal('1090'),
        quiet_order_id=best_order.id,
    )
    await db.commit()

    events = (await db.execute(select(WatchlistEvent).order_by(WatchlistEvent.created_at.asc()))).scalars().all()
    event_types = [event.event_type.value for event in events]
    assert 'PIN_FILLED' in event_types
    assert 'SLICE_BEST_PRICE_MOVED' in event_types

    move_events = [event for event in events if event.event_type.value == 'SLICE_BEST_PRICE_MOVED']
    assert move_events[-1].event_payload['old_price_per_mt_usd'] == 1090.0
    assert move_events[-1].event_payload['new_price_per_mt_usd'] == 1115.0

"""Tests for trade-path watchlist event propagation."""
from datetime import UTC, datetime
from decimal import Decimal
from types import SimpleNamespace
from uuid import uuid4

import pytest
from fastapi import HTTPException
from sqlalchemy import delete, select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.models.audit import AuditLog  # noqa: F401 — registers audit_logs on Base.metadata
from app.models.catalog import DeliveryPoint, Product
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide, Trade
from app.models.user import OrgType, Organization, User, UserRole, UserStatus
from app.models.watchlist import WatchlistEvent, WatchlistTarget, WatchlistTargetType
from app.routers import trades as trades_router


def _fake_request():
    """Minimal Request stand-in for endpoints that record audit entries."""
    return SimpleNamespace(headers={}, client=SimpleNamespace(host="127.0.0.1"))
from app.services.watchlists import ensure_market_radar
from app.services.watchlist_events import sync_target_snapshot

REQUIRED_TABLES = [
    'audit_logs',
    'organizations',
    'users',
    'products',
    'delivery_points',
    'orderbook_orders',
    'trades',
    'live_slice_benchmarks',
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
        for table in ('audit_logs', 'watchlist_events', 'watchlist_targets', 'watchlists', 'live_slice_benchmarks', 'trades', 'orderbook_orders', 'users', 'delivery_points', 'products', 'organizations'):
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


async def _make_product(db: AsyncSession) -> Product:
    product = Product(name=f'Bio Methanol-{uuid4().hex[:6]}', fuel_type='Methanol', fuel_grade='Bio')
    db.add(product)
    await db.flush()
    return product


async def _make_delivery_point(db: AsyncSession) -> DeliveryPoint:
    delivery_point = DeliveryPoint(name=f'Singapore-{uuid4().hex[:6]}', region='Asia')
    db.add(delivery_point)
    await db.flush()
    return delivery_point


async def _make_ask(
    db: AsyncSession,
    *,
    org_id,
    product_id,
    delivery_point_id,
    price='1090',
    qty='1000',
    certification_declared: bool = True,
    certification_scheme: str | None = 'ISCC EU',
) -> OrderBookOrder:
    order = OrderBookOrder(
        organization_id=org_id,
        side=OrderSide.ASK,
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        quantity_mt=Decimal(qty),
        remaining_quantity_mt=Decimal(qty),
        price_per_mt_usd=Decimal(price),
        availability_window='SPOT',
        status=OrderBookStatus.OPEN,
        created_at=datetime.now(UTC),
        certification_declared=certification_declared,
        certification_scheme=certification_scheme,
        specification_standard='IMPCA',
        msds_available=True,
        certifications=['ISCC EU'] if certification_scheme else [],
    )
    db.add(order)
    await db.flush()
    await db.refresh(order, ['product', 'delivery_point'])
    return order


@pytest.mark.asyncio
async def test_create_trade_rejects_demo_listing(monkeypatch, db: AsyncSession):
    buyer_org = await _make_org(db, 'Buyer', OrgType.SHIPPING_LINE)
    supplier_org = await _make_org(db, 'Supplier', OrgType.FUEL_SUPPLIER)
    product = await _make_product(db)
    delivery_point = await _make_delivery_point(db)
    ask = await _make_ask(db, org_id=supplier_org.id, product_id=product.id, delivery_point_id=delivery_point.id)
    await db.commit()

    monkeypatch.setattr(
        trades_router,
        'is_demo_market_organization',
        lambda org_id: org_id == supplier_org.id,
    )

    current_user = SimpleNamespace(id=uuid4(), organization_id=buyer_org.id, role=UserRole.BUYER)
    payload = trades_router.TradeCreate(order_id=ask.id, quantity_mt=Decimal('100'))

    with pytest.raises(HTTPException) as exc_info:
        await trades_router.create_trade(payload=payload, request=_fake_request(), db=db, current_user=current_user)

    assert exc_info.value.status_code == 400
    assert 'Demo listings' in exc_info.value.detail


@pytest.mark.asyncio
async def test_create_trade_rejects_non_executable_order(monkeypatch, db: AsyncSession):
    buyer_org = await _make_org(db, 'Buyer', OrgType.SHIPPING_LINE)
    buyer = await _make_user(db, buyer_org, UserRole.BUYER)
    supplier_org = await _make_org(db, 'Supplier', OrgType.FUEL_SUPPLIER)
    product = await _make_product(db)
    delivery_point = await _make_delivery_point(db)
    ask = await _make_ask(
        db,
        org_id=supplier_org.id,
        product_id=product.id,
        delivery_point_id=delivery_point.id,
        certification_declared=False,
        certification_scheme=None,
    )
    await db.commit()

    current_user = SimpleNamespace(id=uuid4(), organization_id=buyer_org.id, role=UserRole.BUYER)
    payload = trades_router.TradeCreate(order_id=ask.id, quantity_mt=Decimal('100'))

    with pytest.raises(HTTPException) as exc_info:
        await trades_router.create_trade(payload=payload, request=_fake_request(), db=db, current_user=current_user)

    assert exc_info.value.status_code == 400
    assert 'execution-qualified' in exc_info.value.detail


@pytest.mark.asyncio
async def test_create_trade_emits_pin_and_slice_events(monkeypatch, db: AsyncSession):
    buyer_org = await _make_org(db, 'Buyer', OrgType.SHIPPING_LINE)
    buyer = await _make_user(db, buyer_org, UserRole.BUYER)
    supplier_org = await _make_org(db, 'Supplier', OrgType.FUEL_SUPPLIER)
    product = await _make_product(db)
    delivery_point = await _make_delivery_point(db)
    ask = await _make_ask(db, org_id=supplier_org.id, product_id=product.id, delivery_point_id=delivery_point.id, price='1090')
    await _make_ask(db, org_id=supplier_org.id, product_id=product.id, delivery_point_id=delivery_point.id, price='1110')

    radar = await ensure_market_radar(db, buyer.id)
    slice_target = WatchlistTarget(
        watchlist_id=radar.id,
        target_type=WatchlistTargetType.SLICE,
        market_product_code='BIO_METHANOL',
        delivery_point_id=delivery_point.id,
        availability_window_code='SPOT',
        snapshot_market_product='BIO_METHANOL',
        snapshot_delivery_point_name=delivery_point.name,
        snapshot_availability_window='SPOT',
    )
    pin_target = WatchlistTarget(
        watchlist_id=radar.id,
        target_type=WatchlistTargetType.PIN,
        order_id=ask.id,
        market_product_code='BIO_METHANOL',
        delivery_point_id=delivery_point.id,
        availability_window_code='SPOT',
    )
    sync_target_snapshot(pin_target, ask)
    db.add_all([slice_target, pin_target])
    await db.commit()

    async def _noop_notify(*args, **kwargs):
        return None

    async def _noop_publish(*args, **kwargs):
        return None

    monkeypatch.setattr(trades_router, 'notify_org_users', _noop_notify)
    monkeypatch.setattr(trades_router.event_bus, 'publish', _noop_publish)

    payload = trades_router.TradeCreate(order_id=ask.id, quantity_mt=Decimal('1000'))
    current_user = SimpleNamespace(id=uuid4(), organization_id=buyer_org.id, role=UserRole.BUYER)

    response = await trades_router.create_trade(payload=payload, request=_fake_request(), db=db, current_user=current_user)

    assert response.status == 'PENDING_CONFIRMATION'
    events = (await db.execute(select(WatchlistEvent).order_by(WatchlistEvent.created_at.asc()))).scalars().all()
    event_types = [event.event_type.value for event in events]
    assert 'PIN_FILLED' in event_types
    assert 'SLICE_BEST_PRICE_MOVED' in event_types


@pytest.mark.asyncio
async def test_decline_trade_does_not_revive_cancelled_order(monkeypatch, db: AsyncSession):
    """Declining a pending trade must not resurrect an order the owner cancelled."""
    buyer_org = await _make_org(db, 'Buyer', OrgType.SHIPPING_LINE)
    supplier_org = await _make_org(db, 'Supplier', OrgType.FUEL_SUPPLIER)
    supplier = await _make_user(db, supplier_org, UserRole.SUPPLIER)
    product = await _make_product(db)
    delivery_point = await _make_delivery_point(db)
    ask = await _make_ask(db, org_id=supplier_org.id, product_id=product.id, delivery_point_id=delivery_point.id)

    trade = Trade(
        bid_order_id=None,
        ask_order_id=ask.id,
        buyer_id=buyer_org.id,
        seller_id=supplier_org.id,
        initiated_by=trades_router.Initiator.BUYER,
        quantity_mt=Decimal('400'),
        price_per_mt_usd=ask.price_per_mt_usd,
        status=trades_router.TradeStatus.PENDING_CONFIRMATION,
    )
    db.add(trade)
    # Owner cancelled the resting order while the trade was pending
    ask.remaining_quantity_mt = Decimal('600')
    ask.status = OrderBookStatus.CANCELLED
    await db.commit()

    async def _noop_notify(*args, **kwargs):
        return None

    monkeypatch.setattr(trades_router, 'notify_org_users', _noop_notify)

    current_user = SimpleNamespace(id=uuid4(), organization_id=supplier_org.id, role=UserRole.SUPPLIER)
    response = await trades_router.decline_trade(trade_id=trade.id, request=_fake_request(), db=db, current_user=current_user)

    assert response.status == 'DECLINED'
    refreshed = await db.get(OrderBookOrder, ask.id)
    assert refreshed.status == OrderBookStatus.CANCELLED
    assert refreshed.remaining_quantity_mt == Decimal('600')


@pytest.mark.asyncio
async def test_decline_trade_restores_watchlist_state(monkeypatch, db: AsyncSession):
    buyer_org = await _make_org(db, 'Buyer', OrgType.SHIPPING_LINE)
    buyer = await _make_user(db, buyer_org, UserRole.BUYER)
    supplier_org = await _make_org(db, 'Supplier', OrgType.FUEL_SUPPLIER)
    supplier = await _make_user(db, supplier_org, UserRole.SUPPLIER)
    product = await _make_product(db)
    delivery_point = await _make_delivery_point(db)
    ask = await _make_ask(db, org_id=supplier_org.id, product_id=product.id, delivery_point_id=delivery_point.id)

    radar = await ensure_market_radar(db, buyer.id)
    slice_target = WatchlistTarget(
        watchlist_id=radar.id,
        target_type=WatchlistTargetType.SLICE,
        market_product_code='BIO_METHANOL',
        delivery_point_id=delivery_point.id,
        availability_window_code='SPOT',
        snapshot_market_product='BIO_METHANOL',
        snapshot_delivery_point_name=delivery_point.name,
        snapshot_availability_window='SPOT',
    )
    pin_target = WatchlistTarget(
        watchlist_id=radar.id,
        target_type=WatchlistTargetType.PIN,
        order_id=ask.id,
        market_product_code='BIO_METHANOL',
        delivery_point_id=delivery_point.id,
        availability_window_code='SPOT',
    )
    sync_target_snapshot(pin_target, ask)
    db.add_all([slice_target, pin_target])
    await db.flush()

    trade = Trade(
        bid_order_id=None,
        ask_order_id=ask.id,
        buyer_id=buyer_org.id,
        seller_id=supplier_org.id,
        initiated_by=trades_router.Initiator.BUYER,
        quantity_mt=Decimal('400'),
        price_per_mt_usd=ask.price_per_mt_usd,
        status=trades_router.TradeStatus.PENDING_CONFIRMATION,
    )
    db.add(trade)
    ask.remaining_quantity_mt = Decimal('600')
    ask.status = OrderBookStatus.PARTIALLY_FILLED
    await db.commit()

    async def _noop_notify(*args, **kwargs):
        return None

    monkeypatch.setattr(trades_router, 'notify_org_users', _noop_notify)

    current_user = SimpleNamespace(id=uuid4(), organization_id=supplier_org.id, role=UserRole.SUPPLIER)
    response = await trades_router.decline_trade(trade_id=trade.id, request=_fake_request(), db=db, current_user=current_user)

    assert response.status == 'DECLINED'
    events = (await db.execute(select(WatchlistEvent).order_by(WatchlistEvent.created_at.asc()))).scalars().all()
    event_types = [event.event_type.value for event in events]
    assert 'PIN_QUANTITY_CHANGED' in event_types or 'PIN_PARTIALLY_FILLED' in event_types
    refreshed_pin = await db.get(WatchlistTarget, pin_target.id)
    assert refreshed_pin.snapshot_remaining_quantity_mt == 1000.0

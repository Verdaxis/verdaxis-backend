"""Tests for trade-path watchlist event propagation."""
from datetime import UTC, datetime, timedelta
from decimal import Decimal
from types import SimpleNamespace
from uuid import uuid4

import pytest
from fastapi import HTTPException
from sqlalchemy import delete, select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.market_catalog import BIOFUEL_SPECIFICATION_STANDARDS, DELIVERY_POINTS_BY_NAME, PRODUCTS_BY_NAME
from app.models.audit import AuditLog  # noqa: F401 — registers audit_logs on Base.metadata
from app.models.catalog import DeliveryPoint, Product
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide, Trade
from app.models.market_event import MarketEventOutbox
from app.models.negotiation import Negotiation, NegotiationStatus
from app.models.orders import Commission
from app.models.rfq import RFQ, RFQQuote, RFQStatus, QuoteStatus
from app.models.subscription import Subscription, SubscriptionTier
from app.models.user import (
    OrganizationProvenance,
    OrgType,
    Organization,
    User,
    UserRole,
    UserStatus,
)
from app.models.watchlist import WatchlistEvent, WatchlistTarget, WatchlistTargetType
from app.routers import trades as trades_router
from app.services import demo_activity
from app.services.demo_market import DEMO_ACTIVITY_BUYER_ORG_ID, DEMO_ACTIVITY_SELLER_ORG_ID
from app.services.watchlists import ensure_market_radar
from app.services.watchlist_events import sync_target_snapshot


def _fake_request():
    """Minimal Request stand-in for endpoints that record audit entries."""
    return SimpleNamespace(headers={}, client=SimpleNamespace(host="127.0.0.1"))


REQUIRED_TABLES = [
    'audit_logs',
    'organizations',
    'users',
    'products',
    'delivery_points',
    'orderbook_orders',
    'trades',
    'subscriptions',
    'live_slice_benchmarks',
    'watchlists',
    'watchlist_targets',
    'watchlist_events',
    'market_event_outbox',
    'match_suggestions',
    'negotiations',
    'rfqs',
    'rfq_quotes',
    'commissions',
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
        await session.rollback()
        for table in ('commissions', 'rfq_quotes', 'rfqs', 'negotiations', 'match_suggestions', 'audit_logs', 'watchlist_events', 'watchlist_targets', 'watchlists', 'market_event_outbox', 'live_slice_benchmarks', 'trades', 'subscriptions', 'orderbook_orders', 'users', 'delivery_points', 'products', 'organizations'):
            await session.execute(delete(Base.metadata.tables[table]))
        await session.commit()


async def _make_org(db: AsyncSession, name: str, org_type: OrgType) -> Organization:
    org = Organization(
        name=f'{name}-{uuid4().hex[:6]}',
        type=org_type,
        provenance=OrganizationProvenance.REAL,
        verification_status="APPROVED",
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
        email_verified=True,
        kyc_status="APPROVED",
        organization_id=org.id,
    )
    db.add(user)
    await db.flush()
    return user


async def _make_product(db: AsyncSession, name: str = 'Bio Methanol') -> Product:
    spec = PRODUCTS_BY_NAME[name]
    product = Product(
        id=spec.id,
        name=spec.name,
        fuel_type=spec.fuel_type,
        fuel_grade=spec.fuel_grade,
        is_active=True,
    )
    db.add(product)
    await db.flush()
    return product


async def _make_delivery_point(db: AsyncSession) -> DeliveryPoint:
    spec = DELIVERY_POINTS_BY_NAME['Singapore']
    delivery_point = DeliveryPoint(
        id=spec.id,
        name=spec.name,
        region=spec.region,
        is_active=True,
    )
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
    owner_user_id=None,
) -> OrderBookOrder:
    order = OrderBookOrder(
        organization_id=org_id,
        owner_user_id=owner_user_id,
        provenance=OrganizationProvenance.REAL,
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


def _pending_trade(
    ask: OrderBookOrder,
    *,
    buyer_id,
    seller_id,
    quantity: str,
) -> Trade:
    return Trade(
        bid_order_id=None,
        ask_order_id=ask.id,
        buyer_id=buyer_id,
        seller_id=seller_id,
        initiator_org_id=buyer_id,
        buyer_provenance=OrganizationProvenance.REAL,
        seller_provenance=OrganizationProvenance.REAL,
        initiated_by=trades_router.Initiator.BUYER,
        product_id=ask.product_id,
        product_name=ask.product.name,
        fuel_type=ask.product.fuel_type,
        fuel_grade=ask.product.fuel_grade,
        market_product=ask.market_product,
        delivery_point_id=ask.delivery_point_id,
        delivery_point_name=ask.delivery_point.name,
        delivery_point_region=ask.delivery_point.region,
        availability_window=ask.availability_window,
        quantity_mt=Decimal(quantity),
        price_per_mt_usd=ask.price_per_mt_usd,
        status=trades_router.TradeStatus.PENDING_CONFIRMATION,
    )


@pytest.mark.asyncio
async def test_create_trade_rejects_demo_listing(monkeypatch, db: AsyncSession):
    buyer_org = await _make_org(db, 'Buyer', OrgType.SHIPPING_LINE)
    buyer = await _make_user(db, buyer_org, UserRole.BUYER)
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

    current_user = buyer
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

    current_user = buyer
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
    supplier = await _make_user(db, supplier_org, UserRole.SUPPLIER)
    product = await _make_product(db)
    delivery_point = await _make_delivery_point(db)
    ask = await _make_ask(db, org_id=supplier_org.id, product_id=product.id, delivery_point_id=delivery_point.id, price='1090', owner_user_id=supplier.id)
    await _make_ask(db, org_id=supplier_org.id, product_id=product.id, delivery_point_id=delivery_point.id, price='1110', owner_user_id=supplier.id)

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

    monkeypatch.setattr(trades_router, 'notify_org_users', _noop_notify)

    payload = trades_router.TradeCreate(order_id=ask.id, quantity_mt=Decimal('1000'))
    current_user = buyer
    db.add(
        Subscription(
            org_id=supplier_org.id,
            tier=SubscriptionTier.STANDARD,
            started_at=datetime.now(UTC),
        )
    )
    await db.flush()

    response = await trades_router.create_trade(payload=payload, request=_fake_request(), db=db, current_user=current_user)

    assert response.status == 'PENDING_CONFIRMATION'
    assert response.is_anonymous is True
    assert response.seller_id is None
    assert response.seller_name == 'Anonymous'
    assert response.commission_payer == 'SELLER'
    assert response.commission_plan is None
    assert response.commission_fee_per_mt_usd is None
    assert response.commission_rate_pct == Decimal('0')
    events = (await db.execute(select(WatchlistEvent).order_by(WatchlistEvent.created_at.asc()))).scalars().all()
    event_types = [event.event_type.value for event in events]
    assert 'PIN_FILLED' in event_types
    assert 'SLICE_BEST_PRICE_MOVED' in event_types
    outbox = (await db.execute(select(MarketEventOutbox))).scalars().one()
    assert outbox.event_type == 'trade_created'
    assert set(outbox.participant_org_ids) == {
        str(buyer_org.id),
        str(supplier_org.id),
    }


@pytest.mark.asyncio
async def test_decline_trade_does_not_revive_cancelled_order(monkeypatch, db: AsyncSession):
    """Declining a pending trade must not resurrect an order the owner cancelled."""
    buyer_org = await _make_org(db, 'Buyer', OrgType.SHIPPING_LINE)
    supplier_org = await _make_org(db, 'Supplier', OrgType.FUEL_SUPPLIER)
    supplier = await _make_user(db, supplier_org, UserRole.SUPPLIER)
    product = await _make_product(db)
    delivery_point = await _make_delivery_point(db)
    ask = await _make_ask(db, org_id=supplier_org.id, product_id=product.id, delivery_point_id=delivery_point.id)

    trade = _pending_trade(
        ask,
        buyer_id=buyer_org.id,
        seller_id=supplier_org.id,
        quantity='400',
    )
    db.add(trade)
    # Owner cancelled the resting order while the trade was pending
    ask.remaining_quantity_mt = Decimal('600')
    ask.status = OrderBookStatus.CANCELLED
    await db.commit()

    async def _noop_notify(*args, **kwargs):
        return None

    monkeypatch.setattr(trades_router, 'notify_org_users', _noop_notify)

    current_user = supplier
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

    trade = _pending_trade(
        ask,
        buyer_id=buyer_org.id,
        seller_id=supplier_org.id,
        quantity='400',
    )
    db.add(trade)
    ask.remaining_quantity_mt = Decimal('600')
    ask.status = OrderBookStatus.PARTIALLY_FILLED
    await db.commit()

    async def _noop_notify(*args, **kwargs):
        return None

    monkeypatch.setattr(trades_router, 'notify_org_users', _noop_notify)

    current_user = supplier
    response = await trades_router.decline_trade(trade_id=trade.id, request=_fake_request(), db=db, current_user=current_user)

    assert response.status == 'DECLINED'
    events = (await db.execute(select(WatchlistEvent).order_by(WatchlistEvent.created_at.asc()))).scalars().all()
    event_types = [event.event_type.value for event in events]
    assert 'PIN_QUANTITY_CHANGED' in event_types or 'PIN_PARTIALLY_FILLED' in event_types
    refreshed_pin = await db.get(WatchlistTarget, pin_target.id)
    assert refreshed_pin.snapshot_remaining_quantity_mt == 1000.0


@pytest.mark.asyncio
@pytest.mark.parametrize("product_name", ["B30", "B100"])
async def test_biofuel_direct_bid_fill_requires_qualified_ask_but_buyers_can_fill_asks(monkeypatch, db, product_name):
    buyer_org = await _make_org(db, "Biofuel Buyer", OrgType.SHIPPING_LINE)
    buyer = await _make_user(db, buyer_org, UserRole.BUYER)
    supplier_org = await _make_org(db, "Biofuel Supplier", OrgType.FUEL_SUPPLIER)
    supplier = await _make_user(db, supplier_org, UserRole.SUPPLIER)
    product = await _make_product(db, product_name)
    port = await _make_delivery_point(db)
    bid = await _make_ask(
        db, org_id=buyer_org.id, product_id=product.id,
        delivery_point_id=port.id, owner_user_id=buyer.id,
    )
    bid.side = OrderSide.BID
    await db.commit()

    with pytest.raises(HTTPException, match="Place a qualified ASK") as failure:
        await trades_router.create_trade(
            payload=trades_router.TradeCreate(order_id=bid.id, quantity_mt=Decimal("100")),
            request=_fake_request(), db=db, current_user=supplier,
        )
    assert failure.value.status_code == 400
    assert bid.remaining_quantity_mt == Decimal("1000")
    assert (await db.execute(select(Trade))).scalars().all() == []

    ask = await _make_ask(
        db, org_id=supplier_org.id, product_id=product.id,
        delivery_point_id=port.id, owner_user_id=supplier.id,
    )
    ask.specification_standard = BIOFUEL_SPECIFICATION_STANDARDS[product.id]
    ask.carbon_intensity_gco2_mj = Decimal("70")
    ask.carbon_intensity_method = "Lifecycle calculation for the whole supplied fuel"
    ask.feedstock = "Used cooking oil FAME"
    ask.origin = "Singapore"
    await db.commit()

    async def no_notification(*args, **kwargs):
        pass

    monkeypatch.setattr(trades_router, "notify_org_users", no_notification)
    response = await trades_router.create_trade(
        payload=trades_router.TradeCreate(order_id=ask.id, quantity_mt=Decimal("100")),
        request=_fake_request(), db=db, current_user=buyer,
    )
    assert response.status == "PENDING_CONFIRMATION"
    assert response.market_product == product_name
    assert ask.remaining_quantity_mt == Decimal("900")


@pytest.mark.asyncio
@pytest.mark.parametrize("prune_path", ["age", "cap", "linked_age", "linked_cap"])
async def test_demo_pruning_preserves_customer_pins_and_removes_unreferenced_orders(monkeypatch, db, prune_path):
    now = datetime(2026, 9, 27, 0, tzinfo=UTC)
    created_at = now - timedelta(days=8 if "age" in prune_path else 1)
    viewer_org = await _make_org(db, "Viewer", OrgType.SHIPPING_LINE)
    viewer = await _make_user(db, viewer_org, UserRole.BUYER)
    for organization_id, kind in (
        (DEMO_ACTIVITY_BUYER_ORG_ID, OrgType.SHIPPING_LINE),
        (DEMO_ACTIVITY_SELLER_ORG_ID, OrgType.FUEL_SUPPLIER),
    ):
        db.add(Organization(
            id=organization_id, name=f"Demo {kind.value}", type=kind,
            provenance=OrganizationProvenance.DEMO, verification_status="APPROVED",
        ))
    product = await _make_product(db)
    port = await _make_delivery_point(db)
    pinned, unreferenced = [
        await _make_ask(db, org_id=DEMO_ACTIVITY_SELLER_ORG_ID, product_id=product.id, delivery_point_id=port.id)
        for _ in range(2)
    ]
    for order in (pinned, unreferenced):
        order.provenance = OrganizationProvenance.DEMO
        order.created_at = created_at
        order.expires_at = now + timedelta(days=1)
    radar = await ensure_market_radar(db, viewer.id)
    pin = WatchlistTarget(
        watchlist_id=radar.id, target_type=WatchlistTargetType.PIN, order_id=pinned.id,
        market_product_code="BIO_METHANOL", delivery_point_id=port.id, availability_window_code="SPOT",
    )
    db.add(pin)
    if prune_path.startswith("linked"):
        for order in (pinned, unreferenced):
            trade = _pending_trade(
                order, buyer_id=DEMO_ACTIVITY_BUYER_ORG_ID,
                seller_id=DEMO_ACTIVITY_SELLER_ORG_ID, quantity="100",
            )
            trade.created_at = created_at
            trade.buyer_provenance = trade.seller_provenance = OrganizationProvenance.DEMO
            db.add(trade)
    if "cap" in prune_path:
        cap_name = "MAX_GENERATED_TRADES" if prune_path.startswith("linked") else "MAX_GENERATED_ORDERS"
        monkeypatch.setattr(demo_activity, cap_name, 0)
    await db.commit()

    result = await demo_activity.prune_demo_activity(db, now=now)
    await db.commit()

    assert result["orders_pruned"] == 1
    assert result["trades_pruned"] == (2 if prune_path.startswith("linked") else 0)
    assert (await db.execute(select(OrderBookOrder.id))).scalars().all() == [pinned.id]
    assert (await db.execute(select(WatchlistTarget.order_id))).scalars().all() == [pinned.id]


@pytest.mark.asyncio
@pytest.mark.parametrize("prune_path", ["age", "cap"])
async def test_demo_pruning_preserves_commission_rfq_and_negotiation_trade_history(monkeypatch, db, prune_path):
    now = datetime(2026, 9, 27, 0, tzinfo=UTC)
    created_at = now - timedelta(days=8 if prune_path == "age" else 1)
    for organization_id, kind in (
        (DEMO_ACTIVITY_BUYER_ORG_ID, OrgType.SHIPPING_LINE),
        (DEMO_ACTIVITY_SELLER_ORG_ID, OrgType.FUEL_SUPPLIER),
    ):
        db.add(Organization(
            id=organization_id, name=f"Demo {kind.value}", type=kind,
            provenance=OrganizationProvenance.DEMO, verification_status="APPROVED",
        ))
    product = await _make_product(db)
    port = await _make_delivery_point(db)
    orders, trades = [], []
    for _ in range(4):
        order = await _make_ask(
            db, org_id=DEMO_ACTIVITY_SELLER_ORG_ID, product_id=product.id, delivery_point_id=port.id,
        )
        order.provenance = OrganizationProvenance.DEMO
        order.created_at = created_at
        order.expires_at = now + timedelta(days=1)
        trade = _pending_trade(
            order, buyer_id=DEMO_ACTIVITY_BUYER_ORG_ID,
            seller_id=DEMO_ACTIVITY_SELLER_ORG_ID, quantity="100",
        )
        trade.buyer_provenance = trade.seller_provenance = OrganizationProvenance.DEMO
        trade.created_at = created_at
        db.add(trade)
        orders.append(order)
        trades.append(trade)
    await db.flush()
    commission = Commission(trade_id=trades[0].id, match_id=uuid4(), amount_usd=Decimal("200"))
    rfq_id, quote_id = uuid4(), uuid4()
    rfq = RFQ(
        id=rfq_id, buyer_org_id=DEMO_ACTIVITY_BUYER_ORG_ID, product_id=product.id,
        delivery_point_id=port.id, quantity_mt=Decimal("100"), availability_window="SPOT",
        status=RFQStatus.ACCEPTED, trade_id=trades[1].id, accepted_quote_id=quote_id,
        expires_at=now + timedelta(days=1),
    )
    quote = RFQQuote(
        id=quote_id, rfq_id=rfq_id, seller_org_id=DEMO_ACTIVITY_SELLER_ORG_ID,
        price_per_mt_usd=Decimal("1090"), status=QuoteStatus.ACCEPTED,
    )
    negotiation = Negotiation(
        initiator_org_id=DEMO_ACTIVITY_BUYER_ORG_ID, counterparty_org_id=DEMO_ACTIVITY_SELLER_ORG_ID,
        initiator_side="BUYER", last_actor_org_id=DEMO_ACTIVITY_BUYER_ORG_ID,
        product_id=product.id, delivery_point_id=port.id, availability_window="SPOT",
        quantity_mt=Decimal("100"), current_price=Decimal("1090"),
        status=NegotiationStatus.AGREED, trade_id=trades[2].id,
        expires_at=now + timedelta(days=1),
    )
    db.add_all([commission, rfq, quote, negotiation])
    if prune_path == "cap":
        monkeypatch.setattr(demo_activity, "MAX_GENERATED_TRADES", 0)
    await db.commit()

    result = await demo_activity.prune_demo_activity(db, now=now)
    await db.commit()

    assert result == {"trades_pruned": 1, "orders_pruned": 1}
    assert set((await db.execute(select(Trade.id))).scalars()) == {trade.id for trade in trades[:3]}
    assert set((await db.execute(select(OrderBookOrder.id))).scalars()) == {order.id for order in orders[:3]}
    assert (await db.execute(select(Commission.trade_id))).scalar_one() == trades[0].id
    assert (await db.execute(select(RFQ.trade_id))).scalar_one() == trades[1].id
    assert (await db.execute(select(Negotiation.trade_id))).scalar_one() == trades[2].id

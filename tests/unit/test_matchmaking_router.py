"""Tests for live matchmaking suggestions built from the user's own orders."""
from datetime import UTC, datetime
from decimal import Decimal
from uuid import uuid4

import pytest
from sqlalchemy import delete
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.market_catalog import DELIVERY_POINTS_BY_NAME, PRODUCTS_BY_NAME
from app.models.catalog import DeliveryPoint, Product
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import OrganizationProvenance, OrgType, Organization, User, UserRole, UserStatus
from app.routers.matchmaking import list_suggestions

REQUIRED_TABLES = [
    'organizations',
    'users',
    'products',
    'delivery_points',
    'orderbook_orders',
    'match_suggestions',
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
        for table in ('match_suggestions', 'orderbook_orders', 'users', 'delivery_points', 'products', 'organizations'):
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
    price: str,
    off_spec: bool = False,
    certification_declared: bool | None = None,
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
        certification_declared=(side == OrderSide.ASK) if certification_declared is None else certification_declared,
        certification_scheme='ISCC EU',
        specification_standard='IMPCA',
        msds_available=True,
        certifications=['ISCC EU'],
        off_spec=off_spec,
    )
    db.add(order)
    await db.flush()
    await db.refresh(order, ['product', 'delivery_point'])
    return order


@pytest.mark.asyncio
async def test_buyer_suggestions_come_from_own_bid_not_watchlist_entries(db: AsyncSession):
    buyer_org = await _make_org(db, 'Buyer', OrgType.SHIPPING_LINE)
    supplier_org = await _make_org(db, 'Supplier', OrgType.FUEL_SUPPLIER)
    buyer = await _make_user(db, buyer_org, UserRole.BUYER)
    singapore = await _make_delivery_point(db, 'Singapore')
    product = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')

    await _make_order(
        db,
        organization_id=buyer_org.id,
        product_id=product.id,
        delivery_point_id=singapore.id,
        side=OrderSide.BID,
        price='1100',
    )
    ask = await _make_order(
        db,
        organization_id=supplier_org.id,
        product_id=product.id,
        delivery_point_id=singapore.id,
        side=OrderSide.ASK,
        price='1080',
    )
    await db.commit()

    suggestions = await list_suggestions(db=db, current_user=buyer)

    assert len(suggestions) == 1
    assert suggestions[0]['ask_order_id'] == str(ask.id)
    assert 'market_product_match' in suggestions[0]['match_reasons']
    assert 'availability_match' in suggestions[0]['match_reasons']


@pytest.mark.asyncio
async def test_off_spec_candidates_are_excluded(db: AsyncSession):
    buyer_org = await _make_org(db, 'Buyer', OrgType.SHIPPING_LINE)
    supplier_org = await _make_org(db, 'Supplier', OrgType.FUEL_SUPPLIER)
    buyer = await _make_user(db, buyer_org, UserRole.BUYER)
    singapore = await _make_delivery_point(db, 'Singapore')
    product = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')

    await _make_order(
        db,
        organization_id=buyer_org.id,
        product_id=product.id,
        delivery_point_id=singapore.id,
        side=OrderSide.BID,
        price='1100',
    )
    await _make_order(
        db,
        organization_id=supplier_org.id,
        product_id=product.id,
        delivery_point_id=singapore.id,
        side=OrderSide.ASK,
        price='1080',
        off_spec=True,
    )
    await db.commit()

    suggestions = await list_suggestions(db=db, current_user=buyer)

    assert suggestions == []


@pytest.mark.asyncio
async def test_supplier_suggestions_allow_matching_bid_without_supplier_declaration(db: AsyncSession):
    supplier_org = await _make_org(db, 'Supplier', OrgType.FUEL_SUPPLIER)
    buyer_org = await _make_org(db, 'Buyer', OrgType.SHIPPING_LINE)
    supplier = await _make_user(db, supplier_org, UserRole.SUPPLIER)
    singapore = await _make_delivery_point(db, 'Singapore')
    product = await _make_product(db, name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio')

    await _make_order(
        db,
        organization_id=supplier_org.id,
        product_id=product.id,
        delivery_point_id=singapore.id,
        side=OrderSide.ASK,
        price='1080',
    )
    bid = await _make_order(
        db,
        organization_id=buyer_org.id,
        product_id=product.id,
        delivery_point_id=singapore.id,
        side=OrderSide.BID,
        price='1100',
        certification_declared=False,
    )
    await db.commit()

    suggestions = await list_suggestions(db=db, current_user=supplier)

    assert len(suggestions) == 1
    assert suggestions[0]['bid_order_id'] == str(bid.id)

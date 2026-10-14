"""Unit tests for the read-only market price resolver used by voyage planning.

The resolver must read the database exactly like the public order-book API,
batch every lookup into one query, and never expose an identity.
"""
from __future__ import annotations

from datetime import UTC, date, datetime, timedelta
from decimal import Decimal
from uuid import uuid4

import pytest
from sqlalchemy import delete, event
from sqlalchemy.exc import OperationalError
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.market_catalog import DELIVERY_POINTS_BY_NAME, PRODUCTS_BY_CODE
from app.models.benchmark import Benchmark
from app.models.catalog import DeliveryPoint, Product
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import Organization, OrganizationProvenance, OrgType, User
from app.schemas.market_activity import MarketDemoStatus
from app.schemas.voyage import PlannerFuel, PriceLookup, PriceSource
from app.services.voyage_prices import (
    MarketDataUnavailable,
    candidate_windows,
    market_slice_snapshot,
    resolve_product_prices,
)

REQUIRED_TABLES = [
    "organizations",
    "users",
    "products",
    "delivery_points",
    "orderbook_orders",
    "benchmarks",
]

NOW = datetime(2026, 10, 14, 12, 0, tzinfo=UTC)
TODAY = NOW.date()
DEPARTURE = date(2026, 11, 9)
MONTH_WINDOW = "2026-11"
QUARTER_WINDOW = "2026-Q4"


@pytest.fixture(scope="module")
def async_engine():
    return create_async_engine("sqlite+aiosqlite://", echo=False, future=True)


@pytest.fixture(scope="module")
async def setup_tables(async_engine):
    tables = [Base.metadata.tables[name] for name in REQUIRED_TABLES]
    from app.models.kyc_execution_grace import KYCExecutionGrace
    tables.append(KYCExecutionGrace.__table__)
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
        for table in reversed(REQUIRED_TABLES):
            await session.execute(delete(Base.metadata.tables[table]))
        await session.commit()
        yield session
        await session.rollback()
        for table in reversed(REQUIRED_TABLES):
            await session.execute(delete(Base.metadata.tables[table]))
        await session.commit()


async def _make_org(
    db: AsyncSession,
    provenance: OrganizationProvenance = OrganizationProvenance.REAL,
) -> Organization:
    org = Organization(
        name=f"Voyage test org {uuid4().hex[:8]}",
        type=OrgType.FUEL_SUPPLIER,
        provenance=provenance,
    )
    db.add(org)
    await db.flush()
    return org


async def _make_user(db: AsyncSession, org: Organization) -> User:
    user = User(
        email=f"voyage-{uuid4().hex[:8]}@example.com",
        password_hash="not-a-real-hash",
        organization_id=org.id,
    )
    db.add(user)
    await db.flush()
    return user


async def _make_product(db: AsyncSession, market_product: str = "BIO_METHANOL") -> Product:
    spec = PRODUCTS_BY_CODE[market_product]
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


async def _make_delivery_point(db: AsyncSession, name: str = "Singapore") -> DeliveryPoint:
    spec = DELIVERY_POINTS_BY_NAME[name]
    point = DeliveryPoint(id=spec.id, name=spec.name, region=spec.region, is_active=True)
    db.add(point)
    await db.flush()
    return point


def _make_order(
    *,
    org_id,
    product_id,
    delivery_point_id,
    price: str,
    window: str,
    side: OrderSide = OrderSide.ASK,
    provenance: OrganizationProvenance = OrganizationProvenance.REAL,
    carbon_intensity: Decimal | None = Decimal("18.50"),
    expires_at: datetime | None = None,
    owner_user_id=None,
) -> OrderBookOrder:
    order = OrderBookOrder(
        organization_id=org_id,
        provenance=provenance,
        side=side,
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        quantity_mt=Decimal("1000"),
        remaining_quantity_mt=Decimal("1000"),
        price_per_mt_usd=Decimal(price),
        availability_window=window,
        status=OrderBookStatus.OPEN,
        created_at=NOW - timedelta(hours=1),
        updated_at=NOW - timedelta(hours=1),
        expires_at=expires_at,
        owner_user_id=owner_user_id,
        off_spec=False,
        certification_declared=True,
        certification_scheme="ISCC EU",
    )
    if side == OrderSide.ASK:
        # The public ASK scope requires declared quality evidence.
        order.specification_standard = "ISO 8217"
        order.msds_available = True
        order.carbon_intensity_gco2_mj = carbon_intensity
        order.feedstock = "Waste biomass"
        order.origin = "Singapore"
    return order


def _lookup(fuel: str | PlannerFuel, port: str, departure: date = DEPARTURE) -> PriceLookup:
    return PriceLookup(fuel=PlannerFuel(fuel), port=port, departure_date=departure)


async def _seed_ask(
    db: AsyncSession,
    *,
    price: str,
    window: str,
    product: Product,
    point: DeliveryPoint,
    provenance: OrganizationProvenance = OrganizationProvenance.REAL,
    carbon_intensity: Decimal | None = Decimal("18.50"),
) -> OrderBookOrder:
    org = await _make_org(db, provenance=provenance)
    expires_at = NOW + timedelta(days=3) if provenance == OrganizationProvenance.DEMO else None
    order = _make_order(
        org_id=org.id,
        product_id=product.id,
        delivery_point_id=point.id,
        price=price,
        window=window,
        provenance=provenance,
        carbon_intensity=carbon_intensity,
        expires_at=expires_at,
    )
    db.add(order)
    await db.flush()
    return order


def test_candidate_windows_near_and_far():
    assert candidate_windows(TODAY + timedelta(days=10), today=TODAY) == (
        "SPOT",
        "2026-10",
        "2026-Q4",
    )
    assert candidate_windows(date(2027, 2, 3), today=TODAY) == ("2027-02", "2027-Q1")


async def test_lowest_real_ask_wins_and_is_executable(db: AsyncSession):
    product = await _make_product(db)
    point = await _make_delivery_point(db)
    await _seed_ask(db, price="1500", window=MONTH_WINDOW, product=product, point=point)
    await _seed_ask(db, price="1445", window=MONTH_WINDOW, product=product, point=point)
    await db.commit()

    lookup = _lookup("BIO_METHANOL", "Singapore")
    prices = await resolve_product_prices(db, [lookup], now=NOW)

    resolved = prices[lookup]
    assert resolved.price_usd_per_mt == Decimal("1445")
    assert resolved.source is PriceSource.LIVE_ORDER
    assert resolved.executable is True
    assert resolved.availability_window == MONTH_WINDOW
    assert resolved.observed_at == NOW


async def test_demo_ask_is_never_executable(db: AsyncSession):
    product = await _make_product(db)
    point = await _make_delivery_point(db)
    await _seed_ask(
        db,
        price="1445",
        window=MONTH_WINDOW,
        product=product,
        point=point,
        provenance=OrganizationProvenance.DEMO,
    )
    await db.commit()

    lookup = _lookup("BIO_METHANOL", "Singapore")
    prices = await resolve_product_prices(db, [lookup], now=NOW)

    resolved = prices[lookup]
    assert resolved.source is PriceSource.DEMO_ORDER
    assert resolved.executable is False


async def test_earlier_window_preferred(db: AsyncSession):
    product = await _make_product(db)
    point = await _make_delivery_point(db)
    await _seed_ask(db, price="1500", window=MONTH_WINDOW, product=product, point=point)
    await _seed_ask(db, price="1000", window=QUARTER_WINDOW, product=product, point=point)
    await db.commit()

    lookup = _lookup("BIO_METHANOL", "Singapore")
    prices = await resolve_product_prices(db, [lookup], now=NOW)

    resolved = prices[lookup]
    assert resolved.availability_window == MONTH_WINDOW
    assert resolved.price_usd_per_mt == Decimal("1500")


async def test_declared_intensity_is_copied(db: AsyncSession):
    product = await _make_product(db)
    point = await _make_delivery_point(db)
    await _seed_ask(
        db,
        price="1445",
        window=MONTH_WINDOW,
        product=product,
        point=point,
        carbon_intensity=Decimal("14"),
    )
    await db.commit()

    lookup = _lookup("BIO_METHANOL", "Singapore")
    prices = await resolve_product_prices(db, [lookup], now=NOW)

    assert prices[lookup].carbon_intensity_gco2e_per_mj == Decimal("14")


async def test_benchmark_fallback(db: AsyncSession):
    point = await _make_delivery_point(db)
    db.add(
        Benchmark(
            market_product="BIO_METHANOL",
            delivery_point_id=point.id,
            availability_window=MONTH_WINDOW,
            price_per_mt_usd=Decimal("1234.56"),
            source="manual_override",
        )
    )
    await db.commit()

    lookup = _lookup("BIO_METHANOL", "Singapore")
    prices = await resolve_product_prices(db, [lookup], now=NOW)

    resolved = prices[lookup]
    assert resolved.source is PriceSource.BENCHMARK
    assert resolved.executable is False
    assert resolved.price_usd_per_mt == Decimal("1234.56")


async def test_no_data_leaves_lookup_unresolved(db: AsyncSession):
    lookup = _lookup("UCOME_B100", "Singapore")
    prices = await resolve_product_prices(db, [lookup], now=NOW)

    assert lookup not in prices


async def test_single_query_for_many_lookups(db: AsyncSession, async_engine):
    lookups = [
        _lookup(fuel, port)
        for fuel in (
            "UCOME_B100",
            "BIO_METHANOL",
            "E_METHANOL",
            "BIO_ETHANOL",
            "SYNTHETIC_ETHANOL",
        )
        for port in ("Singapore", "Rotterdam")
    ]
    assert len(lookups) == 10
    order_statements: list[str] = []

    def _record(conn, cursor, statement, parameters, context, executemany):
        if "orderbook_orders" in statement:
            order_statements.append(statement)

    event.listen(async_engine.sync_engine, "before_cursor_execute", _record)
    try:
        await resolve_product_prices(db, lookups, now=NOW)
    finally:
        event.remove(async_engine.sync_engine, "before_cursor_execute", _record)

    assert len(order_statements) <= 2


async def test_snapshot_has_no_identifiers(db: AsyncSession):
    org = await _make_org(db)
    user = await _make_user(db, org)
    product = await _make_product(db)
    point = await _make_delivery_point(db)
    order = _make_order(
        org_id=org.id,
        product_id=product.id,
        delivery_point_id=point.id,
        price="1445",
        window=MONTH_WINDOW,
        owner_user_id=user.id,
    )
    db.add(order)
    await db.commit()

    snapshot = await market_slice_snapshot(
        db,
        market_product="BIO_METHANOL",
        delivery_point="Singapore",
        availability_window=MONTH_WINDOW,
        now=NOW,
    )

    rendered = snapshot.model_dump_json()
    for identifier in (org.id, user.id, order.id):
        assert str(identifier) not in rendered


async def test_snapshot_mixed_status(db: AsyncSession):
    product = await _make_product(db)
    point = await _make_delivery_point(db)
    await _seed_ask(db, price="1500", window=MONTH_WINDOW, product=product, point=point)
    await _seed_ask(
        db,
        price="1600",
        window=MONTH_WINDOW,
        product=product,
        point=point,
        provenance=OrganizationProvenance.DEMO,
    )
    await db.commit()

    snapshot = await market_slice_snapshot(
        db,
        market_product="BIO_METHANOL",
        delivery_point="Singapore",
        availability_window=MONTH_WINDOW,
        now=NOW,
    )

    assert snapshot.demo_status is MarketDemoStatus.MIXED
    assert snapshot.ask_order_count == 2
    assert snapshot.best_ask_usd_per_mt == Decimal("1500")
    assert snapshot.bid_order_count == 0
    assert snapshot.best_bid_usd_per_mt is None


async def test_snapshot_rejects_unknown_product_and_port(db: AsyncSession):
    with pytest.raises(ValueError):
        await market_slice_snapshot(
            db,
            market_product="VLSFO",
            delivery_point="Singapore",
            availability_window=MONTH_WINDOW,
            now=NOW,
        )
    with pytest.raises(ValueError) as port_error:
        await market_slice_snapshot(
            db,
            market_product="BIO_METHANOL",
            delivery_point="Hamburg",
            availability_window=MONTH_WINDOW,
            now=NOW,
        )
    assert "Singapore" in str(port_error.value)


async def test_database_error_is_wrapped(db: AsyncSession):
    class _FailingSession:
        async def execute(self, *args, **kwargs):
            raise OperationalError("SELECT 1", {}, Exception("database is down"))

    with pytest.raises(MarketDataUnavailable):
        await resolve_product_prices(
            _FailingSession(),
            [_lookup("BIO_METHANOL", "Singapore")],
            now=NOW,
        )

"""Tests for the canonical Forward Curve market-slice service."""
from __future__ import annotations

from datetime import UTC, datetime
from decimal import Decimal
from uuid import uuid4

import pytest
from fastapi import FastAPI
from httpx import ASGITransport, AsyncClient
from sqlalchemy import delete
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base, get_db
from app.models.catalog import DeliveryPoint, Product
from app.models.orderbook import Initiator, OrderBookOrder, OrderBookStatus, OrderSide, Trade, TradeStatus
from app.models.user import Organization, OrgType
from app.routers.curves import router
from app.schemas.curves import ForwardCurveEvidenceLayer, MarketSignalType
from app.schemas.market_activity import MarketDemoStatus, MarketSourceKind
from app.services.demo_market import DEMO_ACTIVITY_SELLER_ORG_ID
from app.services.forward_curve_market_slices import forward_curve_market_slices


REQUIRED_TABLES = [
    "organizations",
    "products",
    "delivery_points",
    "orderbook_orders",
    "trades",
    "benchmarks",
    "market_signal_ingestion_runs",
    "market_indications",
    "fair_price_bands",
    "physical_stems",
]


@pytest.fixture(scope="module")
def async_engine():
    return create_async_engine("sqlite+aiosqlite://", echo=False, future=True)


@pytest.fixture(scope="module")
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
        for table in reversed(REQUIRED_TABLES):
            await session.execute(delete(Base.metadata.tables[table]))
        await session.commit()
        yield session
        await session.rollback()
        for table in reversed(REQUIRED_TABLES):
            await session.execute(delete(Base.metadata.tables[table]))
        await session.commit()


async def _make_org(db: AsyncSession, name: str) -> Organization:
    org = Organization(name=name, type=OrgType.FUEL_SUPPLIER)
    db.add(org)
    await db.flush()
    return org


async def _make_demo_org(db: AsyncSession, org_id, name: str) -> Organization:
    org = Organization(id=org_id, name=name, type=OrgType.FUEL_SUPPLIER)
    db.add(org)
    await db.flush()
    return org


async def _make_product(db: AsyncSession, name: str, grade: str = "Bio") -> Product:
    product = Product(
        name=f"{name} {uuid4().hex[:8]}",
        fuel_type="Methanol",
        fuel_grade=grade,
        is_active=True,
    )
    db.add(product)
    await db.flush()
    return product


async def _make_delivery_point(db: AsyncSession, name: str, region: str = "Asia") -> DeliveryPoint:
    point = DeliveryPoint(name=name, region=region, is_active=True)
    db.add(point)
    await db.flush()
    return point


def _make_order(
    *,
    org_id,
    side: OrderSide,
    product_id,
    delivery_point_id,
    price: str,
    window: str = "SPOT",
) -> OrderBookOrder:
    payload = dict(
        organization_id=org_id,
        side=side,
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        quantity_mt=Decimal("1000"),
        remaining_quantity_mt=Decimal("1000"),
        price_per_mt_usd=Decimal(price),
        availability_window=window,
        status=OrderBookStatus.OPEN,
        created_at=datetime.now(UTC),
        certification_declared=True,
        certification_scheme="ISCC EU",
    )
    if side == OrderSide.ASK:
        payload.update(
            specification_standard="ISO 8217",
            msds_available=True,
            carbon_intensity_gco2_mj=Decimal("18.50"),
            feedstock="Waste biomass",
            origin="Singapore",
        )
    return OrderBookOrder(**payload)


@pytest.mark.asyncio
async def test_table_aggregates_products_by_canonical_market_product_and_approved_ports(db: AsyncSession):
    org = await _make_org(db, "Real Supplier")
    singapore = await _make_delivery_point(db, "Singapore")
    await _make_delivery_point(db, "Port Klang")
    bio_methanol_a = await _make_product(db, "Bio Methanol")
    bio_methanol_b = await _make_product(db, "Methanol Green", grade="Green")

    db.add_all(
        [
            _make_order(
                org_id=org.id,
                side=OrderSide.BID,
                product_id=bio_methanol_a.id,
                delivery_point_id=singapore.id,
                price="700",
            ),
            _make_order(
                org_id=org.id,
                side=OrderSide.ASK,
                product_id=bio_methanol_b.id,
                delivery_point_id=singapore.id,
                price="730",
            ),
            _make_order(
                org_id=org.id,
                side=OrderSide.BID,
                product_id=bio_methanol_b.id,
                delivery_point_id=singapore.id,
                price="680",
            ),
            _make_order(
                org_id=org.id,
                side=OrderSide.ASK,
                product_id=bio_methanol_a.id,
                delivery_point_id=singapore.id,
                price="760",
            ),
        ]
    )
    await db.commit()

    table = await forward_curve_market_slices.load_table(db, windows=["SPOT"])

    rows = [row for row in table.rows if row.market_product == "BIO_METHANOL"]
    assert len(rows) == 1
    row = rows[0]
    assert row.delivery_point_name == "Singapore"
    assert row.product_count == 2
    assert "Port Klang" not in {item.delivery_point_name for item in table.rows}
    cell = row.cells["SPOT"]
    assert cell.best_bid == Decimal("700.00")
    assert cell.best_ask == Decimal("730.00")
    assert cell.primary_value == Decimal("715.00")
    assert cell.primary_source_kind == MarketSourceKind.LIVE_ORDER


@pytest.mark.asyncio
async def test_one_sided_orderbook_does_not_create_primary_midpoint(db: AsyncSession):
    org = await _make_org(db, "Real Buyer")
    singapore = await _make_delivery_point(db, "Singapore")
    bio_methanol = await _make_product(db, "Bio Methanol")
    db.add(
        _make_order(
            org_id=org.id,
            side=OrderSide.BID,
            product_id=bio_methanol.id,
            delivery_point_id=singapore.id,
            price="700",
        )
    )
    await db.commit()

    table = await forward_curve_market_slices.load_table(db, windows=["SPOT"])
    cell = next(row for row in table.rows if row.market_product == "BIO_METHANOL").cells["SPOT"]

    assert cell.best_bid == Decimal("700.00")
    assert cell.best_ask is None
    assert cell.primary_value is None
    assert cell.primary_signal_type == MarketSignalType.NO_DATA


@pytest.mark.asyncio
async def test_mixed_demo_real_orderbook_does_not_publish_primary_midpoint(db: AsyncSession):
    real_org = await _make_org(db, "Real Buyer")
    demo_org = await _make_demo_org(db, DEMO_ACTIVITY_SELLER_ORG_ID, "Demo Seller")
    singapore = await _make_delivery_point(db, "Singapore")
    bio_methanol = await _make_product(db, "Bio Methanol")
    db.add_all(
        [
            _make_order(
                org_id=real_org.id,
                side=OrderSide.BID,
                product_id=bio_methanol.id,
                delivery_point_id=singapore.id,
                price="700",
            ),
            _make_order(
                org_id=demo_org.id,
                side=OrderSide.ASK,
                product_id=bio_methanol.id,
                delivery_point_id=singapore.id,
                price="730",
            ),
        ]
    )
    await db.commit()

    table = await forward_curve_market_slices.load_table(db, windows=["SPOT"])
    cell = next(row for row in table.rows if row.market_product == "BIO_METHANOL").cells["SPOT"]

    assert cell.best_bid == Decimal("700.00")
    assert cell.best_ask == Decimal("730.00")
    assert cell.primary_value is None
    assert cell.primary_signal_type == MarketSignalType.NO_DATA

    slice_response = await forward_curve_market_slices.load_slice(
        db,
        market_product="BIO_METHANOL",
        delivery_point_id=singapore.id,
        availability_window="SPOT",
    )
    bid_point = next(point for point in slice_response.evidence_points if point.layer == ForwardCurveEvidenceLayer.ORDERBOOK_BID)
    ask_point = next(point for point in slice_response.evidence_points if point.layer == ForwardCurveEvidenceLayer.ORDERBOOK_ASK)
    assert bid_point.source_kind == MarketSourceKind.LIVE_ORDER
    assert bid_point.demo_status == MarketDemoStatus.REAL_ONLY
    assert ask_point.source_kind == MarketSourceKind.DEMO_SEED
    assert ask_point.demo_status == MarketDemoStatus.DEMO_ONLY


@pytest.mark.asyncio
async def test_mixed_demo_real_trade_is_not_promoted_to_confirmed_trade_source(db: AsyncSession):
    real_buyer = await _make_org(db, "Real Buyer")
    demo_seller = await _make_demo_org(db, DEMO_ACTIVITY_SELLER_ORG_ID, "Demo Seller")
    singapore = await _make_delivery_point(db, "Singapore")
    bio_methanol = await _make_product(db, "Bio Methanol")
    ask = _make_order(
        org_id=demo_seller.id,
        side=OrderSide.ASK,
        product_id=bio_methanol.id,
        delivery_point_id=singapore.id,
        price="730",
    )
    db.add(ask)
    await db.flush()
    db.add(
        Trade(
            ask_order_id=ask.id,
            buyer_id=real_buyer.id,
            seller_id=demo_seller.id,
            initiated_by=Initiator.BUYER,
            quantity_mt=Decimal("500"),
            price_per_mt_usd=Decimal("730"),
            status=TradeStatus.CONFIRMED,
            confirmed_at=datetime.now(UTC),
        )
    )
    await db.commit()

    slice_response = await forward_curve_market_slices.load_slice(
        db,
        market_product="BIO_METHANOL",
        delivery_point_id=singapore.id,
        availability_window="SPOT",
    )

    assert slice_response.trades[0].source_kind == MarketSourceKind.MIXED_SOURCE
    assert slice_response.cell.primary_source_kind == MarketSourceKind.MIXED_SOURCE


@pytest.mark.asyncio
async def test_slice_endpoint_rejects_legacy_window_alias_before_db_use():
    app = FastAPI()
    app.include_router(router, prefix="/api/v1")

    async def mock_db():
        yield object()

    app.dependency_overrides[get_db] = mock_db
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        response = await client.get(
            f"/api/v1/curves/forward/slice?market_product=BIO_METHANOL&delivery_point_id={uuid4()}&availability_window=Q1_2026"
        )

    assert response.status_code == 422
    assert "canonical" in response.json()["detail"]


@pytest.mark.asyncio
async def test_too_many_table_windows_returns_422_before_db_use():
    app = FastAPI()
    app.include_router(router, prefix="/api/v1")

    async def mock_db():
        yield object()

    app.dependency_overrides[get_db] = mock_db

    windows = [f"2027-{month:02d}" for month in range(1, 13)] + [f"2028-{month:02d}" for month in range(1, 7)]
    query = "&".join(f"windows={window}" for window in windows)
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
        response = await client.get(f"/api/v1/curves/forward/table?{query}")

    assert response.status_code == 422
    assert "At most" in response.json()["detail"]


def _assert_no_forbidden_keys(value, forbidden: set[str]):
    if isinstance(value, dict):
        assert forbidden.isdisjoint(value.keys())
        for child in value.values():
            _assert_no_forbidden_keys(child, forbidden)
    elif isinstance(value, list):
        for child in value:
            _assert_no_forbidden_keys(child, forbidden)


@pytest.mark.asyncio
async def test_public_serialized_table_and_slice_redact_internal_feed_and_counterparty_fields(db: AsyncSession):
    org = await _make_org(db, "Real Supplier")
    singapore = await _make_delivery_point(db, "Singapore")
    bio_methanol = await _make_product(db, "Bio Methanol")
    db.add(
        _make_order(
            org_id=org.id,
            side=OrderSide.BID,
            product_id=bio_methanol.id,
            delivery_point_id=singapore.id,
            price="700",
        )
    )
    await db.commit()

    forbidden = {
        "organization_id",
        "buyer_id",
        "seller_id",
        "source_record_id",
        "source_event_id",
        "stem_uid",
        "model_name",
        "model_version",
        "trusted_ingestion_run_id",
    }
    table = await forward_curve_market_slices.load_table(db, windows=["SPOT"])
    slice_response = await forward_curve_market_slices.load_slice(
        db,
        market_product="BIO_METHANOL",
        delivery_point_id=singapore.id,
        availability_window="SPOT",
    )

    _assert_no_forbidden_keys(table.model_dump(mode="json"), forbidden)
    _assert_no_forbidden_keys(slice_response.model_dump(mode="json"), forbidden)

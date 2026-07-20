"""Tests for the canonical Forward Curve market-slice service."""
from __future__ import annotations

import json
from datetime import UTC, datetime, timedelta
from decimal import Decimal
from uuid import uuid4

import pytest
from fastapi import FastAPI
from httpx import ASGITransport, AsyncClient
from sqlalchemy import delete
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base, get_db
from app.market_catalog import DELIVERY_POINTS_BY_NAME, PRODUCTS_BY_NAME
from app.models.catalog import DeliveryPoint, Product
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import Organization, OrgType, OrganizationProvenance
from app.routers.curves import router
from app.schemas.curves import ForwardCurveEvidenceLayer, MarketSignalType
from app.schemas.market_activity import MarketDemoStatus, MarketSourceKind
from app.services.demo_market import DEMO_ACTIVITY_SELLER_ORG_ID, is_demo_market_organization
from app.services.forward_curve_market_slices import forward_curve_market_slices
from app.services.provenance import execution_provenance_compatible


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
    org = Organization(name=name, type=OrgType.FUEL_SUPPLIER, provenance=OrganizationProvenance.REAL)
    db.add(org)
    await db.flush()
    return org


async def _make_demo_org(db: AsyncSession, org_id, name: str) -> Organization:
    org = Organization(id=org_id, name=name, type=OrgType.FUEL_SUPPLIER, provenance=OrganizationProvenance.DEMO)
    db.add(org)
    await db.flush()
    return org


async def _make_product(
    db: AsyncSession,
    name: str,
    grade: str = "Bio",
    fuel_type: str = "Methanol",
) -> Product:
    spec = PRODUCTS_BY_NAME.get(name)
    product = Product(
        id=spec.id if spec is not None else uuid4(),
        name=name,
        fuel_type=fuel_type,
        fuel_grade=grade,
        is_active=True,
    )
    db.add(product)
    await db.flush()
    return product


async def _make_delivery_point(db: AsyncSession, name: str, region: str = "Asia") -> DeliveryPoint:
    spec = DELIVERY_POINTS_BY_NAME.get(name)
    point = DeliveryPoint(
        id=spec.id if spec is not None else uuid4(),
        name=name,
        region=spec.region if spec is not None else region,
        is_active=True,
    )
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
        provenance=(OrganizationProvenance.DEMO if is_demo_market_organization(org_id) else OrganizationProvenance.REAL),
        side=side,
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        quantity_mt=Decimal("1000"),
        remaining_quantity_mt=Decimal("1000"),
        price_per_mt_usd=Decimal(price),
        availability_window=window,
        status=OrderBookStatus.OPEN,
        created_at=datetime.now(UTC),
        expires_at=(
            datetime.now(UTC) + timedelta(days=1)
            if is_demo_market_organization(org_id)
            else None
        ),
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
async def test_table_uses_only_exact_active_canonical_products_and_approved_ports(db: AsyncSession):
    org = await _make_org(db, "Real Supplier")
    singapore = await _make_delivery_point(db, "Singapore")
    await _make_delivery_point(db, "Port Klang")
    bio_methanol_a = await _make_product(db, "Bio Methanol")
    legacy_alias = await _make_product(db, "Methanol Green", grade="Green")

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
                product_id=legacy_alias.id,
                delivery_point_id=singapore.id,
                price="730",
            ),
            _make_order(
                org_id=org.id,
                side=OrderSide.BID,
                product_id=legacy_alias.id,
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
    assert row.product_count == 1
    assert "Port Klang" not in {item.delivery_point_name for item in table.rows}
    cell = row.cells["SPOT"]
    assert cell.best_bid == Decimal("700.00")
    assert cell.best_ask == Decimal("760.00")
    assert cell.primary_value == Decimal("730.00")
    assert cell.primary_source_kind == MarketSourceKind.LIVE_ORDER


@pytest.mark.asyncio
async def test_table_cells_are_compact_and_filters_preserve_matrix_identity(db: AsyncSession):
    singapore = await _make_delivery_point(db, "Singapore")
    await _make_delivery_point(db, "Rotterdam")
    await _make_product(db, "Bio Methanol")
    await _make_product(db, "Bio Ethanol", fuel_type="Ethanol", grade="Bio")
    await db.commit()

    table = await forward_curve_market_slices.load_table(
        db,
        windows=["SPOT"],
        market_products=["BIO_METHANOL"],
        delivery_point_ids=[singapore.id],
    )

    assert len(table.rows) == 1
    row = table.rows[0]
    assert row.market_product == "BIO_METHANOL"
    assert row.delivery_point_id == singapore.id
    cell_payload = row.cells["SPOT"].model_dump(mode="json")
    assert {
        "market_product",
        "product_name",
        "representative_product_id",
        "delivery_point_id",
        "delivery_point_name",
        "region",
        "availability_window",
        "generated_at",
        "label_policy",
        "indication_summary",
        "fair_price_band",
        "fair_price_band_provenance",
        "physical_stem_summary",
    }.isdisjoint(cell_payload)


@pytest.mark.asyncio
async def test_default_four_by_eight_table_payload_is_bounded(db: AsyncSession):
    for name, fuel_type, grade in (
        ("Bio Methanol", "Methanol", "Bio"),
        ("e-Methanol", "Methanol", "E"),
        ("Bio Ethanol", "Ethanol", "Bio"),
        ("Synthetic Ethanol", "Ethanol", "Synthetic"),
    ):
        await _make_product(db, name, fuel_type=fuel_type, grade=grade)
    for name, region in (
        ("Dalian", "Asia"),
        ("Busan", "Asia"),
        ("Shanghai", "Asia"),
        ("Singapore", "Asia"),
        ("Rotterdam", "Europe"),
        ("Houston", "Americas"),
        ("Los Angeles", "Americas"),
        ("Santos", "Americas"),
    ):
        await _make_delivery_point(db, name, region)
    await db.commit()

    table = await forward_curve_market_slices.load_table(db)
    encoded = json.dumps(
        table.model_dump(mode="json"),
        separators=(",", ":"),
    ).encode()

    assert len(table.rows) == 32
    assert len(table.columns) <= 16
    assert len(encoded) < 250_000, len(encoded)


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
async def test_real_orderbook_headline_never_pairs_with_demo_ask(db: AsyncSession):
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
    assert cell.best_ask is None
    assert cell.real_best_bid == Decimal("700.00")
    assert cell.demo_best_ask == Decimal("730.00")
    assert cell.demo_status == MarketDemoStatus.REAL_ONLY
    assert cell.primary_value is None
    assert cell.primary_signal_type == MarketSignalType.NO_DATA

    slice_response = await forward_curve_market_slices.load_slice(
        db,
        market_product="BIO_METHANOL",
        delivery_point_id=singapore.id,
        availability_window="SPOT",
    )
    bid_point = next(point for point in slice_response.evidence_points if point.layer == ForwardCurveEvidenceLayer.ORDERBOOK_BID)
    assert bid_point.source_kind == MarketSourceKind.LIVE_ORDER
    assert bid_point.demo_status == MarketDemoStatus.REAL_ONLY
    assert not any(
        point.layer == ForwardCurveEvidenceLayer.ORDERBOOK_ASK
        for point in slice_response.evidence_points
    )


@pytest.mark.asyncio
async def test_demo_only_orderbook_remains_visible_and_explicitly_demo(db: AsyncSession):
    demo_org = await _make_demo_org(db, DEMO_ACTIVITY_SELLER_ORG_ID, "Demo Supplier")
    singapore = await _make_delivery_point(db, "Singapore")
    product = await _make_product(db, "Bio Methanol")
    db.add_all(
        [
            _make_order(
                org_id=demo_org.id,
                side=OrderSide.BID,
                product_id=product.id,
                delivery_point_id=singapore.id,
                price="700",
            ),
            _make_order(
                org_id=demo_org.id,
                side=OrderSide.ASK,
                product_id=product.id,
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
    assert cell.primary_value == Decimal("715.00")
    assert cell.primary_source_kind == MarketSourceKind.DEMO_SEED
    assert cell.demo_status == MarketDemoStatus.DEMO_ONLY


def test_mixed_demo_real_trade_snapshot_fails_closed_in_execution_policy():
    assert not execution_provenance_compatible(
        OrganizationProvenance.REAL,
        OrganizationProvenance.DEMO,
    )


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


@pytest.mark.asyncio
async def test_table_wire_projection_is_filterable_and_bounded_for_full_matrix(db: AsyncSession):
    for name, fuel_type, grade in (
        ("Bio Methanol", "Methanol", "Bio"),
        ("e-Methanol", "Methanol", "E"),
        ("Bio Ethanol", "Ethanol", "Bio"),
        ("Synthetic Ethanol", "Ethanol", "Synthetic"),
    ):
        await _make_product(db, name, grade=grade, fuel_type=fuel_type)
    for name, region in (
        ("Dalian", "Asia"),
        ("Busan", "Asia"),
        ("Shanghai", "Asia"),
        ("Singapore", "Asia"),
        ("Rotterdam", "Europe"),
        ("Houston", "North America"),
        ("Los Angeles", "North America"),
        ("Santos", "South America"),
    ):
        await _make_delivery_point(db, name, region)
    await db.commit()

    table = await forward_curve_market_slices.load_table(db)
    payload = table.model_dump(mode="json")
    encoded = json.dumps(payload, separators=(",", ":")).encode()

    assert len(table.rows) == 32
    assert 1 <= len(table.columns) <= 16
    assert all(len(row.cells) == len(table.columns) for row in table.rows)
    assert len(encoded) < 350_000
    for row in payload["rows"]:
        for cell in row["cells"].values():
            assert "real_best_bid" not in cell
            assert "demo_best_ask" not in cell
            assert "benchmark_mid" not in cell

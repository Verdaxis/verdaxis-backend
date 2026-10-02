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
from app.models.benchmark import Benchmark
from app.models.catalog import DeliveryPoint, Product
from app.models.orderbook import Initiator, OrderBookOrder, OrderBookStatus, OrderSide, Trade, TradeStatus
from app.models.user import Organization, OrgType, OrganizationProvenance
from app.routers.curves import router
from app.schemas.curves import (
    ForwardCurveBoardFairPriceBand,
    ForwardCurveBoardIndicationSummary,
    ForwardCurveEvidenceLayer,
    ForwardCurveMarketCell,
    ForwardCurveSignalProvenance,
    ForwardCurveSignalSourceKind,
    ForwardCurveTableCell,
    MarketSignalType,
)
from app.schemas.market_activity import MarketDemoStatus, MarketSourceKind
from app.services.demo_market import DEMO_ACTIVITY_BUYER_ORG_ID, DEMO_ACTIVITY_SELLER_ORG_ID, is_demo_market_organization
from app.services.forward_curve_market_slices import (
    ProductGroup,
    SliceKey,
    ViewerContext,
    _no_data_indication_summary,
    _no_data_stem_summary,
    forward_curve_market_slices,
)
from app.services.provenance import execution_provenance_compatible


REQUIRED_TABLES = [
    "organizations",
    "users",
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
    if product_id == PRODUCTS_BY_NAME["UCOME B100"].id:
        from app.services.fame_order import validate_fame_order_terms
        terms = {
            "side": side.value, "neat_fame": True,
            "standard": "EN_14214", "standard_edition": "2012+A2:2019",
            "sustainability_scheme": "ISCC_EU", "evidence_due": "BEFORE_LOADING",
        }
        if side == OrderSide.ASK:
            terms.update(
                uco_mass_pct=100, certificate_reference="curve-cert",
                certificate_holder="Curve supplier",
                certificate_valid_until=str((datetime.now(UTC) + timedelta(days=365)).date()),
                evidence_status="PENDING", batch_reference="batch", producing_site="site",
            )
        payload["fame_terms"] = validate_fame_order_terms(product_id, side, terms)
    return OrderBookOrder(**payload)


@pytest.mark.asyncio
@pytest.mark.parametrize("sides", [(OrderSide.BID,), (OrderSide.ASK,), (OrderSide.BID, OrderSide.ASK)])
@pytest.mark.parametrize("include_demo", [False, True])
async def test_orderbook_aggregate_combines_sides_and_product_ids_without_blending_evidence(
    db: AsyncSession, sides, include_demo,
):
    real_org = await _make_org(db, "Real Supplier")
    demo_org = await _make_demo_org(db, DEMO_ACTIVITY_SELLER_ORG_ID, "Demo Supplier")
    point = await _make_delivery_point(db, "Singapore")
    product = await _make_product(db, "Bio Methanol")
    # Exercise the service's multi-product group fold without changing the
    # separate canonical catalog admission rules used by its public callers.
    second_product = Product(
        id=uuid4(), name="Second group member", fuel_type="Methanol", fuel_grade="Bio", is_active=True,
    )
    db.add(second_product)
    await db.flush()
    now = datetime.now(UTC)
    for provenance, base_price in (
        (OrganizationProvenance.REAL, 700),
        (OrganizationProvenance.DEMO, 900),
        (OrganizationProvenance.UNKNOWN, 2000),
    ):
        for index, product_id in enumerate((product.id, second_product.id)):
            for side in sides:
                price = base_price + (index * 10 if side == OrderSide.BID else 60 - index * 10)
                order = _make_order(
                    org_id=demo_org.id if provenance == OrganizationProvenance.DEMO else real_org.id,
                    side=side, product_id=product_id, delivery_point_id=point.id, price=str(price),
                )
                order.provenance = provenance
                order.updated_at = now - timedelta(minutes=index)
                if provenance == OrganizationProvenance.DEMO and index == 0:
                    order.idempotency_operation = "DEMO_COVERAGE"
                db.add(order)
    expired = _make_order(
        org_id=real_org.id, side=OrderSide.BID, product_id=product.id,
        delivery_point_id=point.id, price="9999",
    )
    expired.expires_at = now - timedelta(days=1)
    db.add(expired)
    await db.commit()
    group = ProductGroup("BIO_METHANOL", product.name, product.id, (product.id, second_product.id))
    key = SliceKey("BIO_METHANOL", point.id, "SPOT")

    buckets = await forward_curve_market_slices._load_orderbook(
        db, [key], {group.market_product: group}, ViewerContext(include_demo=include_demo),
    )

    assert set(buckets) == {key}
    bucket = buckets[key]
    assert bucket["real_order_count"] == 2 * len(sides)
    assert bucket["real_volume_mt"] == Decimal("2000") * len(sides)
    assert bucket["real_best_bid"] == (Decimal("710") if OrderSide.BID in sides else None)
    assert bucket["real_best_ask"] == (Decimal("750") if OrderSide.ASK in sides else None)
    assert bucket["real_last_order_at"].replace(tzinfo=UTC) == now
    assert bucket["demo_order_count"] == (2 * len(sides) if include_demo else 0)
    assert bucket["unknown_order_count"] == (2 * len(sides) if include_demo else 0)
    assert bucket["managed_demo_order_count"] == (len(sides) if include_demo else 0)
    assert bucket["demo_volume_mt"] == (Decimal("2000") * len(sides) if include_demo else Decimal("0"))
    assert bucket["demo_best_bid"] == (Decimal("910") if include_demo and OrderSide.BID in sides else None)
    assert bucket["demo_best_ask"] == (Decimal("950") if include_demo and OrderSide.ASK in sides else None)
    if include_demo:
        assert bucket["demo_last_order_at"].replace(tzinfo=UTC) == now


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
    assert len(table.columns) <= 24
    assert len(encoded) < 500_000, len(encoded)


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


@pytest.fixture
async def managed_demo_slice(db: AsyncSession):
    buyer = await _make_demo_org(db, DEMO_ACTIVITY_BUYER_ORG_ID, "Demo Buyer")
    seller = await _make_demo_org(db, DEMO_ACTIVITY_SELLER_ORG_ID, "Demo Seller")
    product = await _make_product(db, "Bio Methanol")
    point = await _make_delivery_point(db, "Singapore")
    observed_at = datetime.now(UTC) - timedelta(minutes=2)
    bid = _make_order(org_id=buyer.id, side=OrderSide.BID, product_id=product.id,
                      delivery_point_id=point.id, price="700")
    ask = _make_order(org_id=seller.id, side=OrderSide.ASK, product_id=product.id,
                      delivery_point_id=point.id, price="730")
    for order in (bid, ask):
        order.idempotency_operation = "DEMO_COVERAGE"
        order.updated_at = observed_at
    confirmed_at = observed_at - timedelta(days=1)
    trade = Trade(
        buyer_id=buyer.id, seller_id=seller.id, initiator_org_id=buyer.id,
        buyer_provenance=OrganizationProvenance.DEMO,
        seller_provenance=OrganizationProvenance.DEMO,
        initiated_by=Initiator.BUYER,
        product_id=product.id, product_name=product.name,
        fuel_type=product.fuel_type, fuel_grade=product.fuel_grade,
        market_product="BIO_METHANOL",
        delivery_point_id=point.id, delivery_point_name=point.name,
        delivery_point_region=point.region, availability_window="SPOT",
        market_snapshot_version=1,
        quantity_mt=Decimal("100"), price_per_mt_usd=Decimal("990"),
        status=TradeStatus.CONFIRMED,
        created_at=confirmed_at - timedelta(minutes=1), confirmed_at=confirmed_at,
    )
    db.add_all([bid, ask, trade])
    await db.commit()
    return product, point, bid, ask, trade


@pytest.mark.asyncio
async def test_managed_demo_book_is_primary_without_rewriting_trade_history(db, managed_demo_slice):
    _product, point, bid, _ask, trade = managed_demo_slice
    table = await forward_curve_market_slices.load_table(db, windows=["SPOT"])
    table_cell = next(row for row in table.rows if row.market_product == "BIO_METHANOL").cells["SPOT"]
    response = await forward_curve_market_slices.load_slice(
        db, market_product="BIO_METHANOL", delivery_point_id=point.id, availability_window="SPOT",
    )
    for cell in (table_cell, response.cell):
        assert cell.primary_value == Decimal("715.00")
        assert cell.primary_signal_type == MarketSignalType.ORDERBOOK_BID
        assert cell.primary_source_kind == MarketSourceKind.DEMO_SEED
        assert cell.public_source_label == "Demo orderbook midpoint"
        assert cell.demo_status == MarketDemoStatus.DEMO_ONLY
        assert cell.observed_at == bid.updated_at
        assert cell.is_executable is False
    assert len(response.trades) == 1
    assert response.trades[0].price_per_mt_usd == trade.price_per_mt_usd
    assert response.trades[0].confirmed_at == trade.confirmed_at
    prints = [item for item in response.evidence_points if item.layer == ForwardCurveEvidenceLayer.HISTORICAL_TRADE]
    assert len(prints) == 1
    assert prints[0].price_per_mt_usd == trade.price_per_mt_usd
    assert prints[0].observed_at == trade.confirmed_at
    await db.refresh(trade)
    assert trade.price_per_mt_usd == Decimal("990")


@pytest.mark.asyncio
async def test_real_trade_keeps_precedence_over_managed_demo_book(db, managed_demo_slice):
    _product, point, _bid, _ask, trade = managed_demo_slice
    buyer = await _make_org(db, "Real Trade Buyer")
    seller = await _make_org(db, "Real Trade Seller")
    trade.buyer_id, trade.seller_id = buyer.id, seller.id
    trade.buyer_provenance = trade.seller_provenance = OrganizationProvenance.REAL
    await db.commit()
    response = await forward_curve_market_slices.load_slice(
        db, market_product="BIO_METHANOL", delivery_point_id=point.id, availability_window="SPOT",
    )
    assert response.cell.primary_value == Decimal("990.00")
    assert response.cell.primary_signal_type == MarketSignalType.CONFIRMED_TRADE
    assert response.cell.primary_source_kind == MarketSourceKind.CONFIRMED_TRADE
    assert response.cell.demo_status == MarketDemoStatus.REAL_ONLY
    assert response.cell.observed_at == trade.confirmed_at
    assert response.trades[0].source_kind == MarketSourceKind.CONFIRMED_TRADE


@pytest.mark.asyncio
async def test_table_projection_preserves_fields_without_dumping_full_cells(db, managed_demo_slice, monkeypatch):
    _product, point, _bid, _ask, _trade = managed_demo_slice
    response = await forward_curve_market_slices.load_slice(
        db, market_product="BIO_METHANOL", delivery_point_id=point.id, availability_window="SPOT",
    )
    expected = ForwardCurveTableCell.model_validate(response.cell.model_dump())

    def reject_full_cell(*_args, **_kwargs):
        raise AssertionError("Table projection must not construct full market cells")

    monkeypatch.setattr(ForwardCurveMarketCell, "__init__", reject_full_cell)
    table = await forward_curve_market_slices.load_table(db, windows=["SPOT"])
    actual = next(row for row in table.rows if row.market_product == "BIO_METHANOL").cells["SPOT"]
    # Also compare fields excluded from the wire payload, which policy tests use.
    assert actual.__dict__ == expected.__dict__


@pytest.mark.parametrize(
    "evidence_case",
    ["no_data", "reference", "indication", "fair_band", "live", "demo", "formal_print"],
)
def test_direct_table_cell_exactly_matches_original_full_cell_projection(evidence_case):
    generated_at = datetime(2026, 10, 2, 6, tzinfo=UTC)
    product_id = uuid4()
    point = DeliveryPoint(id=uuid4(), name="Singapore", region="Asia", is_active=True)
    group = ProductGroup("BIO_METHANOL", "Bio Methanol", product_id, (product_id,))
    key = SliceKey(group.market_product, point.id, "SPOT")
    order_bucket = {}
    trade = None
    benchmark = None
    indication = _no_data_indication_summary()
    fair_band = None

    if evidence_case == "reference":
        benchmark = Benchmark(
            market_product=group.market_product,
            delivery_point_id=point.id,
            availability_window="SPOT",
            price_per_mt_usd=Decimal("725.50"),
            source="manual_override",
            created_at=generated_at - timedelta(days=1),
            updated_at=generated_at - timedelta(hours=1),
        )
    elif evidence_case == "live":
        order_bucket = {
            "real_best_bid": Decimal("700"),
            "real_best_ask": Decimal("730"),
            "real_volume_mt": Decimal("2000"),
            "real_order_count": 2,
            "real_last_order_at": generated_at - timedelta(minutes=5),
        }
    elif evidence_case == "demo":
        order_bucket = {
            "demo_best_bid": Decimal("690"),
            "demo_best_ask": Decimal("720"),
            "demo_volume_mt": Decimal("2000"),
            "demo_order_count": 2,
            "managed_demo_order_count": 2,
            "demo_last_order_at": generated_at - timedelta(minutes=10),
        }
    elif evidence_case == "formal_print":
        trade = {
            "price_per_mt_usd": Decimal("740"),
            "confirmed_at": generated_at - timedelta(minutes=15),
            "demo_status": MarketDemoStatus.REAL_ONLY,
            "source_kind": MarketSourceKind.CONFIRMED_TRADE,
        }
    elif evidence_case == "indication":
        indication = ForwardCurveBoardIndicationSummary(
            provenance=ForwardCurveSignalProvenance(
                signal_type=MarketSignalType.MARKET_INDICATION,
                signal_source_kind=ForwardCurveSignalSourceKind.MARKET_INDICATION,
                demo_status=MarketDemoStatus.DEMO_ONLY,
                observed_at=generated_at - timedelta(minutes=20),
                generated_at=generated_at,
                demo_count=1,
            ),
            latest_mid_price_per_mt_usd=Decimal("718.50"),
            indication_count=1,
        )
    elif evidence_case == "fair_band":
        fair_band = ForwardCurveBoardFairPriceBand(
            low_price_per_mt_usd=Decimal("710"),
            mid_price_per_mt_usd=Decimal("720"),
            high_price_per_mt_usd=Decimal("730"),
            provenance=ForwardCurveSignalProvenance(
                signal_type=MarketSignalType.FAIR_PRICE_BAND,
                signal_source_kind=ForwardCurveSignalSourceKind.FAIR_PRICE_MODEL,
                demo_status=MarketDemoStatus.REAL_ONLY,
                observed_at=generated_at - timedelta(minutes=25),
                generated_at=generated_at,
                real_count=1,
            ),
        )

    full_cell = forward_curve_market_slices._build_cell(
        key=key,
        group=group,
        point=point,
        order_bucket=order_bucket,
        trade=trade,
        benchmark=benchmark,
        indication_summary=indication,
        fair_price_band=fair_band,
        physical_stem_summary=_no_data_stem_summary(),
        generated_at=generated_at,
    )
    original_projection = ForwardCurveTableCell.model_validate(full_cell, from_attributes=True)

    direct_projection = forward_curve_market_slices._build_table_cell(
        order_bucket=order_bucket,
        trade=trade,
        benchmark=benchmark,
        indication_summary=indication,
        fair_price_band=fair_band,
        generated_at=generated_at,
    )

    assert direct_projection.__dict__ == original_projection.__dict__


def test_direct_table_cell_skips_detail_only_label_policy_models(monkeypatch):
    def reject_policy(*_args, **_kwargs):
        raise AssertionError("Compact table cells must not build label-policy models")

    monkeypatch.setattr(
        "app.services.forward_curve_market_slices._label_policy",
        reject_policy,
    )
    monkeypatch.setattr(
        "app.services.forward_curve_market_slices.no_data_label_policy",
        reject_policy,
    )
    generated_at = datetime(2026, 10, 2, 6, tzinfo=UTC)
    indication = _no_data_indication_summary()

    no_data = forward_curve_market_slices._build_table_cell(
        order_bucket={},
        trade=None,
        benchmark=None,
        indication_summary=indication,
        fair_price_band=None,
        generated_at=generated_at,
    )
    live = forward_curve_market_slices._build_table_cell(
        order_bucket={
            "real_best_bid": Decimal("700"),
            "real_best_ask": Decimal("730"),
            "real_order_count": 2,
            "real_last_order_at": generated_at,
        },
        trade=None,
        benchmark=None,
        indication_summary=indication,
        fair_price_band=None,
        generated_at=generated_at,
    )

    assert no_data.primary_signal_type == MarketSignalType.NO_DATA
    assert live.primary_value == Decimal("715.00")


@pytest.mark.asyncio
async def test_table_skips_physical_summaries_but_full_cells_load_them(db, monkeypatch):
    product = await _make_product(db, "Bio Methanol")
    point = await _make_delivery_point(db, "Singapore")
    await db.commit()
    calls = []

    async def track_physical_summaries(_db, signal_keys):
        calls.append(signal_keys)
        return {}

    monkeypatch.setattr(
        "app.services.forward_curve_market_slices.load_physical_stem_summaries",
        track_physical_summaries,
    )
    await forward_curve_market_slices.load_table(db, windows=["SPOT"])
    assert calls == []

    group = ProductGroup("BIO_METHANOL", product.name, product.id, (product.id,))
    key = SliceKey(group.market_product, point.id, "SPOT")
    cells = await forward_curve_market_slices.load_many(
        db,
        [key],
        product_groups=[group],
        delivery_points=[point],
        generated_at=datetime.now(UTC),
    )

    assert calls == [[(group.market_product, point.id, "SPOT")]]
    assert isinstance(cells[key], ForwardCurveMarketCell)


@pytest.mark.asyncio
@pytest.mark.parametrize("real_count", [0, 1, 9])
async def test_slice_trade_history_reads_once_and_selects_real_before_limit(
    db, managed_demo_slice, monkeypatch, real_count,
):
    product, point, _bid, _ask, demo_trade = managed_demo_slice
    real_buyer = await _make_org(db, "History Buyer")
    real_seller = await _make_org(db, "History Seller")
    now = datetime.now(UTC)
    for scope, count in (("REAL", real_count), ("DEMO", 9), ("MIXED", 9)):
        for index in range(count):
            real_buyer_scope = scope in {"REAL", "MIXED"}
            buyer_id = real_buyer.id if real_buyer_scope else demo_trade.buyer_id
            confirmed_at = now - timedelta(days=1 if scope == "REAL" else 0, minutes=index)
            db.add(Trade(
                buyer_id=buyer_id,
                seller_id=real_seller.id if scope == "REAL" else demo_trade.seller_id,
                initiator_org_id=buyer_id,
                buyer_provenance=OrganizationProvenance.REAL if real_buyer_scope else OrganizationProvenance.DEMO,
                seller_provenance=OrganizationProvenance.REAL if scope == "REAL" else OrganizationProvenance.DEMO,
                initiated_by=Initiator.BUYER,
                product_id=product.id, product_name=product.name,
                fuel_type=product.fuel_type, fuel_grade=product.fuel_grade,
                market_product="BIO_METHANOL",
                delivery_point_id=point.id, delivery_point_name=point.name,
                delivery_point_region=point.region, availability_window="SPOT",
                market_snapshot_version=1,
                quantity_mt=Decimal("100"), price_per_mt_usd=Decimal("700") + index,
                status=TradeStatus.CONFIRMED,
                created_at=confirmed_at - timedelta(minutes=1), confirmed_at=confirmed_at,
            ))
    await db.commit()
    group = next(
        group for group in await forward_curve_market_slices.load_product_groups(db)
        if group.market_product == "BIO_METHANOL"
    )
    execute = db.execute
    query_count = 0

    async def count_query(*args, **kwargs):
        nonlocal query_count
        query_count += 1
        return await execute(*args, **kwargs)

    monkeypatch.setattr(db, "execute", count_query)
    result = await forward_curve_market_slices._load_slice_trades(db, group=group, point=point, window="SPOT")

    assert query_count == 1
    expected_count = min(real_count, 8) if real_count else 8
    assert len(result) == expected_count
    expected_status = MarketDemoStatus.REAL_ONLY if real_count else MarketDemoStatus.DEMO_ONLY
    assert all(item.demo_status == expected_status for item in result)
    assert [item.price_per_mt_usd for item in result] == [Decimal("700") + index for index in range(expected_count)]


@pytest.mark.asyncio
@pytest.mark.parametrize("book_case", [
    "unmanaged", "unmanaged_extra", "one_managed_side", "one_sided", "expired_ask", "real_book", "unknown_book",
])
async def test_demo_trade_precedence_is_unchanged_outside_managed_two_sided_book(db, managed_demo_slice, book_case):
    product, point, bid, ask, trade = managed_demo_slice
    if book_case == "unmanaged":
        bid.idempotency_operation = ask.idempotency_operation = None
    elif book_case == "unmanaged_extra":
        db.add(_make_order(org_id=bid.organization_id, side=OrderSide.BID, product_id=product.id,
                           delivery_point_id=point.id, price="690"))
    elif book_case == "one_managed_side":
        ask.idempotency_operation = None
    elif book_case == "one_sided":
        ask.status = OrderBookStatus.CANCELLED
    elif book_case == "expired_ask":
        ask.expires_at = datetime.now(UTC) - timedelta(minutes=1)
    elif book_case == "real_book":
        real_org = await _make_org(db, "Real Book")
        for side, price in ((OrderSide.BID, "650"), (OrderSide.ASK, "660")):
            order = _make_order(org_id=real_org.id, side=side, product_id=product.id,
                                delivery_point_id=point.id, price=price)
            order.idempotency_operation = "DEMO_COVERAGE"
            db.add(order)
    elif book_case == "unknown_book":
        bid.provenance = ask.provenance = OrganizationProvenance.UNKNOWN
    await db.commit()
    response = await forward_curve_market_slices.load_slice(
        db, market_product="BIO_METHANOL", delivery_point_id=point.id, availability_window="SPOT",
    )
    assert response.cell.primary_value == Decimal("990.00")
    assert response.cell.primary_signal_type == MarketSignalType.CONFIRMED_TRADE
    assert response.cell.observed_at == trade.confirmed_at
    assert response.trades[0].price_per_mt_usd == trade.price_per_mt_usd
    if book_case == "real_book":
        assert response.cell.best_bid == Decimal("650.00")
        assert response.cell.best_ask == Decimal("660.00")
        assert all(level.demo_status == MarketDemoStatus.REAL_ONLY for level in response.depth_bids + response.depth_asks)
    if book_case == "unknown_book":
        assert response.cell.demo_order_count == 0
        assert not response.depth_bids and not response.depth_asks


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

    windows = [f"{year}-{month:02d}" for year in (2027, 2028) for month in range(1, 13)] + ["2029-01"]
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
    assert 1 <= len(table.columns) <= 24
    assert all(len(row.cells) == len(table.columns) for row in table.rows)
    assert len(encoded) < 500_000
    for row in payload["rows"]:
        for cell in row["cells"].values():
            assert "real_best_bid" not in cell
            assert "demo_best_ask" not in cell
            assert "benchmark_mid" not in cell


@pytest.mark.asyncio
async def test_b100_curve_is_visible_without_prices_and_only_on_approved_lane(db: AsyncSession):
    await _make_product(db, "UCOME B100", fuel_type="FAME", grade="UCOME")
    singapore = await _make_delivery_point(db, "Singapore")
    await _make_delivery_point(db, "Rotterdam")
    table = await forward_curve_market_slices.load_table(
        db, market_products=["UCOME_B100"], windows=["SPOT"],
    )
    assert [(row.market_product, row.delivery_point_id) for row in table.rows] == [
        ("UCOME_B100", singapore.id),
    ]
    cell = table.rows[0].cells["SPOT"]
    assert cell.primary_value is None
    assert cell.is_executable is False
    assert cell.order_count == 0
    detail = await forward_curve_market_slices.load_slice(
        db, market_product="UCOME_B100", delivery_point_id=singapore.id, availability_window="SPOT",
    )
    assert detail.cell.primary_value is None
    assert "specification" in detail.cell.label_policy.disclaimer.lower()


@pytest.mark.asyncio
async def test_b100_curve_refuses_unsupported_delivery_lane(db: AsyncSession):
    await _make_product(db, "UCOME B100", fuel_type="FAME", grade="UCOME")
    rotterdam = await _make_delivery_point(db, "Rotterdam")
    with pytest.raises(ValueError, match="not available"):
        await forward_curve_market_slices.load_slice(
            db, market_product="UCOME_B100", delivery_point_id=rotterdam.id, availability_window="SPOT",
        )


@pytest.mark.asyncio
async def test_b100_product_midpoint_does_not_claim_specification_compatible_execution(db: AsyncSession):
    org = await _make_org(db, "B100 curve supplier")
    product = await _make_product(db, "UCOME B100", fuel_type="FAME", grade="UCOME")
    singapore = await _make_delivery_point(db, "Singapore")
    db.add_all([
        _make_order(org_id=org.id, side=side, product_id=product.id,
                    delivery_point_id=singapore.id, price=price)
        for side, price in [(OrderSide.BID, "1000"), (OrderSide.ASK, "1100")]
    ])
    await db.commit()
    detail = await forward_curve_market_slices.load_slice(
        db, market_product="UCOME_B100", delivery_point_id=singapore.id, availability_window="SPOT",
    )
    assert detail.cell.primary_value == Decimal("1050.00")
    assert detail.cell.public_source_label == "Orderbook midpoint"
    assert detail.cell.is_executable is False
    assert detail.cell.is_reference is True
    assert "specifications" in detail.cell.label_policy.disclaimer


@pytest.mark.asyncio
async def test_b100_expired_operator_declaration_cannot_set_curve_price_or_depth(db: AsyncSession):
    org = await _make_org(db, "B100 expired supplier")
    product = await _make_product(db, "UCOME B100", fuel_type="FAME", grade="UCOME")
    singapore = await _make_delivery_point(db, "Singapore")
    bid = _make_order(org_id=org.id, side=OrderSide.BID, product_id=product.id,
                      delivery_point_id=singapore.id, price="1000")
    ask = _make_order(org_id=org.id, side=OrderSide.ASK, product_id=product.id,
                      delivery_point_id=singapore.id, price="1100")
    ask.fame_terms = {**ask.fame_terms, "certificate_valid_until": "2020-01-01"}
    db.add_all([bid, ask])
    await db.commit()
    detail = await forward_curve_market_slices.load_slice(
        db, market_product="UCOME_B100", delivery_point_id=singapore.id, availability_window="SPOT",
    )
    assert detail.cell.best_ask is None
    assert detail.cell.primary_value is None
    assert detail.cell.order_count == 1
    assert detail.depth_asks == []

"""Tests for Forward Curve monitoring signal loaders."""
from __future__ import annotations

from datetime import datetime, timedelta, timezone
from decimal import Decimal
from uuid import uuid4

import pytest
from sqlalchemy.exc import IntegrityError
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.models.catalog import DeliveryPoint
from app.models.forward_monitoring import (
    FairPriceBand,
    MarketIndication,
    MarketSignalIngestionRun,
    PhysicalStem,
)
from app.schemas.curves import ForwardCurveSignalSourceKind
from app.schemas.market_activity import MarketDemoStatus
from app.services.forward_monitoring import (
    load_fair_price_bands,
    load_indication_summaries,
    load_physical_stem_summaries,
)


REQUIRED_TABLES = [
    "delivery_points",
    "market_signal_ingestion_runs",
    "market_indications",
    "fair_price_bands",
    "physical_stems",
]


@pytest.fixture
async def db():
    engine = create_async_engine("sqlite+aiosqlite://", echo=False, future=True)
    tables = [Base.metadata.tables[name] for name in REQUIRED_TABLES]
    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all, tables=tables)
    session_factory = async_sessionmaker(
        bind=engine,
        class_=AsyncSession,
        expire_on_commit=False,
        autoflush=False,
    )
    async with session_factory() as session:
        yield session
    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.drop_all, tables=tables)
    await engine.dispose()


async def _delivery_point(db: AsyncSession) -> DeliveryPoint:
    point = DeliveryPoint(
        id=uuid4(),
        name=f"Singapore-{uuid4()}",
        region="Asia",
        timezone="Asia/Singapore",
        is_active=True,
    )
    db.add(point)
    await db.commit()
    return point


def _now() -> datetime:
    return datetime.now(timezone.utc)


@pytest.mark.asyncio
async def test_market_indication_created_at_defaults_in_sqlite(db: AsyncSession):
    point = await _delivery_point(db)
    indication = MarketIndication(
        market_product="BIO_METHANOL",
        delivery_point_id=point.id,
        availability_window="SPOT",
        side="BID",
        price_per_mt_usd=Decimal("700.00"),
        source="demo_seed",
        source_event_id="ind-created-at",
        observed_at=_now(),
    )
    db.add(indication)
    await db.commit()
    await db.refresh(indication)

    assert indication.created_at is not None


@pytest.mark.asyncio
async def test_invalid_market_product_is_rejected(db: AsyncSession):
    point = await _delivery_point(db)
    db.add(
        MarketIndication(
            market_product="METHANOL",
            delivery_point_id=point.id,
            availability_window="SPOT",
            side="BID",
            price_per_mt_usd=Decimal("700.00"),
            source="demo_seed",
            observed_at=_now(),
        )
    )

    with pytest.raises(IntegrityError):
        await db.commit()


@pytest.mark.asyncio
async def test_indication_summary_uses_latest_per_side_and_repeated_record_ids(db: AsyncSession):
    point = await _delivery_point(db)
    key = ("BIO_METHANOL", point.id, "SPOT")
    old_time = _now() - timedelta(hours=2)
    new_time = _now()
    db.add_all(
        [
            MarketIndication(
                market_product=key[0],
                delivery_point_id=key[1],
                availability_window=key[2],
                side="BID",
                price_per_mt_usd=Decimal("690.00"),
                quantity_mt=Decimal("1000.00"),
                source="demo_seed",
                source_record_id="same-record",
                source_event_id="ind-old",
                observed_at=old_time,
            ),
            MarketIndication(
                market_product=key[0],
                delivery_point_id=key[1],
                availability_window=key[2],
                side="BID",
                price_per_mt_usd=Decimal("705.00"),
                quantity_mt=Decimal("1200.00"),
                source="demo_seed",
                source_record_id="same-record",
                source_event_id="ind-new",
                observed_at=new_time,
            ),
            MarketIndication(
                market_product=key[0],
                delivery_point_id=key[1],
                availability_window=key[2],
                side="ASK",
                price_per_mt_usd=Decimal("715.00"),
                quantity_mt=Decimal("1300.00"),
                source="demo_seed",
                source_record_id="ask-record",
                source_event_id="ind-ask",
                observed_at=new_time,
            ),
        ]
    )
    await db.commit()

    summary = (await load_indication_summaries(db, [key]))[key]

    assert summary.latest_bid_price_per_mt_usd == Decimal("705.00")
    assert summary.latest_ask_price_per_mt_usd == Decimal("715.00")
    assert summary.total_quantity_mt == Decimal("2500.00")
    assert summary.indication_count == 2
    assert summary.provenance.signal_source_kind == ForwardCurveSignalSourceKind.DEMO_SEED
    assert summary.provenance.demo_status == MarketDemoStatus.DEMO_ONLY


@pytest.mark.asyncio
async def test_duplicate_source_event_id_is_rejected(db: AsyncSession):
    point = await _delivery_point(db)
    common = dict(
        market_product="BIO_METHANOL",
        delivery_point_id=point.id,
        availability_window="SPOT",
        side="BID",
        price_per_mt_usd=Decimal("700.00"),
        source="demo_seed",
        source_event_id="duplicate-event",
    )
    db.add_all(
        [
            MarketIndication(**common, observed_at=_now() - timedelta(minutes=2)),
            MarketIndication(**common, observed_at=_now()),
        ]
    )

    with pytest.raises(IntegrityError):
        await db.commit()


@pytest.mark.asyncio
async def test_physical_stem_latest_state_removes_cancelled_or_allocated_rows(db: AsyncSession):
    point = await _delivery_point(db)
    key = ("BIO_METHANOL", point.id, "SPOT")
    db.add_all(
        [
            PhysicalStem(
                market_product=key[0],
                delivery_point_id=key[1],
                availability_window=key[2],
                quantity_mt=Decimal("1000.00"),
                status="AVAILABLE",
                source="demo_seed",
                stem_uid="stem-cancelled",
                source_event_id="stem-cancelled-old",
                observed_at=_now() - timedelta(hours=2),
            ),
            PhysicalStem(
                market_product=key[0],
                delivery_point_id=key[1],
                availability_window=key[2],
                quantity_mt=Decimal("1000.00"),
                status="CANCELLED",
                source="demo_seed",
                stem_uid="stem-cancelled",
                source_event_id="stem-cancelled-new",
                observed_at=_now() - timedelta(hours=1),
            ),
            PhysicalStem(
                market_product=key[0],
                delivery_point_id=key[1],
                availability_window=key[2],
                quantity_mt=Decimal("500.00"),
                status="AVAILABLE",
                source="demo_seed",
                stem_uid="stem-allocated",
                source_event_id="stem-allocated-old",
                observed_at=_now() - timedelta(hours=2),
            ),
            PhysicalStem(
                market_product=key[0],
                delivery_point_id=key[1],
                availability_window=key[2],
                quantity_mt=Decimal("500.00"),
                status="ALLOCATED",
                source="demo_seed",
                stem_uid="stem-allocated",
                source_event_id="stem-allocated-new",
                observed_at=_now() - timedelta(minutes=30),
            ),
            PhysicalStem(
                market_product=key[0],
                delivery_point_id=key[1],
                availability_window=key[2],
                quantity_mt=Decimal("2200.00"),
                status="AVAILABLE",
                source="demo_seed",
                stem_uid="stem-updated",
                source_record_id="repeat-record",
                source_event_id="stem-updated-old",
                observed_at=_now() - timedelta(hours=2),
            ),
            PhysicalStem(
                market_product=key[0],
                delivery_point_id=key[1],
                availability_window=key[2],
                quantity_mt=Decimal("2400.00"),
                status="AVAILABLE",
                source="demo_seed",
                stem_uid="stem-updated",
                source_record_id="repeat-record",
                source_event_id="stem-updated-new",
                observed_at=_now(),
            ),
        ]
    )
    await db.commit()

    summary = (await load_physical_stem_summaries(db, [key]))[key]

    assert summary.available_quantity_mt == Decimal("2400.00")
    assert summary.stem_count == 1
    assert summary.provenance.demo_status == MarketDemoStatus.DEMO_ONLY


@pytest.mark.asyncio
async def test_forged_manual_real_row_downgrades_without_matching_trusted_run(db: AsyncSession):
    point = await _delivery_point(db)
    key = ("BIO_METHANOL", point.id, "SPOT")
    wrong_run = MarketSignalIngestionRun(
        signal_family="PHYSICAL_STEM",
        source="manual_feed",
        source_kind="PHYSICAL_STEM",
        verified_at=_now(),
    )
    db.add(wrong_run)
    await db.flush()
    db.add(
        MarketIndication(
            market_product=key[0],
            delivery_point_id=key[1],
            availability_window=key[2],
            side="BID",
            price_per_mt_usd=Decimal("705.00"),
            source="manual_feed",
            source_event_id="forged-real",
            trusted_ingestion_run_id=wrong_run.id,
            is_demo=False,
            is_verified_real=True,
            verified_real_at=_now(),
            observed_at=_now(),
        )
    )
    await db.commit()

    summary = (await load_indication_summaries(db, [key]))[key]

    assert summary.provenance.signal_source_kind == ForwardCurveSignalSourceKind.UNKNOWN
    assert summary.provenance.demo_status == MarketDemoStatus.UNKNOWN
    assert summary.provenance.unknown_count == 1


@pytest.mark.asyncio
async def test_trusted_ingestion_run_allows_real_indication(db: AsyncSession):
    point = await _delivery_point(db)
    key = ("BIO_METHANOL", point.id, "SPOT")
    trusted_run = MarketSignalIngestionRun(
        signal_family="MARKET_INDICATION",
        source="broker_feed",
        source_kind="MARKET_INDICATION",
        verified_at=_now(),
    )
    db.add(trusted_run)
    await db.flush()
    db.add(
        MarketIndication(
            market_product=key[0],
            delivery_point_id=key[1],
            availability_window=key[2],
            side="BID",
            price_per_mt_usd=Decimal("705.00"),
            source="broker_feed",
            source_event_id="trusted-real",
            trusted_ingestion_run_id=trusted_run.id,
            is_demo=False,
            is_verified_real=True,
            verified_real_at=_now(),
            observed_at=_now(),
        )
    )
    await db.commit()

    summary = (await load_indication_summaries(db, [key]))[key]

    assert summary.provenance.signal_source_kind == ForwardCurveSignalSourceKind.MARKET_INDICATION
    assert summary.provenance.demo_status == MarketDemoStatus.REAL_ONLY
    assert summary.provenance.real_count == 1


@pytest.mark.asyncio
async def test_fair_price_band_lookback_bounds_append_history(db: AsyncSession):
    point = await _delivery_point(db)
    key = ("BIO_METHANOL", point.id, "SPOT")
    db.add(
        FairPriceBand(
            market_product=key[0],
            delivery_point_id=key[1],
            availability_window=key[2],
            low_price_per_mt_usd=Decimal("680.00"),
            mid_price_per_mt_usd=Decimal("700.00"),
            high_price_per_mt_usd=Decimal("720.00"),
            model_name="internal-model",
            source="demo_seed",
            source_event_id="old-band",
            observed_at=_now() - timedelta(days=40),
        )
    )
    await db.commit()

    assert await load_fair_price_bands(db, [key], lookback_days=30) == {}


def test_openapi_contract_keeps_signal_sources_separate_from_market_sources():
    from app.main import app

    schemas = app.openapi()["components"]["schemas"]
    provenance = schemas["ForwardCurveSignalProvenance"]
    props = provenance["properties"]

    assert "signal_source_kind" in props
    assert props["signal_source_kind"]["$ref"].endswith("/ForwardCurveSignalSourceKind")
    assert "generated_at" in provenance["required"]
    assert "detail" not in props
    assert "display_label" not in props
    assert schemas["ForwardCurveSignalSourceKind"]["enum"] == [
        "MARKET_INDICATION",
        "PHYSICAL_STEM",
        "FAIR_PRICE_MODEL",
        "DEMO_SEED",
        "MIXED_SOURCE",
        "NO_DATA",
        "UNKNOWN",
    ]
    assert schemas["MarketSourceKind"]["enum"] == [
        "CONFIRMED_TRADE",
        "LIVE_ORDER",
        "DEMO_SEED",
        "BENCHMARK_REFERENCE",
        "MIXED_SOURCE",
        "NO_DATA",
        "UNKNOWN",
    ]
    cell_props = schemas["ForwardCurveBoardCell"]["properties"]
    assert cell_props["best_bid_source_kind"]["$ref"].endswith("/MarketSourceKind")
    assert cell_props["fair_price_band_provenance"]["$ref"].endswith("/ForwardCurveSignalProvenance")

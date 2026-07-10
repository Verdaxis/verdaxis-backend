"""Tests for the trusted market-signal ingestion service.

The service under test (`app.services.market_signal_ingestion`) turns operator-
supplied CSV rows into verified ingestion runs whose rows classify as REAL
through the trust predicate in `app.services.forward_monitoring`. These tests
are written first (red) per PLAN-real-signal-ingestion Task 1.
"""
from __future__ import annotations

from datetime import datetime, timedelta, timezone
from decimal import Decimal
from uuid import uuid4

import pytest
from sqlalchemy import func, select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.models.catalog import DeliveryPoint
from app.models.forward_monitoring import (
    FairPriceBand,
    MarketIndication,
    MarketSignalIngestionRun,
)
from app.schemas.curves import ForwardCurveSignalSourceKind
from app.schemas.market_activity import MarketDemoStatus
from app.services.forward_monitoring import (
    load_fair_price_bands,
    load_indication_summaries,
    load_physical_stem_summaries,
)
from app.services.market_signal_ingestion import ingest_signals


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


async def _delivery_point(db: AsyncSession, name: str = "Singapore") -> DeliveryPoint:
    point = DeliveryPoint(
        id=uuid4(),
        name=name,
        region="Asia",
        timezone="Asia/Singapore",
        is_active=True,
    )
    db.add(point)
    await db.commit()
    return point


def _now() -> datetime:
    return datetime.now(timezone.utc)


def _indication_row(point: DeliveryPoint, *, event_id: str, **overrides) -> dict:
    row = {
        "source_event_id": event_id,
        "market_product": "BIO_METHANOL",
        "delivery_point": point.name,
        "availability_window": "SPOT",
        "side": "BID",
        "price_per_mt_usd": "1031.50",
        "quantity_mt": "500",
        "observed_at": _now().isoformat(),
    }
    row.update(overrides)
    return row


def _band_row(point: DeliveryPoint, *, event_id: str, **overrides) -> dict:
    row = {
        "source_event_id": event_id,
        "market_product": "BIO_METHANOL",
        "delivery_point": point.name,
        "availability_window": "SPOT",
        "low_price_per_mt_usd": "980",
        "mid_price_per_mt_usd": "1030",
        "high_price_per_mt_usd": "1080",
        "model_name": "desk-fair-value",
        "model_version": "1",
        "observed_at": _now().isoformat(),
    }
    row.update(overrides)
    return row


def _stem_row(point: DeliveryPoint, *, event_id: str, **overrides) -> dict:
    row = {
        "source_event_id": event_id,
        "stem_uid": "stem-a",
        "market_product": "BIO_METHANOL",
        "delivery_point": point.name,
        "availability_window": "SPOT",
        "status": "AVAILABLE",
        "quantity_mt": "1200",
        "stem_start": _now().isoformat(),
        "stem_end": (_now() + timedelta(days=5)).isoformat(),
        "observed_at": _now().isoformat(),
    }
    row.update(overrides)
    return row


async def _count(db: AsyncSession, model) -> int:
    return (await db.execute(select(func.count()).select_from(model))).scalar_one()


# --- 1. Happy path indications -------------------------------------------------


@pytest.mark.asyncio
async def test_happy_path_indications_creates_verified_run_and_real_rows(db: AsyncSession):
    point = await _delivery_point(db)
    rows = [
        _indication_row(point, event_id="ev-1"),
        _indication_row(point, event_id="ev-2", side="ASK", price_per_mt_usd="1054.00"),
    ]

    report = await ingest_signals(
        db, family="MARKET_INDICATION", source="broker-sheet-a", rows=rows, dry_run=False
    )

    assert report.inserted == 2
    assert report.skipped_duplicates == 0
    assert report.errors == []

    run = (await db.execute(select(MarketSignalIngestionRun))).scalar_one()
    assert run.signal_family == "MARKET_INDICATION"
    assert run.source_kind == "MARKET_INDICATION"
    assert run.source == "broker-sheet-a"
    assert run.verified_at is not None
    assert report.created_run_id == run.id

    indications = (await db.execute(select(MarketIndication))).scalars().all()
    assert len(indications) == 2
    for row in indications:
        assert row.is_demo is False
        assert row.is_verified_real is True
        assert row.verified_real_at is not None
        assert row.trusted_ingestion_run_id == run.id
        assert row.source == "broker-sheet-a"


# --- 2. Trust round-trip (the test that matters most) --------------------------


@pytest.mark.asyncio
async def test_trust_round_trip_indications_classify_real(db: AsyncSession):
    point = await _delivery_point(db)
    await ingest_signals(
        db,
        family="MARKET_INDICATION",
        source="broker-sheet-a",
        rows=[_indication_row(point, event_id="ev-rt-1")],
        dry_run=False,
    )
    await db.commit()

    key = ("BIO_METHANOL", point.id, "SPOT")
    summaries = await load_indication_summaries(db, [key])
    provenance = summaries[key].provenance
    assert provenance.demo_status == MarketDemoStatus.REAL_ONLY
    assert provenance.signal_source_kind == ForwardCurveSignalSourceKind.MARKET_INDICATION
    assert provenance.real_count == 1
    assert provenance.demo_count == 0


@pytest.mark.asyncio
async def test_trust_round_trip_fair_bands_classify_real(db: AsyncSession):
    point = await _delivery_point(db)
    await ingest_signals(
        db,
        family="FAIR_PRICE_BAND",
        source="desk-model",
        rows=[_band_row(point, event_id="band-rt-1")],
        dry_run=False,
    )
    await db.commit()

    key = ("BIO_METHANOL", point.id, "SPOT")
    bands = await load_fair_price_bands(db, [key])
    provenance = bands[key].provenance
    assert provenance.demo_status == MarketDemoStatus.REAL_ONLY
    assert provenance.signal_source_kind == ForwardCurveSignalSourceKind.FAIR_PRICE_MODEL


@pytest.mark.asyncio
async def test_trust_round_trip_stems_classify_real(db: AsyncSession):
    point = await _delivery_point(db)
    await ingest_signals(
        db,
        family="PHYSICAL_STEM",
        source="terminal-ops",
        rows=[_stem_row(point, event_id="stem-rt-1")],
        dry_run=False,
    )
    await db.commit()

    key = ("BIO_METHANOL", point.id, "SPOT")
    stems = await load_physical_stem_summaries(db, [key])
    provenance = stems[key].provenance
    assert provenance.demo_status == MarketDemoStatus.REAL_ONLY
    assert provenance.signal_source_kind == ForwardCurveSignalSourceKind.PHYSICAL_STEM


# --- 3. Fair bands: source_kind mapping and per-row rejection ------------------


@pytest.mark.asyncio
async def test_fair_band_run_uses_fair_price_model_source_kind(db: AsyncSession):
    point = await _delivery_point(db)
    report = await ingest_signals(
        db,
        family="FAIR_PRICE_BAND",
        source="desk-model",
        rows=[_band_row(point, event_id="band-1")],
        dry_run=False,
    )
    assert report.inserted == 1

    run = (await db.execute(select(MarketSignalIngestionRun))).scalar_one()
    # The family/source_kind enums differ for bands; FAIR_PRICE_BAND runs must
    # carry source_kind FAIR_PRICE_MODEL or every row is silently untrusted.
    assert run.signal_family == "FAIR_PRICE_BAND"
    assert run.source_kind == "FAIR_PRICE_MODEL"


@pytest.mark.asyncio
async def test_fair_band_low_mid_high_violation_rejects_row_not_run(db: AsyncSession):
    point = await _delivery_point(db)
    rows = [
        _band_row(point, event_id="band-ok"),
        _band_row(
            point,
            event_id="band-bad",
            low_price_per_mt_usd="1100",
            mid_price_per_mt_usd="1030",
            high_price_per_mt_usd="1080",
        ),
    ]
    report = await ingest_signals(
        db, family="FAIR_PRICE_BAND", source="desk-model", rows=rows, dry_run=False
    )

    assert report.inserted == 1
    assert len(report.errors) == 1
    assert report.errors[0].row_number == 2
    run = (await db.execute(select(MarketSignalIngestionRun))).scalar_one()
    assert run.verified_at is not None
    assert await _count(db, FairPriceBand) == 1


# --- 4. Stems: latest-state semantics -------------------------------------------


@pytest.mark.asyncio
async def test_stem_update_row_reduces_available_quantity(db: AsyncSession):
    point = await _delivery_point(db)
    first = _stem_row(point, event_id="stem-1")
    report = await ingest_signals(
        db, family="PHYSICAL_STEM", source="terminal-ops", rows=[first], dry_run=False
    )
    assert report.inserted == 1
    run = (
        await db.execute(
            select(MarketSignalIngestionRun).where(
                MarketSignalIngestionRun.id == report.created_run_id
            )
        )
    ).scalar_one()
    assert run.source_kind == "PHYSICAL_STEM"
    await db.commit()

    key = ("BIO_METHANOL", point.id, "SPOT")
    before = await load_physical_stem_summaries(db, [key])
    assert before[key].available_quantity_mt == Decimal("1200.00")

    update = _stem_row(
        point,
        event_id="stem-2",
        status="ALLOCATED",
        observed_at=(_now() + timedelta(minutes=5)).isoformat(),
    )
    report2 = await ingest_signals(
        db, family="PHYSICAL_STEM", source="terminal-ops", rows=[update], dry_run=False
    )
    assert report2.inserted == 1
    await db.commit()

    after = await load_physical_stem_summaries(db, [key])
    # Latest state per (source, stem_uid) is ALLOCATED -> no available quantity.
    assert not after[key].available_quantity_mt


# --- 5. Idempotency --------------------------------------------------------------


@pytest.mark.asyncio
async def test_reingesting_same_file_skips_all_rows_and_rolls_back_empty_run(db: AsyncSession):
    point = await _delivery_point(db)
    rows = [
        _indication_row(point, event_id="dup-1"),
        _indication_row(point, event_id="dup-2", side="ASK"),
    ]
    first = await ingest_signals(
        db, family="MARKET_INDICATION", source="broker-sheet-a", rows=rows, dry_run=False
    )
    assert first.inserted == 2
    await db.commit()

    second = await ingest_signals(
        db, family="MARKET_INDICATION", source="broker-sheet-a", rows=rows, dry_run=False
    )
    assert second.inserted == 0
    assert second.skipped_duplicates == 2
    # A verified run with zero rows is a forgery primitive: it must not persist.
    assert second.created_run_id is None
    await db.rollback()

    assert await _count(db, MarketIndication) == 2
    assert await _count(db, MarketSignalIngestionRun) == 1


# --- 6. Validation rejections ----------------------------------------------------


@pytest.mark.asyncio
async def test_validation_rejections_report_row_numbers_and_reasons(db: AsyncSession):
    point = await _delivery_point(db)
    rows = [
        _indication_row(point, event_id="ok-1"),
        _indication_row(point, event_id="bad-product", market_product="VLSFO"),
        _indication_row(point, event_id="bad-point", delivery_point="Atlantis"),
        _indication_row(point, event_id="bad-window", availability_window="M+1"),
        _indication_row(
            point, event_id="naive-ts", observed_at=_now().replace(tzinfo=None).isoformat()
        ),
        _indication_row(
            point,
            event_id="future-ts",
            observed_at=(_now() + timedelta(hours=1)).isoformat(),
        ),
        _indication_row(point, event_id="bad-price", price_per_mt_usd="-10"),
        _indication_row(point, event_id="bad-qty", quantity_mt="0"),
        _indication_row(point, event_id="bad-side", side="BUY"),
        _indication_row(point, event_id=""),
    ]
    report = await ingest_signals(
        db, family="MARKET_INDICATION", source="broker-sheet-a", rows=rows, dry_run=False
    )

    assert report.inserted == 1
    error_rows = {error.row_number for error in report.errors}
    assert error_rows == {2, 3, 4, 5, 6, 7, 8, 9, 10}
    for error in report.errors:
        assert error.reason


@pytest.mark.asyncio
async def test_stem_status_outside_enum_rejected(db: AsyncSession):
    point = await _delivery_point(db)
    report = await ingest_signals(
        db,
        family="PHYSICAL_STEM",
        source="terminal-ops",
        rows=[_stem_row(point, event_id="stem-bad", status="SOLD")],
        dry_run=False,
    )
    assert report.inserted == 0
    assert len(report.errors) == 1
    assert report.created_run_id is None


# --- 7. Staleness warnings --------------------------------------------------------


@pytest.mark.asyncio
async def test_stale_rows_insert_with_warning(db: AsyncSession):
    point = await _delivery_point(db)
    stale = _indication_row(
        point,
        event_id="stale-1",
        observed_at=(_now() - timedelta(days=10)).isoformat(),
    )
    report = await ingest_signals(
        db, family="MARKET_INDICATION", source="broker-sheet-a", rows=[stale], dry_run=False
    )

    # Older than the 7-day indication lookback: inserted but invisible on the
    # board, so the operator must be warned rather than left debugging.
    assert report.inserted == 1
    assert report.stale_warnings == 1


# --- 8. Dry run --------------------------------------------------------------------


@pytest.mark.asyncio
async def test_dry_run_validates_without_writing(db: AsyncSession):
    point = await _delivery_point(db)
    rows = [
        _indication_row(point, event_id="dry-1"),
        _indication_row(point, event_id="bad", price_per_mt_usd="not-a-number"),
    ]
    report = await ingest_signals(
        db, family="MARKET_INDICATION", source="broker-sheet-a", rows=rows, dry_run=True
    )

    assert report.inserted == 0
    assert report.would_insert == 1
    assert len(report.errors) == 1
    assert await _count(db, MarketIndication) == 0
    assert await _count(db, MarketSignalIngestionRun) == 0


# --- 9. Mixed-family rejection ------------------------------------------------------


@pytest.mark.asyncio
async def test_indication_rows_handed_to_band_family_fail_fast(db: AsyncSession):
    point = await _delivery_point(db)
    report = await ingest_signals(
        db,
        family="FAIR_PRICE_BAND",
        source="desk-model",
        rows=[_indication_row(point, event_id="wrong-family")],
        dry_run=False,
    )

    assert report.inserted == 0
    assert report.errors
    assert report.created_run_id is None
    assert await _count(db, FairPriceBand) == 0
    assert await _count(db, MarketSignalIngestionRun) == 0


@pytest.mark.asyncio
async def test_unknown_family_rejected():
    with pytest.raises(ValueError):
        await ingest_signals(
            None, family="ORDERBOOK_BID", source="broker-sheet-a", rows=[], dry_run=True
        )

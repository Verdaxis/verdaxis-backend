"""Batched loaders for Forward Curve read-only monitoring signals."""
from __future__ import annotations

from collections.abc import Iterable
from datetime import datetime, timedelta, timezone
from decimal import Decimal
from typing import TypeVar
from uuid import UUID

from sqlalchemy import Select, func, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.forward_monitoring import (
    FairPriceBand,
    MarketIndication,
    MarketSignalIngestionRun,
    PhysicalStem,
)
from app.schemas.curves import (
    ForwardCurveBoardFairPriceBand,
    ForwardCurveBoardIndication,
    ForwardCurveBoardIndicationSummary,
    ForwardCurveBoardPhysicalStem,
    ForwardCurveBoardPhysicalStemSummary,
    ForwardCurveIndicationSide,
    ForwardCurvePhysicalStemStatus,
    ForwardCurveSignalProvenance,
    ForwardCurveSignalSourceKind,
    MarketSignalType,
    no_data_signal_provenance,
)
from app.schemas.market_activity import MarketDemoStatus, MarketScope


SignalKey = tuple[str, UUID, str]

_T = TypeVar("_T")

_SIGNAL_TO_REAL_SOURCE_KIND = {
    MarketSignalType.MARKET_INDICATION: ForwardCurveSignalSourceKind.MARKET_INDICATION,
    MarketSignalType.FAIR_PRICE_BAND: ForwardCurveSignalSourceKind.FAIR_PRICE_MODEL,
    MarketSignalType.PHYSICAL_STEM: ForwardCurveSignalSourceKind.PHYSICAL_STEM,
}


def normalize_signal_keys(keys: Iterable[SignalKey]) -> list[SignalKey]:
    """Deduplicate signal keys while preserving input order."""

    normalized: list[SignalKey] = []
    seen: set[SignalKey] = set()
    for market_product, delivery_point_id, availability_window in keys:
        key = (str(market_product), delivery_point_id, str(availability_window))
        if key not in seen:
            seen.add(key)
            normalized.append(key)
    return normalized


def no_data_summary_for_signal(signal_type: MarketSignalType):
    if signal_type == MarketSignalType.MARKET_INDICATION:
        return ForwardCurveBoardIndicationSummary(
            provenance=no_data_signal_provenance(MarketSignalType.MARKET_INDICATION)
        )
    if signal_type == MarketSignalType.PHYSICAL_STEM:
        return ForwardCurveBoardPhysicalStemSummary(
            provenance=no_data_signal_provenance(MarketSignalType.PHYSICAL_STEM)
        )
    raise ValueError(f"Unsupported no-data summary signal type: {signal_type}")


def no_data_fair_price_band_provenance() -> ForwardCurveSignalProvenance:
    return no_data_signal_provenance(MarketSignalType.FAIR_PRICE_BAND)


def _key_for_row(row) -> SignalKey:
    return (row.market_product, row.delivery_point_id, row.availability_window)


def _apply_key_filters(stmt: Select, model: type[_T], keys: list[SignalKey]) -> Select:
    market_products = sorted({key[0] for key in keys})
    delivery_point_ids = sorted({key[1] for key in keys}, key=str)
    windows = sorted({key[2] for key in keys})
    return stmt.where(
        model.market_product.in_(market_products),
        model.delivery_point_id.in_(delivery_point_ids),
        model.availability_window.in_(windows),
    )


def _is_trusted_real(row, run: MarketSignalIngestionRun | None, signal_type: MarketSignalType) -> bool:
    expected_source_kind = _SIGNAL_TO_REAL_SOURCE_KIND[signal_type].value
    return (
        not bool(row.is_demo)
        and bool(row.is_verified_real)
        and row.trusted_ingestion_run_id is not None
        and run is not None
        and run.verified_at is not None
        and run.source == row.source
        and run.signal_family == signal_type.value
        and run.source_kind == expected_source_kind
    )


def _classify_rows(rows_with_runs: list[tuple[object, MarketSignalIngestionRun | None]], signal_type: MarketSignalType):
    if not rows_with_runs:
        return ForwardCurveSignalSourceKind.NO_DATA, MarketDemoStatus.NOT_APPLICABLE, 0, 0, 0, None

    real_count = 0
    demo_count = 0
    unknown_count = 0
    observed_at = None
    for row, run in rows_with_runs:
        if observed_at is None or row.observed_at > observed_at:
            observed_at = row.observed_at
        if bool(row.is_demo):
            demo_count += 1
        elif _is_trusted_real(row, run, signal_type):
            real_count += 1
        else:
            unknown_count += 1

    if unknown_count:
        return ForwardCurveSignalSourceKind.UNKNOWN, MarketDemoStatus.UNKNOWN, real_count, demo_count, unknown_count, observed_at
    if real_count and demo_count:
        return ForwardCurveSignalSourceKind.MIXED_SOURCE, MarketDemoStatus.MIXED, real_count, demo_count, unknown_count, observed_at
    if real_count:
        return _SIGNAL_TO_REAL_SOURCE_KIND[signal_type], MarketDemoStatus.REAL_ONLY, real_count, demo_count, unknown_count, observed_at
    if demo_count:
        return ForwardCurveSignalSourceKind.DEMO_SEED, MarketDemoStatus.DEMO_ONLY, real_count, demo_count, unknown_count, observed_at
    return ForwardCurveSignalSourceKind.NO_DATA, MarketDemoStatus.NOT_APPLICABLE, 0, 0, 0, observed_at


def _provenance(rows_with_runs: list[tuple[object, MarketSignalIngestionRun | None]], signal_type: MarketSignalType) -> ForwardCurveSignalProvenance:
    source_kind, demo_status, real_count, demo_count, unknown_count, observed_at = _classify_rows(rows_with_runs, signal_type)
    return ForwardCurveSignalProvenance(
        signal_type=signal_type,
        signal_source_kind=source_kind,
        scope=MarketScope.DELIVERY_POINT,
        demo_status=demo_status,
        observed_at=observed_at,
        generated_at=datetime.now(timezone.utc),
        real_count=real_count,
        demo_count=demo_count,
        unknown_count=unknown_count,
    )


def _single_row_provenance(row, run: MarketSignalIngestionRun | None, signal_type: MarketSignalType) -> ForwardCurveSignalProvenance:
    return _provenance([(row, run)], signal_type)


def _lookback_cutoff(days: int) -> datetime:
    return datetime.now(timezone.utc) - timedelta(days=days)


def _ranked_latest_stmt(model, *, keys: list[SignalKey], partition_by: list, lookback_days: int):
    ranked = (
        select(
            model.id.label("id"),
            func.row_number()
            .over(
                partition_by=partition_by,
                order_by=(model.observed_at.desc(), model.created_at.desc(), model.id.desc()),
            )
            .label("rn"),
        )
        .where(model.observed_at >= _lookback_cutoff(lookback_days))
    )
    ranked = _apply_key_filters(ranked, model, keys).subquery()
    return (
        select(model, MarketSignalIngestionRun)
        .join(ranked, model.id == ranked.c.id)
        .outerjoin(MarketSignalIngestionRun, model.trusted_ingestion_run_id == MarketSignalIngestionRun.id)
        .where(ranked.c.rn == 1)
    )


async def load_indication_summaries(
    db: AsyncSession,
    keys: Iterable[SignalKey],
    *,
    lookback_days: int = 7,
) -> dict[SignalKey, ForwardCurveBoardIndicationSummary]:
    normalized_keys = normalize_signal_keys(keys)
    if not normalized_keys:
        return {}

    stmt = _ranked_latest_stmt(
        MarketIndication,
        keys=normalized_keys,
        partition_by=[
            MarketIndication.market_product,
            MarketIndication.delivery_point_id,
            MarketIndication.availability_window,
            MarketIndication.side,
        ],
        lookback_days=lookback_days,
    )
    result = await db.execute(stmt)
    grouped: dict[SignalKey, list[tuple[MarketIndication, MarketSignalIngestionRun | None]]] = {}
    for row, run in result.all():
        key = _key_for_row(row)
        if key in normalized_keys:
            grouped.setdefault(key, []).append((row, run))

    summaries: dict[SignalKey, ForwardCurveBoardIndicationSummary] = {}
    for key, rows_with_runs in grouped.items():
        latest_bid = None
        latest_ask = None
        latest_mid = None
        total_quantity = Decimal("0")
        has_quantity = False
        for row, _run in rows_with_runs:
            if row.side == ForwardCurveIndicationSide.BID.value:
                latest_bid = row.price_per_mt_usd
            elif row.side == ForwardCurveIndicationSide.ASK.value:
                latest_ask = row.price_per_mt_usd
            elif row.side == ForwardCurveIndicationSide.MID.value:
                latest_mid = row.price_per_mt_usd
            if row.quantity_mt is not None:
                total_quantity += row.quantity_mt
                has_quantity = True

        summaries[key] = ForwardCurveBoardIndicationSummary(
            provenance=_provenance(rows_with_runs, MarketSignalType.MARKET_INDICATION),
            latest_bid_price_per_mt_usd=latest_bid,
            latest_ask_price_per_mt_usd=latest_ask,
            latest_mid_price_per_mt_usd=latest_mid,
            total_quantity_mt=total_quantity if has_quantity else None,
            indication_count=len(rows_with_runs),
        )
    return summaries


async def load_latest_indications_for_focus(
    db: AsyncSession,
    market_product: str,
    delivery_point_id: UUID,
    availability_window: str,
    *,
    limit: int = 10,
    lookback_days: int = 7,
) -> list[ForwardCurveBoardIndication]:
    key = (market_product, delivery_point_id, availability_window)
    stmt = _ranked_latest_stmt(
        MarketIndication,
        keys=[key],
        partition_by=[
            MarketIndication.market_product,
            MarketIndication.delivery_point_id,
            MarketIndication.availability_window,
            MarketIndication.side,
        ],
        lookback_days=lookback_days,
    ).order_by(MarketIndication.observed_at.desc(), MarketIndication.created_at.desc(), MarketIndication.id.desc()).limit(limit)
    result = await db.execute(stmt)
    return [
        ForwardCurveBoardIndication(
            side=ForwardCurveIndicationSide(row.side),
            price_per_mt_usd=row.price_per_mt_usd,
            quantity_mt=row.quantity_mt,
            provenance=_single_row_provenance(row, run, MarketSignalType.MARKET_INDICATION),
        )
        for row, run in result.all()
    ]


async def load_fair_price_bands(
    db: AsyncSession,
    keys: Iterable[SignalKey],
    *,
    lookback_days: int = 30,
) -> dict[SignalKey, ForwardCurveBoardFairPriceBand]:
    normalized_keys = normalize_signal_keys(keys)
    if not normalized_keys:
        return {}

    stmt = _ranked_latest_stmt(
        FairPriceBand,
        keys=normalized_keys,
        partition_by=[
            FairPriceBand.market_product,
            FairPriceBand.delivery_point_id,
            FairPriceBand.availability_window,
        ],
        lookback_days=lookback_days,
    )
    result = await db.execute(stmt)
    bands: dict[SignalKey, ForwardCurveBoardFairPriceBand] = {}
    for row, run in result.all():
        key = _key_for_row(row)
        if key not in normalized_keys:
            continue
        bands[key] = ForwardCurveBoardFairPriceBand(
            low_price_per_mt_usd=row.low_price_per_mt_usd,
            mid_price_per_mt_usd=row.mid_price_per_mt_usd,
            high_price_per_mt_usd=row.high_price_per_mt_usd,
            provenance=_single_row_provenance(row, run, MarketSignalType.FAIR_PRICE_BAND),
        )
    return bands


async def load_physical_stem_summaries(
    db: AsyncSession,
    keys: Iterable[SignalKey],
    *,
    lookback_days: int = 90,
) -> dict[SignalKey, ForwardCurveBoardPhysicalStemSummary]:
    normalized_keys = normalize_signal_keys(keys)
    if not normalized_keys:
        return {}

    stmt = _ranked_latest_stmt(
        PhysicalStem,
        keys=normalized_keys,
        partition_by=[
            PhysicalStem.market_product,
            PhysicalStem.delivery_point_id,
            PhysicalStem.availability_window,
            PhysicalStem.source,
            PhysicalStem.stem_uid,
        ],
        lookback_days=lookback_days,
    )
    result = await db.execute(stmt)
    grouped: dict[SignalKey, list[tuple[PhysicalStem, MarketSignalIngestionRun | None]]] = {}
    for row, run in result.all():
        key = _key_for_row(row)
        if key in normalized_keys:
            grouped.setdefault(key, []).append((row, run))

    summaries: dict[SignalKey, ForwardCurveBoardPhysicalStemSummary] = {}
    for key, rows_with_runs in grouped.items():
        available_quantity = Decimal("0")
        tentative_quantity = Decimal("0")
        stem_count = 0
        earliest_start = None
        latest_end = None
        for row, _run in rows_with_runs:
            if row.status not in {
                ForwardCurvePhysicalStemStatus.AVAILABLE.value,
                ForwardCurvePhysicalStemStatus.TENTATIVE.value,
            }:
                continue
            stem_count += 1
            if row.status == ForwardCurvePhysicalStemStatus.AVAILABLE.value:
                available_quantity += row.quantity_mt
            else:
                tentative_quantity += row.quantity_mt
            if row.stem_start is not None and (earliest_start is None or row.stem_start < earliest_start):
                earliest_start = row.stem_start
            if row.stem_end is not None and (latest_end is None or row.stem_end > latest_end):
                latest_end = row.stem_end

        summaries[key] = ForwardCurveBoardPhysicalStemSummary(
            provenance=_provenance(rows_with_runs, MarketSignalType.PHYSICAL_STEM),
            available_quantity_mt=available_quantity if available_quantity > 0 else None,
            tentative_quantity_mt=tentative_quantity if tentative_quantity > 0 else None,
            stem_count=stem_count,
            earliest_stem_start=earliest_start,
            latest_stem_end=latest_end,
        )
    return summaries


async def load_physical_stems_for_focus(
    db: AsyncSession,
    market_product: str,
    delivery_point_id: UUID,
    availability_window: str,
    *,
    limit: int = 10,
    lookback_days: int = 90,
) -> list[ForwardCurveBoardPhysicalStem]:
    key = (market_product, delivery_point_id, availability_window)
    stmt = _ranked_latest_stmt(
        PhysicalStem,
        keys=[key],
        partition_by=[
            PhysicalStem.market_product,
            PhysicalStem.delivery_point_id,
            PhysicalStem.availability_window,
            PhysicalStem.source,
            PhysicalStem.stem_uid,
        ],
        lookback_days=lookback_days,
    ).order_by(PhysicalStem.observed_at.desc(), PhysicalStem.created_at.desc(), PhysicalStem.id.desc()).limit(limit)
    result = await db.execute(stmt)
    return [
        ForwardCurveBoardPhysicalStem(
            quantity_mt=row.quantity_mt,
            status=ForwardCurvePhysicalStemStatus(row.status),
            stem_start=row.stem_start,
            stem_end=row.stem_end,
            provenance=_single_row_provenance(row, run, MarketSignalType.PHYSICAL_STEM),
        )
        for row, run in result.all()
    ]

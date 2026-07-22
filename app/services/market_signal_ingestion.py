"""Trusted market-signal ingestion: parse -> validate -> plan -> write.

Turns operator-supplied rows (from CSV files) into verified
``MarketSignalIngestionRun`` records whose signal rows classify as REAL
through the trust predicate in ``app.services.forward_monitoring``.

Transaction ownership: this service NEVER commits or rolls back the outer
transaction. It works in nested savepoints and returns an ``IngestReport``;
the caller owns the outer transaction (matching the ``record_audit``
non-committing convention).
"""
from __future__ import annotations

import uuid
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from decimal import Decimal, InvalidOperation

from sqlalchemy import select
from sqlalchemy.exc import IntegrityError
from sqlalchemy.ext.asyncio import AsyncSession

from app.market_catalog import MarketProduct
from app.models.catalog import DeliveryPoint
from app.models.forward_monitoring import (
    FairPriceBand,
    MarketIndication,
    MarketSignalIngestionRun,
    PhysicalStem,
)
from app.services.availability_windows import normalize_availability_window

# Single source of truth for the family -> run source_kind mapping. The two
# enums differ for fair bands (run family FAIR_PRICE_BAND, source_kind
# FAIR_PRICE_MODEL per the CHECK constraints on market_signal_ingestion_runs);
# getting this wrong passes the CHECK but fails the trust join, so every row
# would silently render as untrusted.
FAMILY_TO_SOURCE_KIND = {
    "MARKET_INDICATION": "MARKET_INDICATION",
    "FAIR_PRICE_BAND": "FAIR_PRICE_MODEL",
    "PHYSICAL_STEM": "PHYSICAL_STEM",
}

# Board visibility lookbacks (mirror the loader defaults in
# app/services/forward_monitoring.py). Older rows import fine but never render;
# the operator gets a staleness warning instead of a debugging session.
FAMILY_STALENESS_DAYS = {
    "MARKET_INDICATION": 7,
    "FAIR_PRICE_BAND": 30,
    "PHYSICAL_STEM": 90,
}

_INDICATION_SIDES = {"BID", "ASK", "MID"}
_STEM_STATUSES = {"AVAILABLE", "TENTATIVE", "ALLOCATED", "CANCELLED"}
_MARKET_PRODUCTS = {member.value for member in MarketProduct}
_FUTURE_TOLERANCE = timedelta(minutes=5)
_MAX_PRICE = Decimal("10") ** 8

_REQUIRED_COLUMNS = {
    "MARKET_INDICATION": {
        "source_event_id",
        "market_product",
        "delivery_point",
        "availability_window",
        "side",
        "price_per_mt_usd",
        "observed_at",
    },
    "FAIR_PRICE_BAND": {
        "source_event_id",
        "market_product",
        "delivery_point",
        "availability_window",
        "low_price_per_mt_usd",
        "mid_price_per_mt_usd",
        "high_price_per_mt_usd",
        "model_name",
        "observed_at",
    },
    "PHYSICAL_STEM": {
        "source_event_id",
        "stem_uid",
        "market_product",
        "delivery_point",
        "availability_window",
        "status",
        "quantity_mt",
        "observed_at",
    },
}


@dataclass
class RowError:
    row_number: int
    reason: str


@dataclass
class IngestReport:
    family: str
    source: str
    dry_run: bool
    created_run_id: uuid.UUID | None = None
    inserted: int = 0
    would_insert: int = 0
    skipped_duplicates: int = 0
    stale_warnings: int = 0
    errors: list[RowError] = field(default_factory=list)


class _RowInvalid(Exception):
    def __init__(self, reason: str):
        super().__init__(reason)
        self.reason = reason


def _parse_decimal(raw: object, label: str, *, quantize_price: bool = False) -> Decimal:
    try:
        value = Decimal(str(raw).strip())
    except (InvalidOperation, ValueError) as exc:
        raise _RowInvalid(f"{label} is not a valid number: {raw!r}") from exc
    if value <= 0:
        raise _RowInvalid(f"{label} must be positive, got {value}")
    if quantize_price:
        if value >= _MAX_PRICE:
            raise _RowInvalid(f"{label} exceeds the Numeric(10,2) range: {value}")
        value = value.quantize(Decimal("0.01"))
    return value


def _parse_observed_at(raw: object) -> datetime:
    text = str(raw).strip()
    try:
        value = datetime.fromisoformat(text)
    except ValueError as exc:
        raise _RowInvalid(f"observed_at is not ISO-8601: {raw!r}") from exc
    if value.tzinfo is None:
        raise _RowInvalid("observed_at must carry a timezone (naive timestamps rejected)")
    value = value.astimezone(timezone.utc)
    if value > datetime.now(timezone.utc) + _FUTURE_TOLERANCE:
        raise _RowInvalid(f"observed_at is in the future: {value.isoformat()}")
    return value


def _parse_optional_datetime(raw: object, label: str) -> datetime | None:
    text = str(raw).strip() if raw is not None else ""
    if not text:
        return None
    try:
        value = datetime.fromisoformat(text)
    except ValueError as exc:
        raise _RowInvalid(f"{label} is not ISO-8601: {raw!r}") from exc
    if value.tzinfo is None:
        raise _RowInvalid(f"{label} must carry a timezone")
    return value.astimezone(timezone.utc)


def _required_text(row: dict, column: str) -> str:
    value = str(row.get(column) or "").strip()
    if not value:
        raise _RowInvalid(f"{column} is required and cannot be blank")
    return value


class _DeliveryPointResolver:
    """Resolve delivery points by exact name or UUID against the catalog."""

    def __init__(self, points: list[DeliveryPoint]):
        self._by_name = {point.name: point.id for point in points}
        self._by_id = {point.id: point.id for point in points}

    def resolve(self, raw: str) -> uuid.UUID:
        text = raw.strip()
        if text in self._by_name:
            return self._by_name[text]
        try:
            candidate = uuid.UUID(text)
        except ValueError:
            candidate = None
        if candidate is not None and candidate in self._by_id:
            return candidate
        raise _RowInvalid(f"delivery_point {raw!r} is not an active catalog delivery point")


def _validate_common(row: dict, resolver: _DeliveryPointResolver) -> dict:
    product = _required_text(row, "market_product")
    if product not in _MARKET_PRODUCTS:
        raise _RowInvalid(
            f"market_product {product!r} is not one of the canonical products "
            f"{sorted(_MARKET_PRODUCTS)}"
        )
    delivery_point_id = resolver.resolve(_required_text(row, "delivery_point"))
    try:
        window = normalize_availability_window(_required_text(row, "availability_window"))
    except ValueError as exc:
        raise _RowInvalid(str(exc)) from exc
    return {
        "source_event_id": _required_text(row, "source_event_id"),
        "market_product": product,
        "delivery_point_id": delivery_point_id,
        "availability_window": window,
        "observed_at": _parse_observed_at(row.get("observed_at")),
    }


def _plan_indication(row: dict, resolver: _DeliveryPointResolver) -> dict:
    planned = _validate_common(row, resolver)
    side = _required_text(row, "side").upper()
    if side not in _INDICATION_SIDES:
        raise _RowInvalid(f"side must be one of {sorted(_INDICATION_SIDES)}, got {side!r}")
    planned["side"] = side
    planned["price_per_mt_usd"] = _parse_decimal(
        row.get("price_per_mt_usd"), "price_per_mt_usd", quantize_price=True
    )
    quantity_raw = str(row.get("quantity_mt") or "").strip()
    planned["quantity_mt"] = (
        _parse_decimal(quantity_raw, "quantity_mt") if quantity_raw else None
    )
    return planned


def _plan_fair_band(row: dict, resolver: _DeliveryPointResolver) -> dict:
    planned = _validate_common(row, resolver)
    low = _parse_decimal(row.get("low_price_per_mt_usd"), "low_price_per_mt_usd", quantize_price=True)
    mid = _parse_decimal(row.get("mid_price_per_mt_usd"), "mid_price_per_mt_usd", quantize_price=True)
    high = _parse_decimal(row.get("high_price_per_mt_usd"), "high_price_per_mt_usd", quantize_price=True)
    if not (low <= mid <= high):
        raise _RowInvalid(f"band ordering violated: low={low} mid={mid} high={high}")
    planned.update(
        low_price_per_mt_usd=low,
        mid_price_per_mt_usd=mid,
        high_price_per_mt_usd=high,
        model_name=_required_text(row, "model_name"),
        model_version=str(row.get("model_version") or "").strip() or None,
    )
    return planned


def _plan_stem(row: dict, resolver: _DeliveryPointResolver) -> dict:
    planned = _validate_common(row, resolver)
    status = _required_text(row, "status").upper()
    if status not in _STEM_STATUSES:
        raise _RowInvalid(f"status must be one of {sorted(_STEM_STATUSES)}, got {status!r}")
    stem_start = _parse_optional_datetime(row.get("stem_start"), "stem_start")
    stem_end = _parse_optional_datetime(row.get("stem_end"), "stem_end")
    if stem_start is not None and stem_end is not None and stem_end < stem_start:
        raise _RowInvalid(f"stem_end {stem_end} precedes stem_start {stem_start}")
    planned.update(
        stem_uid=_required_text(row, "stem_uid"),
        status=status,
        quantity_mt=_parse_decimal(row.get("quantity_mt"), "quantity_mt"),
        stem_start=stem_start,
        stem_end=stem_end,
    )
    return planned


_FAMILY_PLANNERS = {
    "MARKET_INDICATION": _plan_indication,
    "FAIR_PRICE_BAND": _plan_fair_band,
    "PHYSICAL_STEM": _plan_stem,
}

_FAMILY_MODELS = {
    "MARKET_INDICATION": MarketIndication,
    "FAIR_PRICE_BAND": FairPriceBand,
    "PHYSICAL_STEM": PhysicalStem,
}


def _check_columns(family: str, rows: list[dict]) -> None:
    """Fail fast when a file's shape does not match the declared family."""
    if not rows:
        return
    missing = _REQUIRED_COLUMNS[family] - set(rows[0].keys())
    if missing:
        raise _RowInvalid(
            f"rows do not match family {family}: missing columns {sorted(missing)}"
        )


async def ingest_signals(
    db: AsyncSession,
    *,
    family: str,
    source: str,
    rows: list[dict],
    dry_run: bool = False,
) -> IngestReport:
    """Validate and insert one run's worth of rows for one signal family.

    Never commits: works in per-row savepoints and flushes at the end so the
    caller sees ids. A run whose rows all fail (or all duplicate) is removed
    from the session before returning — a verified run with zero rows must
    never persist.
    """
    if family not in _FAMILY_PLANNERS:
        raise ValueError(
            f"family must be one of {sorted(_FAMILY_PLANNERS)}, got {family!r}"
        )

    report = IngestReport(family=family, source=source, dry_run=dry_run)
    planner = _FAMILY_PLANNERS[family]
    model = _FAMILY_MODELS[family]
    staleness_cutoff = datetime.now(timezone.utc) - timedelta(
        days=FAMILY_STALENESS_DAYS[family]
    )

    try:
        _check_columns(family, rows)
    except _RowInvalid as exc:
        report.errors.append(RowError(row_number=0, reason=exc.reason))
        return report

    points = (
        (
            await db.execute(
                select(DeliveryPoint).where(DeliveryPoint.is_active.is_(True))
            )
        )
        .scalars()
        .all()
    )
    resolver = _DeliveryPointResolver(list(points))

    planned_rows: list[dict] = []
    for index, row in enumerate(rows, start=1):
        try:
            planned = planner(row, resolver)
        except _RowInvalid as exc:
            report.errors.append(RowError(row_number=index, reason=exc.reason))
            continue
        if planned["observed_at"] < staleness_cutoff:
            report.stale_warnings += 1
        planned_rows.append(planned)

    if dry_run or not planned_rows:
        report.would_insert = len(planned_rows)
        return report

    now = datetime.now(timezone.utc)
    run = MarketSignalIngestionRun(
        signal_family=family,
        source=source,
        source_kind=FAMILY_TO_SOURCE_KIND[family],
        started_at=now,
        verified_at=None,
    )
    db.add(run)
    await db.flush()

    for planned in planned_rows:
        record = model(
            **planned,
            source=source,
            trusted_ingestion_run_id=run.id,
            is_demo=False,
            is_verified_real=True,
            verified_real_at=now,
        )
        try:
            async with db.begin_nested():
                db.add(record)
                await db.flush()
        except IntegrityError:
            # Unique (source, source_event_id) collision: skip-and-continue via
            # the savepoint; a raw IntegrityError would poison the whole
            # transaction on Postgres.
            report.skipped_duplicates += 1
            continue
        report.inserted += 1

    if report.inserted > 0:
        run.verified_at = datetime.now(timezone.utc)
        report.created_run_id = run.id
        await db.flush()
    else:
        # All rows duplicated or failed at insert time: a verified-empty run is
        # a forgery primitive, so the run must not survive.
        await db.delete(run)
        await db.flush()

    return report

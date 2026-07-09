"""Explicit demo seed data for Forward Curve monitoring signals."""
from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from decimal import Decimal

from sqlalchemy import delete
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.forward_monitoring import FairPriceBand, MarketIndication, PhysicalStem
from app.models.orderbook import OrderSide
from app.seeds.catalog_seed import DELIVERY_POINT_IDS, PRODUCTS
from app.seeds.market_seed import PRICING, SPOT_WINDOW, _seed_price_for_slice


DEMO_FORWARD_MONITORING_SOURCE = "demo_seed"
DEMO_FORWARD_MONITORING_EVENT_PREFIX = "forward-monitoring-demo:"

_PRODUCT_TO_MARKET_PRODUCT = {
    product.name: product.market_product
    for product in PRODUCTS
    if product.market_product is not None
}


@dataclass(frozen=True)
class ForwardMonitoringSeedResult:
    deleted_indications: int
    deleted_fair_price_bands: int
    deleted_physical_stems: int
    inserted_indications: int
    inserted_fair_price_bands: int
    inserted_physical_stems: int


def _utcnow() -> datetime:
    return datetime.now(timezone.utc)


def _add_month_offset(year: int, month: int, offset: int) -> tuple[int, int]:
    zero_based = month - 1 + offset
    return year + (zero_based // 12), (zero_based % 12) + 1


def _default_curve_windows(reference_now: datetime) -> list[str]:
    """Mirror the board's default focus windows without importing the router."""
    year = reference_now.year
    month = reference_now.month
    quarter = ((month - 1) // 3) + 1

    windows = [SPOT_WINDOW]
    for offset in range(1, 7):
        next_year, next_month = _add_month_offset(year, month, offset)
        windows.append(f"{next_year}-{next_month:02d}")

    for offset in range(1, 5):
        absolute_quarter = (year * 4) + (quarter - 1) + offset
        quarter_year = absolute_quarter // 4
        next_quarter = (absolute_quarter % 4) + 1
        windows.append(f"{quarter_year}-Q{next_quarter}")

    windows.extend([f"{year + 1}-CAL", f"{year + 2}-CAL"])
    return windows


def _demo_slices(reference_now: datetime) -> list[tuple[str, str, str]]:
    slices: list[tuple[str, str, str]] = []
    seen: set[tuple[str, str, str]] = set()

    def add(product_name: str, port_name: str, window: str) -> None:
        key = (product_name, port_name, window)
        if key not in seen:
            seen.add(key)
            slices.append(key)

    for product_name, ports in PRICING.items():
        for port_name in ports:
            add(product_name, port_name, SPOT_WINDOW)

    curve_windows = _default_curve_windows(reference_now)
    for window in curve_windows:
        add("Bio Methanol", "Singapore", window)

    if len(curve_windows) > 1:
        add("e-Methanol", "Rotterdam", curve_windows[1])
    first_quarter = next((window for window in curve_windows if "-Q" in window), None)
    if first_quarter is not None:
        add("Bio Ethanol", "Santos", first_quarter)

    return slices


def _window_adjustment(window: str, curve_windows: list[str]) -> Decimal:
    if window == SPOT_WINDOW:
        return Decimal("0.00")
    try:
        index = curve_windows.index(window)
    except ValueError:
        return Decimal("0.00")
    if "-CAL" in window:
        return Decimal(index * 9).quantize(Decimal("0.01"))
    if "-Q" in window:
        return Decimal(index * 7).quantize(Decimal("0.01"))
    return Decimal(index * 4).quantize(Decimal("0.01"))


def _prices_for_slice(
    product_name: str,
    port_name: str,
    window: str,
    curve_windows: list[str],
) -> tuple[Decimal, Decimal, Decimal, Decimal, Decimal]:
    bid_lo, bid_hi, ask_lo, ask_hi = PRICING[product_name][port_name]
    adjustment = _window_adjustment(window, curve_windows)
    bid_price = (
        _seed_price_for_slice(
            OrderSide.BID,
            bid_lo=bid_lo,
            bid_hi=bid_hi,
            ask_lo=ask_lo,
            ask_hi=ask_hi,
            window=SPOT_WINDOW,
            depth_index=0,
        )
        + adjustment
        - Decimal("2.00")
    ).quantize(Decimal("0.01"))
    ask_price = (
        _seed_price_for_slice(
            OrderSide.ASK,
            bid_lo=bid_lo,
            bid_hi=bid_hi,
            ask_lo=ask_lo,
            ask_hi=ask_hi,
            window=SPOT_WINDOW,
            depth_index=0,
        )
        + adjustment
        + Decimal("2.00")
    ).quantize(Decimal("0.01"))
    mid_price = ((bid_price + ask_price) / Decimal("2")).quantize(Decimal("0.01"))
    band_width = max((ask_price - bid_price) / Decimal("3"), Decimal("8.00")).quantize(Decimal("0.01"))
    low_price = (mid_price - band_width).quantize(Decimal("0.01"))
    high_price = (mid_price + band_width).quantize(Decimal("0.01"))
    return bid_price, ask_price, low_price, mid_price, high_price


def _event_id(table: str, product_name: str, port_name: str, window: str, suffix: str) -> str:
    normalized = "|".join([table, product_name, port_name, window, suffix]).lower().replace(" ", "-")
    return f"{DEMO_FORWARD_MONITORING_EVENT_PREFIX}{normalized}"


def _source_record_id(table: str, product_name: str, port_name: str, window: str) -> str:
    normalized = "|".join([table, product_name, port_name, window]).lower().replace(" ", "-")
    return f"{DEMO_FORWARD_MONITORING_EVENT_PREFIX}{normalized}"


def _rowcount(result) -> int:
    return int(result.rowcount or 0)


async def _delete_demo_rows(db: AsyncSession) -> tuple[int, int, int]:
    deleted_indications = _rowcount(await db.execute(
        delete(MarketIndication).where(
            MarketIndication.source == DEMO_FORWARD_MONITORING_SOURCE,
            MarketIndication.source_event_id.like(f"{DEMO_FORWARD_MONITORING_EVENT_PREFIX}%"),
            MarketIndication.is_demo.is_(True),
            MarketIndication.is_verified_real.is_(False),
            MarketIndication.trusted_ingestion_run_id.is_(None),
            MarketIndication.verified_real_at.is_(None),
        )
    ))
    deleted_fair_price_bands = _rowcount(await db.execute(
        delete(FairPriceBand).where(
            FairPriceBand.source == DEMO_FORWARD_MONITORING_SOURCE,
            FairPriceBand.source_event_id.like(f"{DEMO_FORWARD_MONITORING_EVENT_PREFIX}%"),
            FairPriceBand.is_demo.is_(True),
            FairPriceBand.is_verified_real.is_(False),
            FairPriceBand.trusted_ingestion_run_id.is_(None),
            FairPriceBand.verified_real_at.is_(None),
        )
    ))
    deleted_physical_stems = _rowcount(await db.execute(
        delete(PhysicalStem).where(
            PhysicalStem.source == DEMO_FORWARD_MONITORING_SOURCE,
            PhysicalStem.source_event_id.like(f"{DEMO_FORWARD_MONITORING_EVENT_PREFIX}%"),
            PhysicalStem.is_demo.is_(True),
            PhysicalStem.is_verified_real.is_(False),
            PhysicalStem.trusted_ingestion_run_id.is_(None),
            PhysicalStem.verified_real_at.is_(None),
        )
    ))
    return deleted_indications, deleted_fair_price_bands, deleted_physical_stems


async def seed_forward_monitoring_demo_data(
    db: AsyncSession,
    *,
    reference_now: datetime | None = None,
) -> ForwardMonitoringSeedResult:
    """Seed explicit demo-only monitoring signal rows for staging review."""
    now = reference_now or _utcnow()
    curve_windows = _default_curve_windows(now)

    deleted_indications, deleted_fair_price_bands, deleted_physical_stems = await _delete_demo_rows(db)

    indication_rows: list[MarketIndication] = []
    band_rows: list[FairPriceBand] = []
    stem_rows: list[PhysicalStem] = []

    for index, (product_name, port_name, window) in enumerate(_demo_slices(now)):
        market_product = _PRODUCT_TO_MARKET_PRODUCT[product_name]
        delivery_point_id = DELIVERY_POINT_IDS[port_name]
        observed_at = now - timedelta(hours=index % 18, minutes=5)
        bid_price, ask_price, low_price, mid_price, high_price = _prices_for_slice(
            product_name,
            port_name,
            window,
            curve_windows,
        )
        base_quantity = Decimal(1000 + ((index % 7) * 250)).quantize(Decimal("0.01"))
        tentative_quantity = Decimal(500 + ((index % 5) * 200)).quantize(Decimal("0.01"))
        record_id = _source_record_id("indication", product_name, port_name, window)
        stem_start = now + timedelta(days=2 + (index % 8))
        stem_end = stem_start + timedelta(days=2)

        indication_rows.extend([
            MarketIndication(
                market_product=market_product,
                delivery_point_id=delivery_point_id,
                availability_window=window,
                side="BID",
                price_per_mt_usd=bid_price,
                quantity_mt=base_quantity,
                source=DEMO_FORWARD_MONITORING_SOURCE,
                source_record_id=record_id,
                source_event_id=_event_id("indication", product_name, port_name, window, "bid"),
                is_demo=True,
                is_verified_real=False,
                observed_at=observed_at,
            ),
            MarketIndication(
                market_product=market_product,
                delivery_point_id=delivery_point_id,
                availability_window=window,
                side="ASK",
                price_per_mt_usd=ask_price,
                quantity_mt=(base_quantity + Decimal("250.00")).quantize(Decimal("0.01")),
                source=DEMO_FORWARD_MONITORING_SOURCE,
                source_record_id=record_id,
                source_event_id=_event_id("indication", product_name, port_name, window, "ask"),
                is_demo=True,
                is_verified_real=False,
                observed_at=observed_at + timedelta(minutes=1),
            ),
        ])

        band_rows.append(
            FairPriceBand(
                market_product=market_product,
                delivery_point_id=delivery_point_id,
                availability_window=window,
                low_price_per_mt_usd=low_price,
                mid_price_per_mt_usd=mid_price,
                high_price_per_mt_usd=high_price,
                model_name="demo_forward_monitor",
                model_version="2026-06-demo",
                source=DEMO_FORWARD_MONITORING_SOURCE,
                source_event_id=_event_id("fair-band", product_name, port_name, window, "band"),
                is_demo=True,
                is_verified_real=False,
                observed_at=observed_at + timedelta(minutes=2),
            )
        )

        stem_uid_base = _source_record_id("stem", product_name, port_name, window)
        stem_rows.extend([
            PhysicalStem(
                market_product=market_product,
                delivery_point_id=delivery_point_id,
                availability_window=window,
                quantity_mt=base_quantity,
                stem_start=stem_start,
                stem_end=stem_end,
                status="AVAILABLE",
                source=DEMO_FORWARD_MONITORING_SOURCE,
                stem_uid=f"{stem_uid_base}:available",
                source_record_id=stem_uid_base,
                source_event_id=_event_id("stem", product_name, port_name, window, "available"),
                is_demo=True,
                is_verified_real=False,
                observed_at=observed_at + timedelta(minutes=3),
            ),
            PhysicalStem(
                market_product=market_product,
                delivery_point_id=delivery_point_id,
                availability_window=window,
                quantity_mt=tentative_quantity,
                stem_start=stem_start + timedelta(days=4),
                stem_end=stem_end + timedelta(days=4),
                status="TENTATIVE",
                source=DEMO_FORWARD_MONITORING_SOURCE,
                stem_uid=f"{stem_uid_base}:tentative",
                source_record_id=stem_uid_base,
                source_event_id=_event_id("stem", product_name, port_name, window, "tentative"),
                is_demo=True,
                is_verified_real=False,
                observed_at=observed_at + timedelta(minutes=4),
            ),
        ])

    db.add_all([*indication_rows, *band_rows, *stem_rows])
    await db.commit()
    return ForwardMonitoringSeedResult(
        deleted_indications=deleted_indications,
        deleted_fair_price_bands=deleted_fair_price_bands,
        deleted_physical_stems=deleted_physical_stems,
        inserted_indications=len(indication_rows),
        inserted_fair_price_bands=len(band_rows),
        inserted_physical_stems=len(stem_rows),
    )


def expected_forward_monitoring_demo_slices(reference_now: datetime | None = None) -> int:
    """Return the number of slice contexts seeded by the demo monitoring seeder."""
    return len(_demo_slices(reference_now or _utcnow()))

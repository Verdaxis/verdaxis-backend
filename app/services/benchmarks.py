from __future__ import annotations

from datetime import UTC, datetime
from decimal import Decimal
import re
from typing import Iterable
from uuid import UUID

from sqlalchemy import select
from sqlalchemy.exc import OperationalError, ProgrammingError
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.benchmark import Benchmark
from app.models.catalog import DeliveryPoint
from app.schemas.benchmark import BenchmarkQuote
from app.seeds.market_seed import PRICING
from app.services.availability_windows import SPOT_WINDOW, normalize_availability_window

_WINDOW_MONTH_RE = re.compile(r"^(?P<year>\d{4})-(?P<month>\d{2})$")
_WINDOW_QUARTER_RE = re.compile(r"^(?P<year>\d{4})-Q(?P<quarter>[1-4])$")
_WINDOW_CAL_RE = re.compile(r"^(?P<year>\d{4})-CAL$")

MARKET_PRODUCT_NAMES = {
    "BIO_METHANOL": "Bio Methanol",
    "E_METHANOL": "e-Methanol",
    "BIO_ETHANOL": "Bio Ethanol",
    "SYNTHETIC_ETHANOL": "Synthetic Ethanol",
}


def _base_benchmark_price(market_product: str, delivery_point_name: str) -> Decimal | None:
    product_name = MARKET_PRODUCT_NAMES.get(market_product)
    if not product_name:
        return None

    port_prices = PRICING.get(product_name, {}).get(delivery_point_name)
    if not port_prices:
        return None

    bid_lo, bid_hi, ask_lo, ask_hi = port_prices
    midpoint = (Decimal(str(bid_lo)) + Decimal(str(bid_hi)) + Decimal(str(ask_lo)) + Decimal(str(ask_hi))) / Decimal("4")
    return midpoint.quantize(Decimal("0.01"))


def _month_index(year: int, month: int) -> int:
    return year * 12 + (month - 1)


def _window_adjustment(availability_window: str, *, now: datetime | None = None) -> Decimal:
    normalized = normalize_availability_window(availability_window)
    if normalized == SPOT_WINDOW:
        return Decimal("0.00")

    now = now or datetime.now(UTC)
    current_month_index = _month_index(now.year, now.month)

    month_match = _WINDOW_MONTH_RE.match(normalized)
    if month_match:
        target_index = _month_index(int(month_match.group("year")), int(month_match.group("month")))
        return (Decimal(max(target_index - current_month_index, 0)) * Decimal("7.50")).quantize(Decimal("0.01"))

    quarter_match = _WINDOW_QUARTER_RE.match(normalized)
    if quarter_match:
        year = int(quarter_match.group("year"))
        quarter = int(quarter_match.group("quarter"))
        start_month = ((quarter - 1) * 3) + 1
        target_index = _month_index(year, start_month)
        return (Decimal(max(target_index - current_month_index, 0)) * Decimal("7.50")).quantize(Decimal("0.01"))

    cal_match = _WINDOW_CAL_RE.match(normalized)
    if cal_match:
        year = int(cal_match.group("year"))
        target_index = _month_index(year, 1)
        return (Decimal(max(target_index - current_month_index, 0)) * Decimal("5.00")).quantize(Decimal("0.01"))

    return Decimal("0.00")


def compute_premium_discount(
    *,
    listing_price_per_mt_usd: Decimal,
    benchmark_price_per_mt_usd: Decimal,
) -> Decimal:
    return (listing_price_per_mt_usd - benchmark_price_per_mt_usd).quantize(Decimal("0.01"))


async def get_benchmark_quote(
    db: AsyncSession,
    *,
    market_product: str | None,
    delivery_point_id: UUID | None,
    availability_window: str,
) -> BenchmarkQuote | None:
    if not market_product or delivery_point_id is None:
        return None

    normalized_window = normalize_availability_window(availability_window)
    delivery_point = await db.get(DeliveryPoint, delivery_point_id)
    if not delivery_point:
        return None

    override_stmt = select(Benchmark).where(
        Benchmark.market_product == market_product,
        Benchmark.delivery_point_id == delivery_point_id,
        Benchmark.availability_window == normalized_window,
    )
    try:
        override_result = await db.execute(override_stmt)
        override = override_result.scalar_one_or_none()
    except (OperationalError, ProgrammingError):
        # Older SQLite-backed test schemas may not include the override table yet.
        # Fall back to the seeded benchmark matrix instead of failing the whole response path.
        override = None

    if override is not None:
        price = Decimal(str(override.price_per_mt_usd)).quantize(Decimal("0.01"))
        source = override.source
        observed_at = override.updated_at or override.created_at
    else:
        base_price = _base_benchmark_price(market_product, delivery_point.name)
        if base_price is None:
            return None
        price = (base_price + _window_adjustment(normalized_window)).quantize(Decimal("0.01"))
        source = "seed_matrix"
        observed_at = None

    generated_at = datetime.now(UTC)
    return BenchmarkQuote(
        market_product=market_product,
        delivery_point_id=delivery_point_id,
        delivery_point_name=delivery_point.name,
        availability_window=normalized_window,
        benchmark_price_per_mt_usd=price,
        source=source,
        generated_at=generated_at,
        observed_at=observed_at,
    )


async def get_benchmark_quotes(
    db: AsyncSession,
    requests: Iterable[tuple[str | None, UUID | None, str, str | None]],
) -> dict[tuple[str, UUID, str], BenchmarkQuote]:
    """Batch benchmark quotes for known market product, delivery point, window tuples.

    Each request is `(market_product, delivery_point_id, availability_window, delivery_point_name)`.
    The delivery point name is supplied by the caller to avoid a per-cell delivery-point lookup.
    """
    normalized_requests: dict[tuple[str, UUID, str], str] = {}
    for market_product, delivery_point_id, availability_window, delivery_point_name in requests:
        if not market_product or delivery_point_id is None or not delivery_point_name:
            continue
        normalized_requests[
            (
                market_product,
                delivery_point_id,
                normalize_availability_window(availability_window),
            )
        ] = delivery_point_name

    if not normalized_requests:
        return {}

    market_products = sorted({key[0] for key in normalized_requests})
    delivery_point_ids = sorted({key[1] for key in normalized_requests}, key=str)
    windows = sorted({key[2] for key in normalized_requests})

    overrides: dict[tuple[str, UUID, str], Benchmark] = {}
    override_stmt = select(Benchmark).where(
        Benchmark.market_product.in_(market_products),
        Benchmark.delivery_point_id.in_(delivery_point_ids),
        Benchmark.availability_window.in_(windows),
    )
    try:
        override_result = await db.execute(override_stmt)
        for override in override_result.scalars().all():
            overrides[(override.market_product, override.delivery_point_id, override.availability_window)] = override
    except (OperationalError, ProgrammingError):
        # Older SQLite-backed test schemas may not include the override table yet.
        overrides = {}

    generated_at = datetime.now(UTC)
    quotes: dict[tuple[str, UUID, str], BenchmarkQuote] = {}
    for key, delivery_point_name in normalized_requests.items():
        market_product, delivery_point_id, availability_window = key
        override = overrides.get(key)
        if override is not None:
            price = Decimal(str(override.price_per_mt_usd)).quantize(Decimal("0.01"))
            source = override.source
            observed_at = override.updated_at or override.created_at
        else:
            base_price = _base_benchmark_price(market_product, delivery_point_name)
            if base_price is None:
                continue
            price = (base_price + _window_adjustment(availability_window)).quantize(Decimal("0.01"))
            source = "seed_matrix"
            observed_at = None

        quotes[key] = BenchmarkQuote(
            market_product=market_product,
            delivery_point_id=delivery_point_id,
            delivery_point_name=delivery_point_name,
            availability_window=availability_window,
            benchmark_price_per_mt_usd=price,
            source=source,
            generated_at=generated_at,
            observed_at=observed_at,
        )

    return quotes

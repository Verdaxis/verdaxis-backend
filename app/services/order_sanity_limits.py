"""Fat-finger guard: reject implausible order prices and volumes.

The guard runs during request validation, BEFORE the market slice advisory
lock is taken, so a rejected order never contends for the lock. It reads only
(the published benchmark) and writes nothing.

Two independent rules, each disabled by setting its limit to 0:

* Price band: the limit price may not deviate from the slice's published
  benchmark (``get_benchmark_quote``: operator override, else the seed matrix
  shown to users as premium/discount) by more than
  ``ORDER_MAX_PRICE_DEVIATION_PCT`` percent. When the slice has no benchmark
  the price rule is skipped; the order is never blocked for lack of a
  reference.
* Volume cap: a single order may not exceed ``ORDER_MAX_QUANTITY_MT``.

Rejections are HTTP 400 with a stable ``code`` and the threshold in the body.
They are logged but deliberately not written to ``audit_logs``: a rejection
changes no state, the request transaction rolls back, and the audit registry
excludes high-volume non-state events.
"""
from __future__ import annotations

import logging
from decimal import ROUND_CEILING, ROUND_FLOOR, Decimal
from uuid import UUID

from app.config import settings
from app.services.benchmarks import get_benchmark_quote
from fastapi import HTTPException, status
from sqlalchemy.ext.asyncio import AsyncSession

logger = logging.getLogger(__name__)

PRICE_OUT_OF_BAND_CODE = "ORDER_PRICE_OUT_OF_BAND"
QUANTITY_LIMIT_EXCEEDED_CODE = "ORDER_QUANTITY_LIMIT_EXCEEDED"

_CENT = Decimal("0.01")
_PERCENT = Decimal(100)


def check_quantity_limit(
    *,
    quantity_mt: Decimal,
    max_quantity_mt: Decimal,
) -> None:
    """Raise when one order is larger than the configured maximum."""
    if max_quantity_mt <= 0 or quantity_mt <= max_quantity_mt:
        return
    logger.warning(
        "order_rejected_quantity_limit",
        extra={
            "submitted_quantity_mt": str(quantity_mt),
            "max_quantity_mt": str(max_quantity_mt),
        },
    )
    raise HTTPException(
        status_code=status.HTTP_400_BAD_REQUEST,
        detail={
            "code": QUANTITY_LIMIT_EXCEEDED_CODE,
            "message": (
                f"Order quantity {quantity_mt:f} MT exceeds the maximum of "
                f"{max_quantity_mt:f} MT per order"
            ),
            "submitted_quantity_mt": str(quantity_mt),
            "max_quantity_mt": str(max_quantity_mt),
        },
    )


def check_price_band(
    *,
    price_per_mt_usd: Decimal,
    reference_price_per_mt_usd: Decimal | None,
    reference_source: str | None,
    max_deviation_pct: Decimal,
) -> None:
    """Raise when the price is further from the reference than the band.

    A missing (or non-positive) reference means there is nothing to compare
    against, so the order passes.
    """
    if max_deviation_pct <= 0:
        return
    if reference_price_per_mt_usd is None or reference_price_per_mt_usd <= 0:
        return

    # abs(price - ref) / ref * 100 > pct, rearranged to avoid a division.
    distance = abs(price_per_mt_usd - reference_price_per_mt_usd)
    if distance * _PERCENT <= max_deviation_pct * reference_price_per_mt_usd:
        return

    band = reference_price_per_mt_usd * max_deviation_pct / _PERCENT
    # Round the displayed limits inward so each one is itself an accepted price.
    min_allowed = max(
        (reference_price_per_mt_usd - band).quantize(_CENT, rounding=ROUND_CEILING),
        _CENT,
    )
    max_allowed = (reference_price_per_mt_usd + band).quantize(
        _CENT, rounding=ROUND_FLOOR
    )
    logger.warning(
        "order_rejected_price_band",
        extra={
            "submitted_price_per_mt_usd": str(price_per_mt_usd),
            "reference_price_per_mt_usd": str(reference_price_per_mt_usd),
            "max_deviation_pct": str(max_deviation_pct),
        },
    )
    raise HTTPException(
        status_code=status.HTTP_400_BAD_REQUEST,
        detail={
            "code": PRICE_OUT_OF_BAND_CODE,
            "message": (
                f"Price {price_per_mt_usd:f} USD/MT is more than "
                f"{max_deviation_pct:f}% away from the reference price "
                f"{reference_price_per_mt_usd:f} USD/MT; allowed range is "
                f"{min_allowed:f} to {max_allowed:f}"
            ),
            "submitted_price_per_mt_usd": str(price_per_mt_usd),
            "reference_price_per_mt_usd": str(reference_price_per_mt_usd),
            "reference_source": reference_source,
            "max_deviation_pct": str(max_deviation_pct),
            "min_allowed_price_per_mt_usd": str(min_allowed),
            "max_allowed_price_per_mt_usd": str(max_allowed),
        },
    )


async def enforce_order_sanity_limits(
    db: AsyncSession,
    *,
    market_product: str | None,
    delivery_point_id: UUID | None,
    availability_window: str,
    quantity_mt: Decimal | None,
    price_per_mt_usd: Decimal | None,
) -> None:
    """Apply the volume cap and price band to the fields being placed.

    Pass ``None`` for a field that is not being set (an amendment that changes
    only the price must not be rejected for an untouched quantity). Call this
    before acquiring any market slice lock.
    """
    if quantity_mt is not None:
        check_quantity_limit(
            quantity_mt=quantity_mt,
            max_quantity_mt=settings.ORDER_MAX_QUANTITY_MT,
        )

    if price_per_mt_usd is None or settings.ORDER_MAX_PRICE_DEVIATION_PCT <= 0:
        return
    quote = await get_benchmark_quote(
        db,
        market_product=market_product,
        delivery_point_id=delivery_point_id,
        availability_window=availability_window,
    )
    if quote is None:
        return
    check_price_band(
        price_per_mt_usd=price_per_mt_usd,
        reference_price_per_mt_usd=quote.benchmark_price_per_mt_usd,
        reference_source=quote.source,
        max_deviation_pct=settings.ORDER_MAX_PRICE_DEVIATION_PCT,
    )

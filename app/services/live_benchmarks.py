from __future__ import annotations

from decimal import Decimal
from typing import Iterable
from uuid import UUID

from sqlalchemy import select, or_, func
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import selectinload

from app.models.live_slice_benchmark import LiveSliceBenchmark
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.services.availability_windows import normalize_availability_window
from app.services.execution_policy import normalize_certification_scheme, order_is_execution_qualified

APPROVED_MARKETPLACE_FUEL_TYPES = ("Methanol", "Ethanol")
LiveBenchmarkKey = tuple[OrderSide, str | None, UUID | None, str | None]


def _text_present(value: str | None) -> bool:
    return bool((value or "").strip())


def public_slice_order_qualified(order: OrderBookOrder) -> bool:
    if order.product is None or order.product.fuel_type not in APPROVED_MARKETPLACE_FUEL_TYPES:
        return False
    if order.status not in (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED):
        return False
    if order.expires_at is not None:
        from datetime import datetime, UTC
        if order.expires_at <= datetime.now(UTC):
            return False
    if order.remaining_quantity_mt <= 0 or order.off_spec:
        return False
    if order.side != OrderSide.ASK:
        return True
    if not order_is_execution_qualified(order):
        return False
    if not order.certification_declared or normalize_certification_scheme(order.certification_scheme) is None:
        return False
    if not _text_present(order.specification_standard):
        return False
    if not order.msds_available:
        return False
    if order.carbon_intensity_gco2_mj is None:
        return False
    if not _text_present(order.feedstock):
        return False
    if not _text_present(order.origin):
        return False
    return True


def normalize_live_benchmark_key(
    side: OrderSide,
    market_product: str | None,
    delivery_point_id: UUID | None,
    availability_window: str | None,
) -> LiveBenchmarkKey | None:
    if not market_product or delivery_point_id is None or not availability_window:
        return None
    return (side, market_product, delivery_point_id, normalize_availability_window(availability_window))


def live_benchmark_key_for_order(order: OrderBookOrder) -> LiveBenchmarkKey | None:
    return normalize_live_benchmark_key(
        order.side,
        order.market_product,
        order.delivery_point_id,
        order.availability_window,
    )


async def rebuild_live_slice_benchmark(
    db: AsyncSession,
    *,
    side: OrderSide,
    market_product: str | None,
    delivery_point_id: UUID | None,
    availability_window: str | None,
) -> Decimal | None:
    key = normalize_live_benchmark_key(side, market_product, delivery_point_id, availability_window)
    if key is None:
        return None

    normalized_side, normalized_market_product, normalized_delivery_point_id, normalized_window = key
    result = await db.execute(
        select(OrderBookOrder)
        .options(selectinload(OrderBookOrder.product))
        .where(
            OrderBookOrder.side == normalized_side,
            OrderBookOrder.delivery_point_id == normalized_delivery_point_id,
            OrderBookOrder.availability_window == normalized_window,
            OrderBookOrder.status.in_((OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED)),
            OrderBookOrder.remaining_quantity_mt > 0,
            or_(OrderBookOrder.expires_at.is_(None), OrderBookOrder.expires_at > func.now()),
        )
    )
    orders = result.unique().scalars().all()
    qualifying_orders = [
        order for order in orders
        if order.market_product == normalized_market_product and public_slice_order_qualified(order)
    ]

    existing = await db.execute(
        select(LiveSliceBenchmark).where(
            LiveSliceBenchmark.side == normalized_side,
            LiveSliceBenchmark.market_product == normalized_market_product,
            LiveSliceBenchmark.delivery_point_id == normalized_delivery_point_id,
            LiveSliceBenchmark.availability_window == normalized_window,
        )
    )
    record = existing.scalars().first()

    if not qualifying_orders:
        if record is not None:
            await db.delete(record)
            await db.flush()
        return None

    total_qty = sum((order.remaining_quantity_mt for order in qualifying_orders), Decimal("0.00"))
    weighted_sum = sum((order.remaining_quantity_mt * order.price_per_mt_usd for order in qualifying_orders), Decimal("0.00"))
    benchmark_price = (weighted_sum / total_qty).quantize(Decimal("0.01"))

    if record is None:
        record = LiveSliceBenchmark(
            side=normalized_side,
            market_product=normalized_market_product,
            delivery_point_id=normalized_delivery_point_id,
            availability_window=normalized_window,
        )
        db.add(record)

    record.benchmark_price_per_mt_usd = benchmark_price
    record.total_remaining_quantity_mt = total_qty.quantize(Decimal("0.01"))
    record.order_count = len(qualifying_orders)
    record.source = "live_slice_vwap"
    await db.flush()
    return benchmark_price


async def rebuild_live_slice_benchmarks_for_keys(
    db: AsyncSession,
    keys: Iterable[LiveBenchmarkKey | None],
) -> None:
    seen: set[LiveBenchmarkKey] = set()
    for key in keys:
        if key is None or key in seen:
            continue
        seen.add(key)
        await rebuild_live_slice_benchmark(
            db,
            side=key[0],
            market_product=key[1],
            delivery_point_id=key[2],
            availability_window=key[3],
        )


async def get_live_slice_benchmark_price(
    db: AsyncSession,
    *,
    side: OrderSide,
    market_product: str | None,
    delivery_point_id: UUID | None,
    availability_window: str | None,
    cache: dict[LiveBenchmarkKey, Decimal | None] | None = None,
) -> Decimal | None:
    key = normalize_live_benchmark_key(side, market_product, delivery_point_id, availability_window)
    if key is None:
        return None
    if cache is not None and key in cache:
        return cache[key]

    result = await db.execute(
        select(LiveSliceBenchmark.benchmark_price_per_mt_usd).where(
            LiveSliceBenchmark.side == key[0],
            LiveSliceBenchmark.market_product == key[1],
            LiveSliceBenchmark.delivery_point_id == key[2],
            LiveSliceBenchmark.availability_window == key[3],
        )
    )
    benchmark_price = result.scalar_one_or_none()
    if benchmark_price is None:
        benchmark_price = await rebuild_live_slice_benchmark(
            db,
            side=key[0],
            market_product=key[1],
            delivery_point_id=key[2],
            availability_window=key[3],
        )
    if cache is not None:
        cache[key] = benchmark_price
    return benchmark_price


async def rebuild_all_live_slice_benchmarks(db: AsyncSession) -> int:
    result = await db.execute(select(OrderBookOrder).options(selectinload(OrderBookOrder.product)))
    keys = {live_benchmark_key_for_order(order) for order in result.unique().scalars().all()}
    await rebuild_live_slice_benchmarks_for_keys(db, keys)
    return len([key for key in keys if key is not None])

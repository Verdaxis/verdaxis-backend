"""Watchlist event generation helpers."""
from __future__ import annotations

from datetime import datetime, UTC, timedelta
from decimal import Decimal
from uuid import UUID

from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import selectinload

from app.models.orderbook import OrderBookOrder, OrderBookStatus
from app.models.watchlist import (
    WatchlistEvent,
    WatchlistEventType,
    WatchlistTarget,
    WatchlistTargetType,
)
from app.services.availability_windows import normalize_availability_window

BEST_PRICE_MOVE_THRESHOLD_PCT = Decimal('1.0')
BENCHMARK_MOVE_THRESHOLD_PCT = Decimal('1.0')
RETENTION_DAYS = 90


def _normalized_window(value: str | None) -> str | None:
    if not value:
        return None
    return normalize_availability_window(value)




async def _best_slice_price(
    db: AsyncSession,
    *,
    market_product_code: str | None,
    delivery_point_id: UUID | None,
    availability_window_code: str | None,
    side: str | None,
) -> Decimal | None:
    if not market_product_code or not delivery_point_id or not availability_window_code or not side:
        return None

    stmt = (
        select(OrderBookOrder)
        .options(selectinload(OrderBookOrder.product))
        .where(
            OrderBookOrder.delivery_point_id == delivery_point_id,
            OrderBookOrder.availability_window == _normalized_window(availability_window_code),
            OrderBookOrder.side == side,
            OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
            OrderBookOrder.off_spec.is_(False),
        )
    )
    orders = (await db.execute(stmt)).scalars().all()
    prices = [candidate.price_per_mt_usd for candidate in orders if candidate.market_product == market_product_code]
    if not prices:
        return None
    return min(prices) if getattr(side, 'value', side) == 'ASK' else max(prices)


def _best_price_changed(old_price: Decimal | None, new_price: Decimal | None) -> bool:
    return old_price != new_price

def _price_move_is_material(old_price: Decimal | None, new_price: Decimal | None, threshold_pct: Decimal = BEST_PRICE_MOVE_THRESHOLD_PCT) -> bool:
    if old_price is None or new_price is None or old_price <= 0 or new_price <= 0:
        return False
    if old_price == new_price:
        return False
    pct = abs((new_price - old_price) / old_price) * Decimal('100')
    return pct >= threshold_pct


def sync_target_snapshot(target: WatchlistTarget, order: OrderBookOrder, *, counterparty_label: str | None = None) -> None:
    target.market_product_code = order.market_product
    target.delivery_point_id = order.delivery_point_id
    target.availability_window_code = _normalized_window(order.availability_window)
    target.snapshot_price_per_mt_usd = float(order.price_per_mt_usd)
    target.snapshot_quantity_mt = float(order.quantity_mt)
    target.snapshot_remaining_quantity_mt = float(order.remaining_quantity_mt)
    target.snapshot_status = order.status.value if hasattr(order.status, 'value') else str(order.status)
    target.snapshot_side = order.side.value if hasattr(order.side, 'value') else str(order.side)
    target.snapshot_market_product = order.market_product
    target.snapshot_delivery_point_name = order.delivery_point_name
    target.snapshot_availability_window = _normalized_window(order.availability_window)
    target.snapshot_counterparty_label = counterparty_label


async def create_event(
    db: AsyncSession,
    *,
    target: WatchlistTarget,
    event_type: WatchlistEventType,
    payload: dict,
) -> WatchlistEvent:
    event = WatchlistEvent(
        watchlist_id=target.watchlist_id,
        watchlist_target_id=target.id,
        event_type=event_type,
        event_payload=payload,
        is_read=False,
    )
    db.add(event)
    return event


async def _emit_pin_events(
    db: AsyncSession,
    *,
    before: dict,
    order: OrderBookOrder,
) -> None:
    pin_targets = await pin_targets_for_order(db, order.id)
    for target in pin_targets:
        old_price = before.get('price_per_mt_usd')
        new_price = order.price_per_mt_usd
        old_remaining = before.get('remaining_quantity_mt')
        new_remaining = order.remaining_quantity_mt
        old_status = before.get('status')
        new_status = order.status

        if _price_move_is_material(old_price, new_price):
            await create_event(
                db,
                target=target,
                event_type=WatchlistEventType.PIN_PRICE_CHANGED,
                payload={'old_price_per_mt_usd': float(old_price), 'new_price_per_mt_usd': float(new_price)},
            )
        if old_remaining is not None and new_remaining != old_remaining:
            event_type = WatchlistEventType.PIN_QUANTITY_CHANGED
            if new_status == OrderBookStatus.PARTIALLY_FILLED:
                event_type = WatchlistEventType.PIN_PARTIALLY_FILLED
            elif new_status == OrderBookStatus.FILLED:
                event_type = WatchlistEventType.PIN_FILLED
            elif new_status == OrderBookStatus.CANCELLED:
                event_type = WatchlistEventType.PIN_WITHDRAWN
            elif new_status == OrderBookStatus.EXPIRED:
                event_type = WatchlistEventType.PIN_EXPIRED
            await create_event(
                db,
                target=target,
                event_type=event_type,
                payload={
                    'old_remaining_quantity_mt': float(old_remaining),
                    'new_remaining_quantity_mt': float(new_remaining),
                    'status': new_status.value,
                },
            )
        elif old_status != new_status and new_status in {OrderBookStatus.FILLED, OrderBookStatus.CANCELLED, OrderBookStatus.EXPIRED}:
            event_type = {
                OrderBookStatus.FILLED: WatchlistEventType.PIN_FILLED,
                OrderBookStatus.CANCELLED: WatchlistEventType.PIN_WITHDRAWN,
                OrderBookStatus.EXPIRED: WatchlistEventType.PIN_EXPIRED,
            }[new_status]
            await create_event(
                db,
                target=target,
                event_type=event_type,
                payload={'status': new_status.value},
            )
        sync_target_snapshot(target, order)


async def emit_pin_updated(
    db: AsyncSession,
    *,
    before: dict,
    order: OrderBookOrder,
) -> None:
    await db.flush()
    await _emit_pin_events(db, before=before, order=order)


async def emit_slice_state_changed(
    db: AsyncSession,
    *,
    market_product_code: str | None,
    delivery_point_id: UUID | None,
    availability_window_code: str | None,
    side: str | None,
    before_best_price: Decimal | None,
    quiet_order_id: UUID | None = None,
) -> None:
    if not market_product_code or not delivery_point_id or not availability_window_code or not side:
        return

    window = _normalized_window(availability_window_code)
    after_best_price = await _best_slice_price(
        db,
        market_product_code=market_product_code,
        delivery_point_id=delivery_point_id,
        availability_window_code=window,
        side=side,
    )

    stmt = select(WatchlistTarget).where(
        WatchlistTarget.target_type == WatchlistTargetType.SLICE,
        WatchlistTarget.market_product_code == market_product_code,
        WatchlistTarget.delivery_point_id == delivery_point_id,
        WatchlistTarget.availability_window_code == window,
    )
    targets = (await db.execute(stmt)).scalars().all()
    for target in targets:
        if _best_price_changed(before_best_price, after_best_price):
            await create_event(
                db,
                target=target,
                event_type=WatchlistEventType.SLICE_BEST_PRICE_MOVED,
                payload={
                    'side': getattr(side, 'value', side),
                    'old_price_per_mt_usd': float(before_best_price) if before_best_price is not None else None,
                    'new_price_per_mt_usd': float(after_best_price) if after_best_price is not None else None,
                },
            )
        if quiet_order_id is not None and before_best_price is not None and after_best_price is None:
            await create_event(
                db,
                target=target,
                event_type=WatchlistEventType.SLICE_WENT_QUIET,
                payload={'order_id': str(quiet_order_id)},
            )


async def matching_slice_targets(db: AsyncSession, order: OrderBookOrder) -> list[WatchlistTarget]:
    market_product = order.market_product
    window = _normalized_window(order.availability_window)
    if not market_product or not order.delivery_point_id or not window:
        return []
    stmt = select(WatchlistTarget).where(
        WatchlistTarget.target_type == WatchlistTargetType.SLICE,
        WatchlistTarget.market_product_code == market_product,
        WatchlistTarget.delivery_point_id == order.delivery_point_id,
        WatchlistTarget.availability_window_code == window,
    )
    return list((await db.execute(stmt)).scalars().all())


async def pin_targets_for_order(db: AsyncSession, order_id: UUID) -> list[WatchlistTarget]:
    stmt = select(WatchlistTarget).where(
        WatchlistTarget.target_type == WatchlistTargetType.PIN,
        WatchlistTarget.order_id == order_id,
    )
    return list((await db.execute(stmt)).scalars().all())


async def emit_order_created(db: AsyncSession, order: OrderBookOrder, *, previous_best_price: Decimal | None = None) -> None:
    if order.off_spec or order.status not in {OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED}:
        return
    current_best_price = await _best_slice_price(
        db,
        market_product_code=order.market_product,
        delivery_point_id=order.delivery_point_id,
        availability_window_code=order.availability_window,
        side=order.side,
    )
    for target in await matching_slice_targets(db, order):
        await create_event(
            db,
            target=target,
            event_type=WatchlistEventType.SLICE_NEW_ORDER,
            payload={
                'order_id': str(order.id),
                'side': order.side.value,
                'price_per_mt_usd': float(order.price_per_mt_usd),
                'remaining_quantity_mt': float(order.remaining_quantity_mt),
            },
        )
        if _best_price_changed(previous_best_price, current_best_price):
            await create_event(
                db,
                target=target,
                event_type=WatchlistEventType.SLICE_BEST_PRICE_MOVED,
                payload={
                    'side': order.side.value,
                    'old_price_per_mt_usd': float(previous_best_price) if previous_best_price is not None else None,
                    'new_price_per_mt_usd': float(current_best_price) if current_best_price is not None else None,
                },
            )


async def emit_benchmark_moved(
    db: AsyncSession,
    *,
    market_product_code: str,
    delivery_point_id: UUID,
    availability_window_code: str,
    old_price: Decimal,
    new_price: Decimal,
) -> None:
    if not _price_move_is_material(old_price, new_price, BENCHMARK_MOVE_THRESHOLD_PCT):
        return
    stmt = select(WatchlistTarget).where(
        WatchlistTarget.target_type == WatchlistTargetType.SLICE,
        WatchlistTarget.market_product_code == market_product_code,
        WatchlistTarget.delivery_point_id == delivery_point_id,
        WatchlistTarget.availability_window_code == _normalized_window(availability_window_code),
    )
    targets = (await db.execute(stmt)).scalars().all()
    for target in targets:
        await create_event(
            db,
            target=target,
            event_type=WatchlistEventType.SLICE_BENCHMARK_MOVED,
            payload={
                'old_price_per_mt_usd': float(old_price),
                'new_price_per_mt_usd': float(new_price),
            },
        )


async def emit_order_updated(
    db: AsyncSession,
    *,
    before: dict,
    order: OrderBookOrder,
) -> None:
    await db.flush()
    await _emit_pin_events(db, before=before, order=order)
    await emit_slice_state_changed(
        db,
        market_product_code=order.market_product,
        delivery_point_id=order.delivery_point_id,
        availability_window_code=order.availability_window,
        side=order.side,
        before_best_price=before.get('slice_best_price_per_mt_usd'),
        quiet_order_id=order.id if order.status in {OrderBookStatus.FILLED, OrderBookStatus.CANCELLED, OrderBookStatus.EXPIRED} else None,
    )


async def prune_old_read_events(db: AsyncSession, *, now: datetime | None = None) -> int:
    now = now or datetime.now(UTC)
    cutoff = now - timedelta(days=RETENTION_DAYS)
    stmt = select(WatchlistEvent).where(
        WatchlistEvent.is_read.is_(True),
        WatchlistEvent.created_at < cutoff,
    )
    rows = (await db.execute(stmt)).scalars().all()
    for row in rows:
        await db.delete(row)
    return len(rows)

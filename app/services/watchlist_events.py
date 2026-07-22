"""Watchlist event generation helpers."""
from __future__ import annotations

from datetime import datetime, UTC, timedelta
from decimal import Decimal
from uuid import UUID

from sqlalchemy import case, func, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.catalog import DeliveryPoint, Product
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.watchlist import (
    WatchlistEvent,
    WatchlistEventType,
    WatchlistTarget,
    WatchlistTargetType,
)
from app.schemas.market_activity import MarketDemoStatus, MarketScope, MarketSourceKind
from app.services.availability_windows import normalize_availability_window
from app.models.user import OrganizationProvenance
from app.services.market_data_eligibility import (
    active_market_catalog_clauses,
    canonical_market_product_expression,
    current_public_order_clause,
)
from app.services.market_provenance import order_market_provenance, select_aggregate_evidence
from app.services.execution_policy import order_is_execution_qualified

BEST_PRICE_MOVE_THRESHOLD_PCT = Decimal('1.0')
BENCHMARK_MOVE_THRESHOLD_PCT = Decimal('1.0')
RETENTION_DAYS = 90


def _normalized_window(value: str | None) -> str | None:
    if not value:
        return None
    return normalize_availability_window(value)




async def _best_slice_price_with_provenance(
    db: AsyncSession,
    *,
    market_product_code: str | None,
    delivery_point_id: UUID | None,
    availability_window_code: str | None,
    side: str | None,
) -> tuple[Decimal | None, dict]:
    if not market_product_code or not delivery_point_id or not availability_window_code or not side:
        return None, _order_provenance_payload(None)

    side_value = getattr(side, "value", side)
    real_clause = OrderBookOrder.provenance == OrganizationProvenance.REAL.value
    demo_clause = OrderBookOrder.provenance == OrganizationProvenance.DEMO.value
    unknown_clause = OrderBookOrder.provenance == OrganizationProvenance.UNKNOWN.value
    observed_at = func.coalesce(OrderBookOrder.updated_at, OrderBookOrder.created_at)
    best = func.min if side_value == OrderSide.ASK.value else func.max
    execution_filters = [OrderBookOrder.off_spec.is_(False)]
    if side_value == OrderSide.ASK.value:
        execution_filters.extend(
            (
                OrderBookOrder.certification_declared.is_(True),
                func.length(func.trim(OrderBookOrder.certification_scheme)) > 0,
            )
        )
    stmt = (
        select(
            best(case((real_clause, OrderBookOrder.price_per_mt_usd))).label("real_price"),
            best(case((demo_clause, OrderBookOrder.price_per_mt_usd))).label("demo_price"),
            func.sum(case((real_clause, 1), else_=0)).label("real_count"),
            func.sum(case((demo_clause, 1), else_=0)).label("demo_count"),
            func.sum(case((unknown_clause, 1), else_=0)).label("unknown_count"),
            func.max(case((real_clause, observed_at))).label("real_observed_at"),
            func.max(case((demo_clause, observed_at))).label("demo_observed_at"),
        )
        .join(Product, OrderBookOrder.product_id == Product.id)
        .join(DeliveryPoint, OrderBookOrder.delivery_point_id == DeliveryPoint.id)
        .where(
            *active_market_catalog_clauses(Product, DeliveryPoint),
            canonical_market_product_expression(Product) == market_product_code,
            OrderBookOrder.delivery_point_id == delivery_point_id,
            OrderBookOrder.availability_window == _normalized_window(availability_window_code),
            OrderBookOrder.side == side_value,
            OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
            current_public_order_clause(OrderBookOrder),
            OrderBookOrder.provenance.in_(
                (
                    OrganizationProvenance.REAL.value,
                    OrganizationProvenance.DEMO.value,
                    OrganizationProvenance.UNKNOWN.value,
                )
            ),
            *execution_filters,
        )
    )
    row = (await db.execute(stmt)).one()
    selection = select_aggregate_evidence(
        real_count=int(row.real_count or 0),
        demo_count=int(row.demo_count or 0),
        unknown_count=int(row.unknown_count or 0),
        real_source=MarketSourceKind.LIVE_ORDER,
    )
    best_price = (
        row.real_price
        if selection.value_prefix == "real"
        else row.demo_price
        if selection.value_prefix == "demo"
        else None
    )
    selected_observed_at = (
        row.real_observed_at
        if selection.value_prefix == "real"
        else row.demo_observed_at
        if selection.value_prefix == "demo"
        else None
    )
    return best_price, {
        "source_kind": selection.source_kind.value,
        "demo_status": selection.demo_status.value,
        "scope": MarketScope.DELIVERY_POINT.value,
        "observed_at": selected_observed_at.isoformat() if selected_observed_at else None,
        "market_product_code": market_product_code,
        "delivery_point_id": str(delivery_point_id),
        "availability_window_code": _normalized_window(availability_window_code),
        "real_count": selection.real_count,
        "demo_count": selection.demo_count,
        "unknown_count": selection.unknown_count,
    }


async def _best_slice_price(
    db: AsyncSession,
    *,
    market_product_code: str | None,
    delivery_point_id: UUID | None,
    availability_window_code: str | None,
    side: str | None,
) -> Decimal | None:
    price, _ = await _best_slice_price_with_provenance(
        db,
        market_product_code=market_product_code,
        delivery_point_id=delivery_point_id,
        availability_window_code=availability_window_code,
        side=side,
    )
    return price


def _best_price_changed(old_price: Decimal | None, new_price: Decimal | None) -> bool:
    return old_price != new_price

def _price_move_is_material(old_price: Decimal | None, new_price: Decimal | None, threshold_pct: Decimal = BEST_PRICE_MOVE_THRESHOLD_PCT) -> bool:
    if old_price is None or new_price is None or old_price <= 0 or new_price <= 0:
        return False
    if old_price == new_price:
        return False
    pct = abs((new_price - old_price) / old_price) * Decimal('100')
    return pct >= threshold_pct


def _order_provenance_payload(order: OrderBookOrder | None) -> dict:
    if order is None:
        return {
            'source_kind': MarketSourceKind.UNKNOWN.value,
            'demo_status': MarketDemoStatus.UNKNOWN.value,
            'scope': MarketScope.UNKNOWN.value,
            'observed_at': None,
        }
    policy = order_market_provenance(order)
    return {
        **policy,
        'observed_at': (order.updated_at or order.created_at).isoformat() if (order.updated_at or order.created_at) else None,
        'market_product_code': order.market_product,
        'delivery_point_id': str(order.delivery_point_id) if order.delivery_point_id else None,
        'delivery_point_name': order.delivery_point_name,
        'availability_window_code': _normalized_window(order.availability_window),
        'order_id': str(order.id),
    }


def _benchmark_provenance_payload(benchmark_source: str | None) -> dict:
    if benchmark_source == 'seed_matrix':
        source_kind = MarketSourceKind.DEMO_SEED
        demo_status = MarketDemoStatus.DEMO_ONLY
    elif benchmark_source:
        source_kind = MarketSourceKind.BENCHMARK_REFERENCE
        demo_status = MarketDemoStatus.NOT_APPLICABLE
    else:
        source_kind = MarketSourceKind.UNKNOWN
        demo_status = MarketDemoStatus.UNKNOWN
    return {
        'source_kind': source_kind.value,
        'demo_status': demo_status.value,
        'scope': MarketScope.DELIVERY_POINT.value,
        'observed_at': None,
    }


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
                payload={
                    **_order_provenance_payload(order),
                    'old_price_per_mt_usd': float(old_price),
                    'new_price_per_mt_usd': float(new_price),
                },
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
                    **_order_provenance_payload(order),
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
                payload={**_order_provenance_payload(order), 'status': new_status.value},
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
    source_order: OrderBookOrder | None = None,
) -> None:
    if not market_product_code or not delivery_point_id or not availability_window_code or not side:
        return

    window = _normalized_window(availability_window_code)
    after_best_price, after_best_provenance = await _best_slice_price_with_provenance(
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
                    **after_best_provenance,
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
                payload={**_order_provenance_payload(source_order), 'order_id': str(quiet_order_id)},
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
    if order.status not in {OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED} or not order_is_execution_qualified(order):
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
                **_order_provenance_payload(order),
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
                    **_order_provenance_payload(order),
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
    benchmark_source: str | None = None,
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
                **_benchmark_provenance_payload(benchmark_source),
                'old_price_per_mt_usd': float(old_price),
                'new_price_per_mt_usd': float(new_price),
                'market_product_code': market_product_code,
                'delivery_point_id': str(delivery_point_id),
                'availability_window_code': _normalized_window(availability_window_code),
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
        source_order=order,
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

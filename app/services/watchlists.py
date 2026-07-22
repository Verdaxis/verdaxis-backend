"""Watchlist service helpers for market-radar containers, targets, and feeds."""
from __future__ import annotations

from collections import defaultdict
from datetime import datetime, UTC
from uuid import UUID

from sqlalchemy import and_, func, or_, select
from sqlalchemy.exc import IntegrityError
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import selectinload

from app.models.catalog import DeliveryPoint, Product
from app.models.orderbook import OrderBookOrder, OrderBookStatus
from app.models.watchlist import (
    Watchlist,
    WatchlistEvent,
    WatchlistKind,
    WatchlistTarget,
    WatchlistTargetType,
)
from app.schemas.market_activity import MarketDemoStatus, MarketScope, MarketSourceKind
from app.schemas.watchlist import (
    WatchlistDetailResponse,
    WatchlistEventResponse,
    WatchlistEventsPageResponse,
    WatchlistSliceResponse,
    WatchlistSummaryResponse,
    WatchlistTargetResponse,
)
from app.services.availability_windows import normalize_availability_window
from app.services.execution_policy import order_is_execution_qualified

DEFAULT_WATCHLIST_NAME = "Market Radar"
DEFAULT_EVENT_PAGE_SIZE = 20
MAX_EVENT_PAGE_SIZE = 100
MAX_SLICE_COUNT = 100


def encode_event_cursor(created_at: datetime, event_id: UUID) -> str:
    return f"{created_at.astimezone(UTC).isoformat()}|{event_id}"


def decode_event_cursor(cursor: str) -> tuple[datetime, UUID]:
    try:
        ts, raw_id = cursor.split("|", 1)
        return datetime.fromisoformat(ts), UUID(raw_id)
    except (ValueError, TypeError, AttributeError) as exc:
        raise ValueError("Invalid watchlist event cursor") from exc


def _coerce_uuid(value: object) -> UUID | None:
    if isinstance(value, UUID):
        return value
    if not value:
        return None
    try:
        return UUID(str(value))
    except (TypeError, ValueError):
        return None


def build_watchlist_event_response(
    event: WatchlistEvent,
    target: WatchlistTarget,
    *,
    delivery_point_name: str | None = None,
) -> WatchlistEventResponse:
    payload = event.event_payload or {}
    source_kind = payload.get("source_kind") or MarketSourceKind.UNKNOWN.value
    demo_status = payload.get("demo_status") or MarketDemoStatus.UNKNOWN.value
    scope = payload.get("scope") or (
        MarketScope.DELIVERY_POINT.value if target.delivery_point_id else MarketScope.UNKNOWN.value
    )
    return WatchlistEventResponse(
        id=event.id,
        watchlist_id=event.watchlist_id,
        watchlist_target_id=event.watchlist_target_id,
        target_type=target.target_type.value if hasattr(target.target_type, 'value') else str(target.target_type),
        event_type=event.event_type.value if hasattr(event.event_type, 'value') else str(event.event_type),
        event_payload=payload,
        source_kind=source_kind,
        scope=scope,
        demo_status=demo_status,
        observed_at=payload.get("observed_at"),
        market_product_code=payload.get("market_product_code") or target.market_product_code,
        delivery_point_id=_coerce_uuid(payload.get("delivery_point_id")) or target.delivery_point_id,
        delivery_point_name=payload.get("delivery_point_name") or delivery_point_name or target.snapshot_delivery_point_name,
        availability_window_code=payload.get("availability_window_code") or target.availability_window_code,
        order_id=_coerce_uuid(payload.get("order_id")) or target.order_id,
        is_read=event.is_read,
        created_at=event.created_at,
    )


async def ensure_market_radar(db: AsyncSession, user_id: UUID) -> Watchlist:
    stmt = (
        select(Watchlist)
        .where(Watchlist.user_id == user_id, Watchlist.kind == WatchlistKind.RADAR_DEFAULT)
        .options(
            selectinload(Watchlist.targets).selectinload(WatchlistTarget.delivery_point),
            selectinload(Watchlist.targets).selectinload(WatchlistTarget.order),
        )
    )
    existing = (await db.execute(stmt)).scalars().first()
    if existing:
        return existing

    radar = Watchlist(user_id=user_id, name=DEFAULT_WATCHLIST_NAME, kind=WatchlistKind.RADAR_DEFAULT)
    db.add(radar)
    try:
        await db.flush()
    except IntegrityError:
        await db.rollback()
        existing = (await db.execute(stmt)).scalars().first()
        if existing:
            return existing
        raise
    await db.refresh(radar)
    return radar


async def load_watchlist_or_404(db: AsyncSession, watchlist_id: UUID, user_id: UUID) -> Watchlist | None:
    stmt = (
        select(Watchlist)
        .where(Watchlist.id == watchlist_id, Watchlist.user_id == user_id)
        .options(
            selectinload(Watchlist.targets).selectinload(WatchlistTarget.delivery_point),
            selectinload(Watchlist.targets).selectinload(WatchlistTarget.order),
        )
    )
    return (await db.execute(stmt)).scalars().first()


async def _load_target_metrics(db: AsyncSession, targets: list[WatchlistTarget]) -> tuple[dict[UUID, int], dict[UUID, int], dict[UUID, datetime | None]]:
    if not targets:
        return {}, {}, {}

    target_ids = [target.id for target in targets]
    unread_stmt = (
        select(WatchlistEvent.watchlist_target_id, func.count(WatchlistEvent.id))
        .where(WatchlistEvent.watchlist_target_id.in_(target_ids), WatchlistEvent.is_read.is_(False))
        .group_by(WatchlistEvent.watchlist_target_id)
    )
    latest_stmt = (
        select(WatchlistEvent.watchlist_target_id, func.max(WatchlistEvent.created_at))
        .where(WatchlistEvent.watchlist_target_id.in_(target_ids))
        .group_by(WatchlistEvent.watchlist_target_id)
    )

    unread_counts = {row[0]: int(row[1]) for row in (await db.execute(unread_stmt)).all()}
    latest_events = {row[0]: row[1] for row in (await db.execute(latest_stmt)).all()}

    slice_targets = [target for target in targets if target.target_type == WatchlistTargetType.SLICE]
    if not slice_targets:
        return {}, unread_counts, latest_events

    combos = {
        (
            target.market_product_code,
            target.delivery_point_id,
            normalize_availability_window(target.availability_window_code or "SPOT"),
        )
        for target in slice_targets
        if target.market_product_code and target.delivery_point_id and target.availability_window_code
    }
    if not combos:
        return {}, unread_counts, latest_events

    open_orders_stmt = (
        select(OrderBookOrder)
        .options(selectinload(OrderBookOrder.product))
        .where(
            OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
            OrderBookOrder.expires_at.is_(None) | (OrderBookOrder.expires_at > func.now()),
        )
    )
    active_counts: dict[UUID, int] = defaultdict(int)
    orders = (await db.execute(open_orders_stmt)).scalars().all()
    combo_to_target = {
        (
            target.market_product_code,
            target.delivery_point_id,
            normalize_availability_window(target.availability_window_code or "SPOT"),
        ): target.id
        for target in slice_targets
        if target.market_product_code and target.delivery_point_id and target.availability_window_code
    }
    for order in orders:
        market_product = order.market_product
        if not market_product or order.delivery_point_id is None or not order_is_execution_qualified(order):
            continue
        key = (market_product, order.delivery_point_id, normalize_availability_window(order.availability_window))
        target_id = combo_to_target.get(key)
        if target_id:
            active_counts[target_id] += 1

    return dict(active_counts), unread_counts, latest_events


def _pin_response(
    target: WatchlistTarget,
    *,
    unread_event_count: int,
    latest_event_at: datetime | None,
) -> WatchlistTargetResponse:
    return WatchlistTargetResponse(
        id=target.id,
        target_type="PIN",
        market_product_code=target.market_product_code,
        delivery_point_id=target.delivery_point_id,
        delivery_point_name=target.delivery_point.name if target.delivery_point else target.snapshot_delivery_point_name,
        availability_window_code=target.availability_window_code,
        order_id=target.order_id,
        snapshot_price_per_mt_usd=target.snapshot_price_per_mt_usd,
        snapshot_quantity_mt=target.snapshot_quantity_mt,
        snapshot_remaining_quantity_mt=target.snapshot_remaining_quantity_mt,
        snapshot_status=target.snapshot_status,
        snapshot_side=target.snapshot_side,
        snapshot_market_product=target.snapshot_market_product,
        snapshot_delivery_point_name=target.snapshot_delivery_point_name,
        snapshot_availability_window=target.snapshot_availability_window,
        snapshot_counterparty_label=target.snapshot_counterparty_label,
        active_order_count=0,
        unread_event_count=unread_event_count,
        latest_event_at=latest_event_at,
        created_at=target.created_at,
    )


async def build_watchlist_summary(db: AsyncSession, watchlist: Watchlist) -> WatchlistSummaryResponse:
    targets = sorted(watchlist.targets, key=lambda item: item.created_at, reverse=True)
    active_counts, unread_counts, latest_events = await _load_target_metrics(db, targets)

    pin_groups: dict[tuple[str, UUID, str], list[WatchlistTargetResponse]] = defaultdict(list)
    slice_responses: list[WatchlistSliceResponse] = []

    for target in targets:
        if target.target_type == WatchlistTargetType.PIN and target.market_product_code and target.delivery_point_id and target.availability_window_code:
            key = (
                target.market_product_code,
                target.delivery_point_id,
                normalize_availability_window(target.availability_window_code),
            )
            pin_groups[key].append(
                _pin_response(
                    target,
                    unread_event_count=unread_counts.get(target.id, 0),
                    latest_event_at=latest_events.get(target.id),
                )
            )

    for group in pin_groups.values():
        group.sort(
            key=lambda item: (item.latest_event_at or datetime.min.replace(tzinfo=UTC), item.created_at),
            reverse=True,
        )

    for target in targets:
        if target.target_type != WatchlistTargetType.SLICE:
            continue
        if not target.market_product_code or not target.delivery_point_id or not target.availability_window_code:
            continue
        key = (
            target.market_product_code,
            target.delivery_point_id,
            normalize_availability_window(target.availability_window_code),
        )
        slice_responses.append(
            WatchlistSliceResponse(
                id=target.id,
                market_product_code=target.market_product_code,
                delivery_point_id=target.delivery_point_id,
                delivery_point_name=target.delivery_point.name if target.delivery_point else target.snapshot_delivery_point_name,
                availability_window_code=target.availability_window_code,
                active_order_count=active_counts.get(target.id, 0),
                unread_event_count=unread_counts.get(target.id, 0) + sum(pin.unread_event_count for pin in pin_groups.get(key, [])),
                latest_event_at=max(
                    [value for value in [latest_events.get(target.id)] + [pin.latest_event_at for pin in pin_groups.get(key, [])] if value],
                    default=None,
                ),
                pins=pin_groups.get(key, []),
                created_at=target.created_at,
            )
        )

    slice_responses.sort(key=lambda item: (item.unread_event_count > 0, item.latest_event_at or datetime.min.replace(tzinfo=UTC), item.created_at), reverse=True)
    total_slice_count = len(slice_responses)
    latest_event_at = max([item.latest_event_at for item in slice_responses if item.latest_event_at], default=None)
    unread_event_count = sum(slice_item.unread_event_count for slice_item in slice_responses)
    has_more_slices = total_slice_count > MAX_SLICE_COUNT
    slice_responses = slice_responses[:MAX_SLICE_COUNT]

    return WatchlistSummaryResponse(
        id=watchlist.id,
        name=watchlist.name,
        kind=watchlist.kind.value if hasattr(watchlist.kind, 'value') else str(watchlist.kind),
        unread_event_count=unread_event_count,
        latest_event_at=latest_event_at,
        total_slice_count=total_slice_count,
        has_more_slices=has_more_slices,
        slices=slice_responses,
        created_at=watchlist.created_at,
    )


async def build_watchlist_detail(db: AsyncSession, watchlist: Watchlist) -> WatchlistDetailResponse:
    summary = await build_watchlist_summary(db, watchlist)
    return WatchlistDetailResponse(**summary.model_dump())


async def list_watchlist_events(
    db: AsyncSession,
    watchlist_id: UUID,
    *,
    cursor: str | None,
    limit: int,
) -> WatchlistEventsPageResponse:
    limit = max(1, min(limit, MAX_EVENT_PAGE_SIZE))
    stmt = (
        select(WatchlistEvent, WatchlistTarget, DeliveryPoint.name)
        .join(WatchlistTarget, WatchlistEvent.watchlist_target_id == WatchlistTarget.id)
        .outerjoin(DeliveryPoint, WatchlistTarget.delivery_point_id == DeliveryPoint.id)
        .where(WatchlistEvent.watchlist_id == watchlist_id)
    )
    if cursor:
        cursor_created_at, cursor_id = decode_event_cursor(cursor)
        stmt = stmt.where(
            or_(
                WatchlistEvent.created_at < cursor_created_at,
                and_(WatchlistEvent.created_at == cursor_created_at, WatchlistEvent.id < cursor_id),
            )
        )
    stmt = stmt.order_by(WatchlistEvent.created_at.desc(), WatchlistEvent.id.desc()).limit(limit + 1)
    rows = (await db.execute(stmt)).all()
    next_cursor = None
    if len(rows) > limit:
        last_event, _, _ = rows[limit - 1]
        next_cursor = encode_event_cursor(last_event.created_at, last_event.id)
        rows = rows[:limit]

    items = [
        build_watchlist_event_response(event, target, delivery_point_name=delivery_point_name)
        for event, target, delivery_point_name in rows
    ]
    return WatchlistEventsPageResponse(items=items, next_cursor=next_cursor)

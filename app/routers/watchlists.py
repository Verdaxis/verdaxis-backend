"""Watchlist CRUD endpoints for legacy entries plus slice-first market radar."""
from datetime import datetime, UTC
from typing import Annotated
from uuid import UUID, uuid4

from fastapi import APIRouter, Depends, HTTPException, Query, Response, status
from sqlalchemy import func, select
from sqlalchemy.exc import IntegrityError
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import selectinload

from app.database import get_db
from app.models.catalog import DeliveryPoint, MarketProduct, Product
from app.models.orderbook import OrderBookOrder
from app.models.user import User
from app.models.watchlist import (
    Watchlist,
    WatchlistEntry,
    WatchlistEvent,
    WatchlistKind,
    WatchlistTarget,
    WatchlistTargetType,
)
from app.routers.auth_simple import get_current_user
from app.schemas.watchlist import (
    PinTargetCreate,
    SliceTargetCreate,
    WatchlistTargetCreate,
    WatchlistCreateRequest,
    WatchlistDetailResponse,
    WatchlistEntryAddRequest,
    WatchlistEntryResponse,
    WatchlistEventResponse,
    WatchlistEventsPageResponse,
    WatchlistResponse,
    WatchlistSummaryResponse,
    WatchlistTargetResponse,
)
from app.services.availability_windows import SPOT_WINDOW, normalize_availability_window
from app.services.watchlist_events import sync_target_snapshot
from app.services.watchlists import (
    build_watchlist_detail,
    build_watchlist_summary,
    ensure_market_radar,
    list_watchlist_events,
    load_watchlist_or_404,
)

router = APIRouter(prefix="/watchlists", tags=["watchlists"])

MAX_WATCHLISTS_PER_USER = 10
MAX_ENTRIES_PER_WATCHLIST = 50
MAX_EVENT_LIMIT = 100


def _build_entry_response(entry: WatchlistEntry, product_name: str, delivery_point_name: str | None):
    return WatchlistEntryResponse(
        id=entry.id or uuid4(),
        product_id=entry.product_id,
        product_name=product_name,
        delivery_point_id=entry.delivery_point_id,
        delivery_point_name=delivery_point_name,
        created_at=entry.created_at or datetime.now(UTC),
    )


def _watchlist_target_response(target: WatchlistTarget) -> WatchlistTargetResponse:
    return WatchlistTargetResponse(
        id=target.id,
        target_type=target.target_type.value,
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
        unread_event_count=0,
        latest_event_at=None,
        created_at=target.created_at,
    )


def _validate_market_product_code(value: str) -> str:
    valid = {member.value for member in MarketProduct}
    if value not in valid:
        raise HTTPException(status_code=status.HTTP_422_UNPROCESSABLE_ENTITY, detail="Invalid market_product_code")
    return value


@router.get("", response_model=list[WatchlistResponse])
async def list_watchlists(
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    """Legacy list endpoint retained for adapter compatibility."""
    stmt = (
        select(Watchlist)
        .where(Watchlist.user_id == current_user.id)
        .options(selectinload(Watchlist.entries))
        .order_by(Watchlist.created_at.desc())
    )
    result = await db.execute(stmt)
    watchlists = result.scalars().all()

    responses = []
    for wl in watchlists:
        entry_responses = []
        if wl.entries:
            product_ids = [e.product_id for e in wl.entries]
            dp_ids = [e.delivery_point_id for e in wl.entries if e.delivery_point_id]

            products_map = {}
            if product_ids:
                prod_result = await db.execute(select(Product.id, Product.name).where(Product.id.in_(product_ids)))
                products_map = dict(prod_result.all())

            dp_map = {}
            if dp_ids:
                dp_result = await db.execute(select(DeliveryPoint.id, DeliveryPoint.name).where(DeliveryPoint.id.in_(dp_ids)))
                dp_map = dict(dp_result.all())

            for entry in wl.entries:
                entry_responses.append(
                    WatchlistEntryResponse(
                        id=entry.id,
                        product_id=entry.product_id,
                        product_name=products_map.get(entry.product_id),
                        delivery_point_id=entry.delivery_point_id,
                        delivery_point_name=dp_map.get(entry.delivery_point_id) if entry.delivery_point_id else None,
                        created_at=entry.created_at,
                    )
                )

        responses.append(
            WatchlistResponse(
                id=wl.id,
                name=wl.name,
                entry_count=len(wl.entries),
                entries=entry_responses,
                created_at=wl.created_at,
            )
        )

    return responses


@router.get("/me", response_model=WatchlistSummaryResponse)
async def get_market_radar(
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    radar = await ensure_market_radar(db, current_user.id)
    await db.commit()
    radar = await load_watchlist_or_404(db, radar.id, current_user.id)
    if radar is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Watchlist not found")
    return await build_watchlist_summary(db, radar)


@router.post("", response_model=WatchlistResponse, status_code=201)
async def create_watchlist(
    body: WatchlistCreateRequest,
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    """Legacy custom watchlist create retained temporarily."""
    count_stmt = select(func.count(Watchlist.id)).where(Watchlist.user_id == current_user.id)
    current_count = (await db.execute(count_stmt)).scalar_one()
    if current_count >= MAX_WATCHLISTS_PER_USER:
        raise HTTPException(status_code=400, detail=f"Maximum of {MAX_WATCHLISTS_PER_USER} watchlists allowed")

    wl = Watchlist(user_id=current_user.id, name=body.name, kind=WatchlistKind.CUSTOM)
    db.add(wl)
    await db.commit()
    await db.refresh(wl)
    return WatchlistResponse(id=wl.id, name=wl.name, entry_count=0, entries=[], created_at=wl.created_at)


@router.get("/{watchlist_id}", response_model=WatchlistDetailResponse)
async def get_watchlist_detail(
    watchlist_id: UUID,
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    watchlist = await load_watchlist_or_404(db, watchlist_id, current_user.id)
    if watchlist is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Watchlist not found")
    return await build_watchlist_detail(db, watchlist)


@router.post("/{watchlist_id}/targets", response_model=WatchlistTargetResponse, status_code=201)
async def create_watchlist_target(
    watchlist_id: UUID,
    body: WatchlistTargetCreate,
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    watchlist = await load_watchlist_or_404(db, watchlist_id, current_user.id)
    if watchlist is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Watchlist not found")

    target: WatchlistTarget
    if isinstance(body, SliceTargetCreate):
        dp = await db.get(DeliveryPoint, body.delivery_point_id)
        if not dp:
            raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Delivery point not found")
        try:
            normalized_window = normalize_availability_window(body.availability_window_code)
        except ValueError as exc:
            raise HTTPException(status_code=status.HTTP_422_UNPROCESSABLE_ENTITY, detail=str(exc)) from exc
        target = WatchlistTarget(
            watchlist_id=watchlist.id,
            target_type=WatchlistTargetType.SLICE,
            market_product_code=_validate_market_product_code(body.market_product_code),
            delivery_point_id=body.delivery_point_id,
            availability_window_code=normalized_window,
            snapshot_market_product=body.market_product_code,
            snapshot_delivery_point_name=dp.name,
            snapshot_availability_window=normalized_window,
        )
    else:
        order_stmt = (
            select(OrderBookOrder)
            .options(selectinload(OrderBookOrder.product), selectinload(OrderBookOrder.delivery_point))
            .where(OrderBookOrder.id == body.order_id)
        )
        order = (await db.execute(order_stmt)).scalars().first()
        if order is None:
            raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Order not found")
        if not order.market_product or not order.delivery_point_id or not order.availability_window:
            raise HTTPException(status_code=status.HTTP_422_UNPROCESSABLE_ENTITY, detail="Pinned order does not map to a canonical slice")
        normalized_window = normalize_availability_window(order.availability_window)
        slice_stmt = select(WatchlistTarget).where(
            WatchlistTarget.watchlist_id == watchlist.id,
            WatchlistTarget.target_type == WatchlistTargetType.SLICE,
            WatchlistTarget.market_product_code == order.market_product,
            WatchlistTarget.delivery_point_id == order.delivery_point_id,
            WatchlistTarget.availability_window_code == normalized_window,
        )
        existing_slice = (await db.execute(slice_stmt)).scalars().first()
        if existing_slice is None:
            companion_slice = WatchlistTarget(
                watchlist_id=watchlist.id,
                target_type=WatchlistTargetType.SLICE,
                market_product_code=order.market_product,
                delivery_point_id=order.delivery_point_id,
                availability_window_code=normalized_window,
                snapshot_market_product=order.market_product,
                snapshot_delivery_point_name=order.delivery_point_name,
                snapshot_availability_window=normalized_window,
            )
            db.add(companion_slice)

        target = WatchlistTarget(
            watchlist_id=watchlist.id,
            target_type=WatchlistTargetType.PIN,
            order_id=order.id,
            market_product_code=order.market_product,
            delivery_point_id=order.delivery_point_id,
            availability_window_code=normalized_window,
        )
        sync_target_snapshot(target, order)

    db.add(target)
    try:
        await db.commit()
    except IntegrityError:
        await db.rollback()
        raise HTTPException(status_code=status.HTTP_409_CONFLICT, detail="Duplicate watchlist target")

    refreshed = await db.get(
        WatchlistTarget,
        target.id,
        options=[selectinload(WatchlistTarget.delivery_point), selectinload(WatchlistTarget.order)],
    )
    return _watchlist_target_response(refreshed or target)


@router.delete("/{watchlist_id}/targets/{target_id}", status_code=204)
async def delete_watchlist_target(
    watchlist_id: UUID,
    target_id: UUID,
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    watchlist = await load_watchlist_or_404(db, watchlist_id, current_user.id)
    if watchlist is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Watchlist not found")

    target = await db.get(WatchlistTarget, target_id)
    if target is None or target.watchlist_id != watchlist.id:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Target not found")

    if target.target_type == WatchlistTargetType.SLICE:
        pin_stmt = select(WatchlistTarget).where(
            WatchlistTarget.watchlist_id == watchlist.id,
            WatchlistTarget.target_type == WatchlistTargetType.PIN,
            WatchlistTarget.market_product_code == target.market_product_code,
            WatchlistTarget.delivery_point_id == target.delivery_point_id,
            WatchlistTarget.availability_window_code == target.availability_window_code,
        )
        for pin in (await db.execute(pin_stmt)).scalars().all():
            await db.delete(pin)

    await db.delete(target)
    await db.commit()
    return Response(status_code=status.HTTP_204_NO_CONTENT)


@router.get("/{watchlist_id}/events", response_model=WatchlistEventsPageResponse)
async def get_watchlist_events(
    watchlist_id: UUID,
    cursor: str | None = Query(None),
    limit: int = Query(20, ge=1, le=MAX_EVENT_LIMIT),
    current_user: Annotated[User, Depends(get_current_user)] = None,
    db: AsyncSession = Depends(get_db),
):
    watchlist = await load_watchlist_or_404(db, watchlist_id, current_user.id)
    if watchlist is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Watchlist not found")
    try:
        return await list_watchlist_events(db, watchlist.id, cursor=cursor, limit=limit)
    except ValueError as exc:
        raise HTTPException(status_code=status.HTTP_422_UNPROCESSABLE_ENTITY, detail=str(exc)) from exc


@router.patch("/{watchlist_id}/events/{event_id}", response_model=WatchlistEventResponse)
async def mark_watchlist_event_read(
    watchlist_id: UUID,
    event_id: UUID,
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    watchlist = await load_watchlist_or_404(db, watchlist_id, current_user.id)
    if watchlist is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Watchlist not found")

    event = await db.get(WatchlistEvent, event_id)
    if event is None or event.watchlist_id != watchlist.id:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Event not found")

    if not event.is_read:
        event.is_read = True
        await db.commit()
    target = await db.get(WatchlistTarget, event.watchlist_target_id)
    target_type = target.target_type.value if target else "SLICE"
    return WatchlistEventResponse(
        id=event.id,
        watchlist_id=event.watchlist_id,
        watchlist_target_id=event.watchlist_target_id,
        target_type=target_type,
        event_type=event.event_type.value,
        event_payload=event.event_payload or {},
        is_read=event.is_read,
        created_at=event.created_at,
    )


@router.post("/{watchlist_id}/entries", response_model=WatchlistEntryResponse, status_code=201)
async def add_watchlist_entry(
    watchlist_id: UUID,
    body: WatchlistEntryAddRequest,
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    """Legacy adapter endpoint retained for one release."""
    stmt = select(Watchlist).where(Watchlist.id == watchlist_id, Watchlist.user_id == current_user.id)
    wl = (await db.execute(stmt)).scalar_one_or_none()
    if not wl:
        raise HTTPException(status_code=404, detail="Watchlist not found")

    product = await db.get(Product, body.product_id)
    if not product:
        raise HTTPException(status_code=404, detail="Product not found")

    dp_name = None
    if body.delivery_point_id:
        dp = await db.get(DeliveryPoint, body.delivery_point_id)
        if not dp:
            raise HTTPException(status_code=404, detail="Delivery point not found")
        dp_name = dp.name

    existing_stmt = select(WatchlistEntry).where(WatchlistEntry.watchlist_id == watchlist_id, WatchlistEntry.product_id == body.product_id)
    if body.delivery_point_id is None:
        existing_stmt = existing_stmt.where(WatchlistEntry.delivery_point_id.is_(None))
    else:
        existing_stmt = existing_stmt.where(WatchlistEntry.delivery_point_id == body.delivery_point_id)
    existing_entry = (await db.execute(existing_stmt)).scalar_one_or_none()
    if existing_entry:
        return _build_entry_response(existing_entry, product.name, dp_name)

    entry_count_stmt = select(func.count(WatchlistEntry.id)).where(WatchlistEntry.watchlist_id == watchlist_id)
    if (await db.execute(entry_count_stmt)).scalar_one() >= MAX_ENTRIES_PER_WATCHLIST:
        raise HTTPException(status_code=400, detail=f"Maximum of {MAX_ENTRIES_PER_WATCHLIST} entries per watchlist")

    entry = WatchlistEntry(watchlist_id=watchlist_id, product_id=body.product_id, delivery_point_id=body.delivery_point_id)
    db.add(entry)
    await db.flush()

    # Translate legacy add-entry into the default radar slice when possible.
    if product.market_product and body.delivery_point_id:
        radar = await ensure_market_radar(db, current_user.id)
        translated_target = WatchlistTarget(
            watchlist_id=radar.id,
            target_type=WatchlistTargetType.SLICE,
            market_product_code=product.market_product,
            delivery_point_id=body.delivery_point_id,
            availability_window_code=SPOT_WINDOW,
            snapshot_market_product=product.market_product,
            snapshot_delivery_point_name=dp_name,
            snapshot_availability_window=SPOT_WINDOW,
        )
        db.add(translated_target)
        try:
            await db.flush()
        except IntegrityError:
            await db.rollback()
            db.add(entry)

    await db.commit()
    await db.refresh(entry)
    return _build_entry_response(entry, product.name, dp_name)


@router.delete("/{watchlist_id}/entries/{entry_id}", status_code=204)
async def remove_watchlist_entry(
    watchlist_id: UUID,
    entry_id: UUID,
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    stmt = select(Watchlist.id).where(Watchlist.id == watchlist_id, Watchlist.user_id == current_user.id)
    if not (await db.execute(stmt)).scalar_one_or_none():
        raise HTTPException(status_code=404, detail="Watchlist not found")

    entry_stmt = select(WatchlistEntry).where(WatchlistEntry.id == entry_id, WatchlistEntry.watchlist_id == watchlist_id)
    entry = (await db.execute(entry_stmt)).scalar_one_or_none()
    if not entry:
        raise HTTPException(status_code=404, detail="Entry not found")

    if entry.delivery_point_id:
        product = await db.get(Product, entry.product_id)
        if product and product.market_product:
            radar = await ensure_market_radar(db, current_user.id)
            translated_stmt = select(WatchlistTarget).where(
                WatchlistTarget.watchlist_id == radar.id,
                WatchlistTarget.target_type == WatchlistTargetType.SLICE,
                WatchlistTarget.market_product_code == product.market_product,
                WatchlistTarget.delivery_point_id == entry.delivery_point_id,
                WatchlistTarget.availability_window_code == SPOT_WINDOW,
            )
            translated_target = (await db.execute(translated_stmt)).scalars().first()
            if translated_target is not None:
                await db.delete(translated_target)

    await db.delete(entry)
    await db.commit()
    return Response(status_code=status.HTTP_204_NO_CONTENT)


@router.delete("/{watchlist_id}", status_code=204)
async def delete_watchlist(
    watchlist_id: UUID,
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    stmt = select(Watchlist).where(Watchlist.id == watchlist_id, Watchlist.user_id == current_user.id).options(selectinload(Watchlist.entries))
    wl = (await db.execute(stmt)).scalar_one_or_none()
    if not wl:
        raise HTTPException(status_code=404, detail="Watchlist not found")
    if wl.kind == WatchlistKind.RADAR_DEFAULT:
        raise HTTPException(status_code=400, detail="The Market Radar container cannot be deleted")
    await db.delete(wl)
    await db.commit()
    return Response(status_code=status.HTTP_204_NO_CONTENT)

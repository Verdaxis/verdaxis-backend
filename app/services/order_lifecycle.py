"""Canonical, bounded order lifecycle transitions under market-slice locks."""
from __future__ import annotations

from collections.abc import Iterable
from datetime import UTC, datetime
from decimal import Decimal
from uuid import UUID

from fastapi import HTTPException
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import Organization
from app.services.audit_actions import ORDER_EXPIRED
from app.services.audit_service import record_audit
from app.services.inventory_reservations import lock_inventory_items
from app.services.market_admission import (
    MarketActorOwnership,
    lock_and_load_market_organizations,
)
from app.services.market_locks import acquire_market_slice_lock
from app.services.watchlist_events import emit_order_updated

MAX_EXPIRY_ROWS_PER_TRANSACTION = 100


async def expire_market_slice_orders(
    db: AsyncSession,
    *,
    side: OrderSide,
    product_id: UUID,
    delivery_point_id: UUID | None,
    availability_window: str,
    now: datetime | None = None,
    limit: int = MAX_EXPIRY_ROWS_PER_TRANSACTION,
    additional_organization_ids: Iterable[UUID] = (),
    actor_ownerships: Iterable[MarketActorOwnership] = (),
    require_approved_organization_ids: Iterable[UUID] = (),
) -> tuple[list[OrderBookOrder], dict[UUID, Organization]]:
    """Expire one exact slice and release every remaining reservation.

    Callers may invoke this repeatedly; each call is bounded and idempotent.
    The advisory lock is acquired before any order or inventory row lock.
    """
    if limit < 1 or limit > MAX_EXPIRY_ROWS_PER_TRANSACTION:
        raise ValueError("expiry limit is outside the operational bound")
    observed_now = now or datetime.now(UTC)
    await acquire_market_slice_lock(
        db,
        side=side,
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        availability_window=availability_window,
    )
    filters = [
        OrderBookOrder.side == side,
        OrderBookOrder.product_id == product_id,
        OrderBookOrder.availability_window == availability_window,
        OrderBookOrder.status.in_(
            (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED)
        ),
        OrderBookOrder.expires_at.is_not(None),
        OrderBookOrder.expires_at <= observed_now,
    ]
    filters.append(
        OrderBookOrder.delivery_point_id.is_(None)
        if delivery_point_id is None
        else OrderBookOrder.delivery_point_id == delivery_point_id
    )
    preview = (
        await db.execute(
            select(OrderBookOrder.id, OrderBookOrder.organization_id)
            .where(*filters)
            .order_by(OrderBookOrder.id)
            .limit(limit)
        )
    ).all()
    rows: list[OrderBookOrder] = []
    if preview:
        # Reapply every expiry predicate while locking the bounded market rows.
        preview_ids = [row.id for row in preview]
        rows = list(
            (
                await db.execute(
                    select(OrderBookOrder)
                    .where(OrderBookOrder.id.in_(preview_ids), *filters)
                    .order_by(OrderBookOrder.id)
                    .with_for_update()
                )
            ).scalars()
        )
        if len(rows) != len(preview_ids):
            raise HTTPException(
                status_code=409,
                detail="Expired order set changed; retry the request",
            )
    organizations = await lock_and_load_market_organizations(
        db,
        [
            *additional_organization_ids,
            *(order.organization_id for order in rows),
        ],
        actor_ownerships=actor_ownerships,
        require_approved=False,
        require_approved_ids=require_approved_organization_ids,
    )
    inventory_ids = {
        order.inventory_item_id
        for order in rows
        if order.inventory_item_id is not None and order.remaining_quantity_mt > 0
    }
    inventory = await lock_inventory_items(db, inventory_ids)
    for order in rows:
        remaining = Decimal(str(order.remaining_quantity_mt))
        before = {
            "price_per_mt_usd": order.price_per_mt_usd,
            "remaining_quantity_mt": remaining,
            "status": order.status,
            "slice_best_price_per_mt_usd": None,
        }
        if order.inventory_item_id is not None and remaining > 0:
            item = inventory[order.inventory_item_id]
            reserved = Decimal(str(item.reserved_stock_mt or 0))
            if reserved < remaining:
                raise HTTPException(
                    status_code=409,
                    detail="Inventory reservation is inconsistent; expiry refused",
                )
            item.reserved_stock_mt = reserved - remaining
            item.current_stock_mt = Decimal(str(item.current_stock_mt or 0)) + remaining
        order.status = OrderBookStatus.EXPIRED
        order.bump_version()
        await emit_order_updated(db, before=before, order=order)
        await record_audit(
            db,
            action=ORDER_EXPIRED,
            resource_type="order",
            resource_id=order.id,
            changes={
                "status": OrderBookStatus.EXPIRED.value,
                "released_quantity_mt": str(remaining),
            },
        )
    return rows, organizations

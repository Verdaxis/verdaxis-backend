"""Single inventory conservation ledger for orderbook lifecycle transitions."""
from __future__ import annotations

from decimal import Decimal
from uuid import UUID

from fastapi import HTTPException
from sqlalchemy import or_, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.marketplace import InventoryItem
from app.models.orderbook import OrderBookOrder, OrderBookStatus, Trade, TradeStatus


def _decimal(value: object) -> Decimal:
    return Decimal(str(value or 0))


async def lock_inventory_items(db: AsyncSession, item_ids: set[UUID]) -> dict[UUID, InventoryItem]:
    """Lock inventory rows in UUID order to avoid inventory deadlocks."""
    if not item_ids:
        return {}
    result = await db.execute(
        select(InventoryItem)
        .where(InventoryItem.id.in_(sorted(item_ids, key=str)))
        .order_by(InventoryItem.id)
        .with_for_update()
    )
    rows = {item.id: item for item in result.scalars().all()}
    missing = item_ids.difference(rows)
    if missing:
        raise HTTPException(status_code=409, detail="Inventory reservation no longer exists")
    return rows


async def assert_inventory_mutable(
    db: AsyncSession,
    item: InventoryItem,
    *,
    deleting: bool = False,
) -> None:
    """Fail closed while an inventory row backs executable or pending work.

    The caller must hold the inventory row lock. A filled order remains a
    commitment while a direct trade awaits confirmation or decline. Deletes
    are stricter because the restrictive FK intentionally preserves every
    order-to-inventory link.
    """
    if _decimal(item.reserved_stock_mt) > 0:
        raise HTTPException(status_code=409, detail="Inventory has an active reservation")

    active_order_id = (
        await db.execute(
            select(OrderBookOrder.id)
            .where(
                OrderBookOrder.inventory_item_id == item.id,
                OrderBookOrder.status.in_(
                    [OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]
                ),
            )
            .limit(1)
        )
    ).scalar_one_or_none()
    if active_order_id is not None:
        raise HTTPException(status_code=409, detail="Inventory has a nonterminal listing")

    nonterminal_trade_id = (
        await db.execute(
            select(Trade.id)
            .join(
                OrderBookOrder,
                or_(
                    Trade.bid_order_id == OrderBookOrder.id,
                    Trade.ask_order_id == OrderBookOrder.id,
                ),
            )
            .where(
                OrderBookOrder.inventory_item_id == item.id,
                Trade.status.in_(
                    (
                        TradeStatus.PENDING_CONFIRMATION,
                        TradeStatus.CONFIRMED,
                        TradeStatus.DELIVERED,
                    )
                ),
            )
            .limit(1)
        )
    ).scalar_one_or_none()
    if nonterminal_trade_id is not None:
        raise HTTPException(status_code=409, detail="Inventory has a nonterminal trade")

    if deleting:
        linked_order_id = (
            await db.execute(
                select(OrderBookOrder.id)
                .where(OrderBookOrder.inventory_item_id == item.id)
                .limit(1)
            )
        ).scalar_one_or_none()
        if linked_order_id is not None:
            raise HTTPException(status_code=409, detail="Inventory has preserved order history")


async def reserve_inventory(db: AsyncSession, item_id: UUID, quantity: Decimal) -> InventoryItem:
    if quantity <= 0:
        raise ValueError("reservation quantity must be positive")
    item = (await lock_inventory_items(db, {item_id}))[item_id]
    current = _decimal(item.current_stock_mt)
    if current < quantity:
        raise HTTPException(status_code=409, detail="Inventory stock is already reserved or unavailable")
    item.current_stock_mt = current - quantity
    item.reserved_stock_mt = _decimal(item.reserved_stock_mt) + quantity
    return item

async def consume_inventory(db: AsyncSession, item_id: UUID, quantity: Decimal) -> InventoryItem:
    """Consume a filled quantity from reserved stock."""
    if quantity <= 0:
        return (await lock_inventory_items(db, {item_id}))[item_id]
    item = (await lock_inventory_items(db, {item_id}))[item_id]
    reserved = _decimal(item.reserved_stock_mt)
    if reserved < quantity:
        raise HTTPException(status_code=409, detail="Inventory reservation is inconsistent")
    item.reserved_stock_mt = reserved - quantity
    return item


async def release_inventory(db: AsyncSession, item_id: UUID, quantity: Decimal) -> InventoryItem:
    """Return an unfilled reservation to current stock."""
    if quantity <= 0:
        return (await lock_inventory_items(db, {item_id}))[item_id]
    item = (await lock_inventory_items(db, {item_id}))[item_id]
    reserved = _decimal(item.reserved_stock_mt)
    if reserved < quantity:
        raise HTTPException(status_code=409, detail="Inventory reservation is inconsistent")
    item.reserved_stock_mt = reserved - quantity
    item.current_stock_mt = _decimal(item.current_stock_mt) + quantity
    return item


async def reconcile_inventory_quantity(
    db: AsyncSession,
    item_id: UUID,
    old_quantity: Decimal,
    new_quantity: Decimal,
) -> InventoryItem:
    """Reconcile unlisted inventory edits without violating conservation."""
    item = (await lock_inventory_items(db, {item_id}))[item_id]
    delta = new_quantity - old_quantity
    current = _decimal(item.current_stock_mt)
    if delta > 0:
        item.current_stock_mt = current + delta
    elif delta < 0:
        reduction = -delta
        if current < reduction:
            raise HTTPException(status_code=409, detail="Cannot reduce inventory below current stock")
        item.current_stock_mt = current - reduction
    return item

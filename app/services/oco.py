"""
OCO (One-Cancels-Other) service helpers.

Provides:
- create_oco_pair: atomically insert two linked OCO orders
- cancel_linked_order: cancel an order and propagate to its linked pair
"""
import uuid
from typing import Any

from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderType

_TERMINAL_STATUSES = {OrderBookStatus.FILLED, OrderBookStatus.CANCELLED, OrderBookStatus.EXPIRED}


async def create_oco_pair(
    db: AsyncSession,
    org_id: uuid.UUID,
    order_a_data: dict[str, Any],
    order_b_data: dict[str, Any],
) -> tuple[OrderBookOrder, OrderBookOrder]:
    """
    Atomically create two OCO-linked orders.

    Both orders are flushed (not committed) so that linked_order_id is set
    within the caller's transaction.

    Returns (order_a, order_b) with symmetric linked_order_id values.
    """
    # Strip any caller-supplied order_type — OCO is always forced
    order_a_kwargs = {k: v for k, v in order_a_data.items() if k != "order_type"}
    order_b_kwargs = {k: v for k, v in order_b_data.items() if k != "order_type"}

    order_a = OrderBookOrder(
        organization_id=org_id,
        order_type=OrderType.OCO,
        **order_a_kwargs,
    )
    order_b = OrderBookOrder(
        organization_id=org_id,
        order_type=OrderType.OCO,
        **order_b_kwargs,
    )

    db.add(order_a)
    db.add(order_b)
    await db.flush()  # Materialise PKs

    # Wire the symmetric link
    order_a.linked_order_id = order_b.id
    order_b.linked_order_id = order_a.id
    await db.flush()

    return order_a, order_b


async def cancel_linked_order(
    db: AsyncSession,
    order: OrderBookOrder,
) -> None:
    """
    Cancel *order* and, if it has a linked OCO partner, cancel that partner too
    (unless the partner is already in a terminal state).

    This is a pure-DB helper — it does NOT commit the transaction.
    """
    order.status = OrderBookStatus.CANCELLED

    if order.linked_order_id is None:
        return

    result = await db.execute(
        select(OrderBookOrder)
        .where(OrderBookOrder.id == order.linked_order_id)
        .with_for_update()
    )
    linked = result.scalars().first()

    if linked is None:
        return

    if linked.status not in _TERMINAL_STATUSES:
        linked.status = OrderBookStatus.CANCELLED

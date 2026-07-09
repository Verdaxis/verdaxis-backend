"""
Match-on-insert engine. When a new order is placed, scan for crossing orders
and automatically create trades. Uses price-time priority (FIFO at each price level).
"""
from datetime import datetime, UTC
import uuid

from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.orderbook import (
    OrderBookOrder, Trade, OrderSide, OrderBookStatus, TradeStatus, Initiator
)
from app.models.notification import Notification, NotificationType
from app.models.user import User
from app.services.demo_market import is_demo_market_organization
from app.services.execution_policy import order_is_execution_qualified, orders_execution_compatible


async def match_order(
    db: AsyncSession,
    new_order: OrderBookOrder,
    is_anonymous: bool = False,
) -> list[Trade]:
    """
    Attempt to match a newly created order against the opposite side of the book.

    Rules:
    - BID matches against ASKs where ask_price <= bid_price
    - ASK matches against BIDs where bid_price >= ask_price
    - Product, delivery point, availability window, and certification constraints must be compatible
    - Price-time priority: best price first, then oldest order first
    - Partial fills allowed: match as much as possible
    - Self-trade prevention: skip orders from same organization
    - All operations within caller's transaction (no separate commit)

    Returns list of Trade objects created (may be empty if no matches).
    """
    trades_created: list[Trade] = []
    pending_notifications: list[tuple[uuid.UUID, str, dict]] = []

    if new_order.remaining_quantity_mt <= 0:
        return trades_created

    new_order_is_demo = is_demo_market_organization(new_order.organization_id)
    if new_order_is_demo:
        return trades_created

    if not order_is_execution_qualified(new_order):
        return trades_created

    # Determine which side to match against
    if new_order.side == OrderSide.BID:
        # BID: match against ASKs where ask_price <= bid_price
        opposite_side = OrderSide.ASK
        # Best ask = lowest price first (ascending), then oldest first
        price_order = OrderBookOrder.price_per_mt_usd.asc()
        price_filter = OrderBookOrder.price_per_mt_usd <= new_order.price_per_mt_usd
    else:
        # ASK: match against BIDs where bid_price >= ask_price
        opposite_side = OrderSide.BID
        # Best bid = highest price first (descending), then oldest first
        price_order = OrderBookOrder.price_per_mt_usd.desc()
        price_filter = OrderBookOrder.price_per_mt_usd >= new_order.price_per_mt_usd

    # Build matching filters: same product_id, same delivery_point_id
    match_filters = [
        OrderBookOrder.side == opposite_side,
        OrderBookOrder.product_id == new_order.product_id,
        OrderBookOrder.availability_window == new_order.availability_window,
        OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
        OrderBookOrder.organization_id != new_order.organization_id,  # No self-trade
        price_filter,
    ]

    # delivery_point_id match: both NULL or both equal
    if new_order.delivery_point_id is None:
        match_filters.append(OrderBookOrder.delivery_point_id.is_(None))
    else:
        match_filters.append(OrderBookOrder.delivery_point_id == new_order.delivery_point_id)

    # Find crossing orders (locked for update to prevent race conditions)
    stmt = (
        select(OrderBookOrder)
        .where(*match_filters)
        .order_by(price_order, OrderBookOrder.created_at.asc())  # Price-time priority
        .with_for_update()
    )

    result = await db.execute(stmt)
    crossing_orders = result.scalars().all()

    for crossing in crossing_orders:
        if new_order.remaining_quantity_mt <= 0:
            break
        if is_demo_market_organization(crossing.organization_id):
            continue
        if not orders_execution_compatible(new_order, crossing):
            continue

        # Determine trade quantity (minimum of both remaining quantities)
        trade_qty = min(new_order.remaining_quantity_mt, crossing.remaining_quantity_mt)

        # Trade price = the resting order's price (price improvement for aggressor)
        trade_price = crossing.price_per_mt_usd

        # Determine buyer/seller
        if new_order.side == OrderSide.BID:
            buyer_org = new_order.organization_id
            seller_org = crossing.organization_id
            bid_order_id = new_order.id
            ask_order_id = crossing.id
            initiated_by = Initiator.BUYER
        else:
            buyer_org = crossing.organization_id
            seller_org = new_order.organization_id
            bid_order_id = crossing.id
            ask_order_id = new_order.id
            initiated_by = Initiator.SELLER

        # Create trade (auto-confirmed since both sides agreed via price)
        trade = Trade(
            bid_order_id=bid_order_id,
            ask_order_id=ask_order_id,
            buyer_id=buyer_org,
            seller_id=seller_org,
            initiated_by=initiated_by,
            quantity_mt=trade_qty,
            price_per_mt_usd=trade_price,
            status=TradeStatus.CONFIRMED,  # Auto-matched = auto-confirmed
            confirmed_at=datetime.now(UTC),
            is_anonymous=is_anonymous,
        )
        db.add(trade)
        await db.flush()

        # Update quantities
        new_order.remaining_quantity_mt -= trade_qty
        crossing.remaining_quantity_mt -= trade_qty

        # Update order statuses
        if new_order.remaining_quantity_mt == 0:
            new_order.status = OrderBookStatus.FILLED
        else:
            new_order.status = OrderBookStatus.PARTIALLY_FILLED

        if crossing.remaining_quantity_mt == 0:
            crossing.status = OrderBookStatus.FILLED
        else:
            crossing.status = OrderBookStatus.PARTIALLY_FILLED

        trades_created.append(trade)

        # Derive product name for notification messages
        product_name = new_order.product_name or "fuel"

        # Queue notifications for both parties; delivered in one batch after
        # the loop so we run a single user query instead of two per trade.
        message = (
            f"Your order was automatically matched: "
            f"{trade_qty} MT of {product_name} at ${trade_price}/MT"
        )
        data = {"trade_id": str(trade.id), "auto_matched": True}
        pending_notifications.append((buyer_org, message, data))
        pending_notifications.append((seller_org, message, data))

    await _notify_orgs(db, pending_notifications)

    return trades_created


async def _notify_orgs(
    db: AsyncSession,
    pending: list[tuple[uuid.UUID, str, str | dict]],
) -> None:
    """Fan (org_id, message, data) tuples out to every user of each org.

    Fetches users for all involved orgs in one query (the per-trade version
    was an N+1: two user queries per matched trade).
    """
    if not pending:
        return
    org_ids = {org_id for org_id, _, _ in pending}
    result = await db.execute(select(User).where(User.organization_id.in_(org_ids)))
    users_by_org: dict[uuid.UUID, list[User]] = {}
    for user in result.scalars():
        users_by_org.setdefault(user.organization_id, []).append(user)

    for org_id, message, data in pending:
        for user in users_by_org.get(org_id, []):
            db.add(Notification(
                recipient_id=user.id,
                type=NotificationType.TRADE_CONFIRMED,
                title="Auto-Matched Trade",
                message=message,
                data=data,
            ))

"""
StopEngine — scans and triggers Stop/StopLimit orders when a trade executes.

Called after every trade with the trade's fuel_type and price. Evaluates all
OPEN stop orders for that fuel type and triggers those whose stop_price has
been crossed, then immediately attempts to match the newly-active order.

Trigger rules:
  BID STOP / BID STOP_LIMIT : triggers when last_trade_price >= stop_price
  ASK STOP / ASK STOP_LIMIT : triggers when last_trade_price <= stop_price

On trigger:
  - order_type changes: STOP -> MARKET, STOP_LIMIT -> LIMIT
  - status changes:     OPEN -> TRIGGERED
  - match_order() is called to attempt immediate execution
  - an SSE event is published to the "orderbook" channel
"""
from decimal import Decimal

from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.orderbook import (
    OrderBookOrder,
    OrderBookStatus,
    OrderSide,
    OrderType,
)
from app.services.event_bus import event_bus
from app.services.matching_engine import match_order


async def check_stops(
    db: AsyncSession,
    *,
    fuel_type: str,
    last_trade_price: Decimal | float,
    _trigger_stops: bool = True,
) -> list[OrderBookOrder]:
    """
    Evaluate all OPEN Stop/StopLimit orders for *fuel_type* against
    *last_trade_price*. Trigger those whose stop condition is met.

    Returns the list of orders that were triggered (already mutated in-place
    and flushed to the session; caller owns the commit).

    *_trigger_stops* — internal recursion guard. When False, the secondary
    match_order calls for triggered orders will NOT themselves invoke
    check_stops again, preventing infinite cascading stop chains. Callers
    outside this module should always leave this at the default (True); the
    orderbook router passes False when calling after a primary match so that
    stop-triggered secondary trades do not re-enter the stop evaluation loop.
    """
    last_trade_price = Decimal(str(last_trade_price))
    # Fetch all OPEN stop-family orders for this fuel type.
    stmt = (
        select(OrderBookOrder)
        .where(
            OrderBookOrder.fuel_type == fuel_type,
            OrderBookOrder.status == OrderBookStatus.OPEN,
            OrderBookOrder.order_type.in_([OrderType.STOP, OrderType.STOP_LIMIT]),
        )
        .with_for_update()
    )
    result = await db.execute(stmt)
    candidates = result.scalars().all()

    triggered: list[OrderBookOrder] = []

    for order in candidates:
        if order.stop_price is None:
            continue  # Guard: malformed order, skip silently

        should_trigger = _evaluate_trigger(order.side, order.stop_price, last_trade_price)
        if not should_trigger:
            continue

        # Mutate the order: promote it into an active order type
        if order.order_type == OrderType.STOP:
            order.order_type = OrderType.MARKET
        else:  # STOP_LIMIT
            order.order_type = OrderType.LIMIT

        order.status = OrderBookStatus.TRIGGERED
        triggered.append(order)

    if not triggered:
        return triggered

    # Flush so match_order can see the updated rows
    await db.flush()

    # Attempt immediate matching and publish SSE for each triggered order
    for order in triggered:
        await match_order(db, order)

        await event_bus.publish(
            "orderbook",
            "stop_triggered",
            {
                "order_id": str(order.id),
                "side": order.side.value,
                "fuel_type": order.fuel_type,
                "stop_price": str(order.stop_price),
                "last_trade_price": str(last_trade_price),
                "new_order_type": order.order_type.value,
            },
        )

    return triggered


def _evaluate_trigger(
    side: OrderSide,
    stop_price: Decimal,
    last_trade_price: Decimal,
) -> bool:
    """
    Return True when the stop condition for *side* is satisfied.

    BID stop: buyer wants in when the market rises above their trigger price.
              Triggers when last_trade_price >= stop_price.

    ASK stop: seller wants out when the market falls below their trigger price.
              Triggers when last_trade_price <= stop_price.
    """
    if side == OrderSide.BID:
        return last_trade_price >= stop_price
    else:  # ASK
        return last_trade_price <= stop_price

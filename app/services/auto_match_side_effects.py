"""Durable side effects shared by every automatic matching entry point."""
from __future__ import annotations

from decimal import Decimal
from typing import Any
from uuid import UUID

from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import selectinload

from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide, Trade
from app.services.activity import order_activity_provenance, trade_activity_provenance
from app.services.audit_actions import TRADE_AUTO_MATCHED
from app.services.audit_service import record_audit
from app.services.market_events import CommittedMarketEvent, participant_market_event
from app.services.watchlist_events import emit_pin_updated, emit_slice_state_changed


async def collect_auto_match_side_effects(
    db: AsyncSession,
    *,
    triggering_order: OrderBookOrder,
    trades: list[Trade],
    actor_user_id: UUID | None,
    audit_context: dict[str, Any],
    resting_side_previous_best_price: Decimal | None,
) -> list[CommittedMarketEvent]:
    """Record DB side effects and return external events for post-commit publish."""
    if not trades:
        return []

    for trade in trades:
        await record_audit(
            db,
            user_id=actor_user_id,
            action=TRADE_AUTO_MATCHED,
            resource_type="trade",
            resource_id=trade.id,
            changes={
                "trade_id": str(trade.id),
                "bid_order_id": str(trade.bid_order_id) if trade.bid_order_id else None,
                "ask_order_id": str(trade.ask_order_id) if trade.ask_order_id else None,
                "quantity_mt": str(trade.quantity_mt),
                "price_per_mt_usd": str(trade.price_per_mt_usd),
                "buyer_org_id": str(trade.buyer_id),
                "seller_org_id": str(trade.seller_id),
            },
            **audit_context,
        )

    matched_quantity_by_resting_order: dict[UUID, Decimal] = {}
    for trade in trades:
        resting_id = (
            trade.ask_order_id
            if triggering_order.side == OrderSide.BID
            else trade.bid_order_id
        )
        if resting_id is not None:
            matched_quantity_by_resting_order[resting_id] = (
                matched_quantity_by_resting_order.get(resting_id, Decimal("0"))
                + trade.quantity_mt
            )

    resting_orders: list[OrderBookOrder] = []
    if matched_quantity_by_resting_order:
        resting_orders = list(
            (
                await db.execute(
                    select(OrderBookOrder)
                    .options(
                        selectinload(OrderBookOrder.organization),
                        selectinload(OrderBookOrder.product),
                        selectinload(OrderBookOrder.delivery_point),
                    )
                    .where(
                        OrderBookOrder.id.in_(matched_quantity_by_resting_order)
                    )
                    .order_by(OrderBookOrder.id)
                )
            ).scalars()
        )
    for resting_order in resting_orders:
        matched_quantity = matched_quantity_by_resting_order[resting_order.id]
        before_remaining = resting_order.remaining_quantity_mt + matched_quantity
        before_status = (
            OrderBookStatus.OPEN
            if before_remaining == resting_order.quantity_mt
            else OrderBookStatus.PARTIALLY_FILLED
        )
        await emit_pin_updated(
            db,
            before={
                "price_per_mt_usd": resting_order.price_per_mt_usd,
                "remaining_quantity_mt": before_remaining,
                "status": before_status,
            },
            order=resting_order,
        )

    if resting_orders:
        representative = resting_orders[0]
        await emit_slice_state_changed(
            db,
            market_product_code=representative.market_product,
            delivery_point_id=representative.delivery_point_id,
            availability_window_code=representative.availability_window,
            side=representative.side,
            before_best_price=resting_side_previous_best_price,
            quiet_order_id=representative.id,
            source_order=representative,
        )

    events: list[CommittedMarketEvent] = [
        participant_market_event(
            event_type="trade_auto_matched",
            aggregate_type="trade",
            aggregate_id=trade.id,
            participant_org_ids=(trade.buyer_id, trade.seller_id),
            payload={
                **trade_activity_provenance(trade),
                "trade_id": str(trade.id),
                "product_name": trade.product_name or "",
                "fuel_type": trade.fuel_type or "",
                "quantity": str(trade.quantity_mt),
                "price": str(trade.price_per_mt_usd),
                "is_anonymous": trade.is_anonymous,
            },
        )
        for trade in trades
    ]
    events.append(
        participant_market_event(
            event_type="orders_matched",
            aggregate_type="order",
            aggregate_id=triggering_order.id,
            participant_org_ids={
                triggering_order.organization_id,
                *(trade.buyer_id for trade in trades),
                *(trade.seller_id for trade in trades),
            },
            payload={
                **order_activity_provenance(triggering_order),
                "order_id": str(triggering_order.id),
                "matches": len(trades),
            },
        )
    )
    return events

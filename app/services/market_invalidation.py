"""Fail-closed market cleanup for an externally revoked organization.

This service deliberately does not change organization approval or provenance,
commit, or publish. The security integration must invoke it in the same retried
transaction that changes approval, then publish the returned event only after
that transaction commits.
"""
from __future__ import annotations

from collections import defaultdict
from datetime import UTC, datetime
from decimal import Decimal
from uuid import UUID

from fastapi import HTTPException
from sqlalchemy import or_, select
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import selectinload

from app.models.orderbook import (
    OrderBookOrder,
    OrderBookStatus,
    OrderSide,
    Trade,
    TradeStatus,
)
from app.services.audit_actions import (
    MARKET_ACCESS_INVALIDATED,
    ORDER_CANCELLED,
    ORDER_EXPIRED,
    ORDER_UPDATED,
    TRADE_CANCELLED,
)
from app.services.audit_service import record_audit
from app.services.inventory_reservations import lock_inventory_items
from app.services.live_benchmarks import rebuild_live_slice_benchmarks_for_keys
from app.services.market_admission import lock_and_load_market_organizations
from app.services.market_events import CommittedMarketEvent, participant_market_event
from app.services.market_locks import acquire_market_slice_locks
from app.services.market_transactions import RetryableMarketTransactionError
from app.services.watchlist_events import emit_order_updated

MAX_INVALIDATION_ROWS_PER_TRANSACTION = 100
_ACTIVE_ORDER_STATUSES = (
    OrderBookStatus.OPEN,
    OrderBookStatus.PARTIALLY_FILLED,
)


def _decimal(value: object) -> Decimal:
    return Decimal(str(value or 0))


def _order_identity(row: object) -> tuple[UUID, UUID, OrderSide, UUID, UUID | None, str]:
    return (
        row.id,
        row.organization_id,
        row.side,
        row.product_id,
        row.delivery_point_id,
        row.availability_window,
    )


async def _preview(
    db: AsyncSession,
    organization_id: UUID,
) -> tuple[list[object], list[object], list[object]]:
    trades = (
        await db.execute(
            select(
                Trade.id,
                Trade.bid_order_id,
                Trade.ask_order_id,
                Trade.buyer_id,
                Trade.seller_id,
                Trade.product_id,
                Trade.delivery_point_id,
                Trade.availability_window,
            )
            .where(
                Trade.status == TradeStatus.PENDING_CONFIRMATION,
                or_(Trade.buyer_id == organization_id, Trade.seller_id == organization_id),
            )
            .order_by(Trade.id)
            .limit(MAX_INVALIDATION_ROWS_PER_TRANSACTION + 1)
        )
    ).all()
    owned_orders = (
        await db.execute(
            select(
                OrderBookOrder.id,
                OrderBookOrder.organization_id,
                OrderBookOrder.side,
                OrderBookOrder.product_id,
                OrderBookOrder.delivery_point_id,
                OrderBookOrder.availability_window,
            )
            .where(
                OrderBookOrder.organization_id == organization_id,
                OrderBookOrder.status.in_(_ACTIVE_ORDER_STATUSES),
            )
            .order_by(OrderBookOrder.id)
            .limit(MAX_INVALIDATION_ROWS_PER_TRANSACTION + 1)
        )
    ).all()
    linked_ids = {
        order_id
        for trade in trades
        for order_id in (trade.bid_order_id, trade.ask_order_id)
        if order_id is not None
    }
    linked_orders = []
    if linked_ids:
        linked_orders = (
            await db.execute(
                select(
                    OrderBookOrder.id,
                    OrderBookOrder.organization_id,
                    OrderBookOrder.side,
                    OrderBookOrder.product_id,
                    OrderBookOrder.delivery_point_id,
                    OrderBookOrder.availability_window,
                )
                .where(OrderBookOrder.id.in_(linked_ids))
                .order_by(OrderBookOrder.id)
            )
        ).all()
    if (
        len(trades) > MAX_INVALIDATION_ROWS_PER_TRANSACTION
        or len(owned_orders) > MAX_INVALIDATION_ROWS_PER_TRANSACTION
    ):
        raise HTTPException(
            status_code=409,
            detail=(
                "Market access invalidation exceeds the per-transaction bound; "
                "approval was not changed"
            ),
        )
    return trades, owned_orders, linked_orders


async def invalidate_organization_market_access(
    db: AsyncSession,
    *,
    organization_id: UUID,
    actor_user_id: UUID | None,
    reason: str,
    reference: str,
) -> list[CommittedMarketEvent]:
    """Cancel executable work and release reservations under canonical locks.

    Lock order is every affected slice, every affected organization, pending
    trade rows, order rows, then inventory rows. A changed preview raises a
    retry marker; the integration transaction boundary must retry from fresh.
    """
    if not reason.strip() or not reference.strip():
        raise ValueError("market invalidation requires reason and reference")

    preview_trades, preview_owned, preview_linked = await _preview(db, organization_id)
    identities = {
        row.id: _order_identity(row) for row in (*preview_owned, *preview_linked)
    }
    slice_keys = [
        (identity[2], identity[3], identity[4], identity[5])
        for identity in identities.values()
    ]
    linked_order_ids = {
        order_id
        for trade in preview_trades
        for order_id in (trade.bid_order_id, trade.ask_order_id)
        if order_id is not None
    }
    # Legacy orderless pending trades still have an immutable market snapshot.
    for trade in preview_trades:
        if (
            trade.bid_order_id is None
            and trade.ask_order_id is None
            and trade.product_id is not None
            and trade.availability_window
        ):
            slice_keys.append(
                (
                    OrderSide.BID,
                    trade.product_id,
                    trade.delivery_point_id,
                    trade.availability_window,
                )
            )
    await acquire_market_slice_locks(db, slice_keys)

    # A transaction could have entered a previously unseen slice before the
    # advisory lock. Never continue under an incomplete slice lock set.
    current_trades, current_owned, current_linked = await _preview(db, organization_id)
    current_identities = {
        row.id: _order_identity(row) for row in (*current_owned, *current_linked)
    }
    if (
        {row.id for row in current_trades} != {row.id for row in preview_trades}
        or current_identities != identities
    ):
        raise RetryableMarketTransactionError("40001")

    trade_ids = sorted((row.id for row in preview_trades), key=str)
    trades: list[Trade] = []
    if trade_ids:
        trades = list(
            (
                await db.execute(
                    select(Trade)
                    .where(
                        Trade.id.in_(trade_ids),
                        Trade.status == TradeStatus.PENDING_CONFIRMATION,
                        or_(
                            Trade.buyer_id == organization_id,
                            Trade.seller_id == organization_id,
                        ),
                    )
                    .order_by(Trade.id)
                    .with_for_update()
                )
            ).scalars()
        )
    if {trade.id for trade in trades} != set(trade_ids):
        raise RetryableMarketTransactionError("40001")

    order_ids = sorted(
        set(identities).union(linked_order_ids), key=str
    )
    orders: list[OrderBookOrder] = []
    if order_ids:
        orders = list(
            (
                await db.execute(
                    select(OrderBookOrder)
                    .options(selectinload(OrderBookOrder.product))
                    .where(OrderBookOrder.id.in_(order_ids))
                    .order_by(OrderBookOrder.id)
                    .with_for_update()
                    .execution_options(populate_existing=True)
                )
            ).unique().scalars()
        )
    if {order.id for order in orders} != set(order_ids):
        raise RetryableMarketTransactionError("40001")

    organization_ids = {
        organization_id,
        *(order.organization_id for order in orders),
        *(trade.buyer_id for trade in trades),
        *(trade.seller_id for trade in trades),
    }
    await lock_and_load_market_organizations(
        db, organization_ids, require_approved=False
    )

    restored_by_order: dict[UUID, Decimal] = defaultdict(lambda: Decimal("0"))
    for trade in trades:
        for order_id in (trade.bid_order_id, trade.ask_order_id):
            if order_id is not None:
                restored_by_order[order_id] += _decimal(trade.quantity_mt)
        trade.status = TradeStatus.CANCELLED
        await record_audit(
            db,
            user_id=actor_user_id,
            action=TRADE_CANCELLED,
            resource_type="trade",
            resource_id=trade.id,
            changes={
                "status": TradeStatus.CANCELLED.value,
                "reason": reason,
                "reference": reference,
            },
        )

    releases: dict[UUID, Decimal] = defaultdict(lambda: Decimal("0"))
    benchmark_keys = []
    observed_now = datetime.now(UTC)
    for order in orders:
        restored = restored_by_order[order.id]
        old_remaining = _decimal(order.remaining_quantity_mt)
        old_status = order.status
        before = {
            "price_per_mt_usd": order.price_per_mt_usd,
            "remaining_quantity_mt": old_remaining,
            "status": old_status,
            "slice_best_price_per_mt_usd": None,
        }
        if restored:
            order.remaining_quantity_mt = old_remaining + restored
            if order.remaining_quantity_mt > order.quantity_mt:
                raise HTTPException(
                    status_code=409,
                    detail="Pending trade quantity exceeds its immutable order capacity",
                )

        terminal_before = old_status in (
            OrderBookStatus.CANCELLED,
            OrderBookStatus.EXPIRED,
        )
        owner_revoked = order.organization_id == organization_id
        expired_now = (
            order.expires_at is not None and order.expires_at <= observed_now
        )
        release = Decimal("0")
        if terminal_before:
            release = restored
        elif owner_revoked:
            release = _decimal(order.remaining_quantity_mt)
            order.status = OrderBookStatus.CANCELLED
        elif expired_now:
            release = _decimal(order.remaining_quantity_mt)
            order.status = OrderBookStatus.EXPIRED
        elif restored:
            order.status = (
                OrderBookStatus.OPEN
                if order.remaining_quantity_mt == order.quantity_mt
                else OrderBookStatus.PARTIALLY_FILLED
            )

        if order.inventory_item_id is not None and release > 0:
            releases[order.inventory_item_id] += release
        if owner_revoked or restored:
            benchmark_keys.append(
                (
                    order.side,
                    order.market_product,
                    order.delivery_point_id,
                    order.availability_window,
                )
            )
            await emit_order_updated(db, before=before, order=order)
            changes = {
                "status": order.status.value,
                "restored_quantity_mt": str(restored),
                "released_quantity_mt": str(release),
                "reason": reason,
                "reference": reference,
            }
            if owner_revoked and not terminal_before:
                await record_audit(
                    db, user_id=actor_user_id, action=ORDER_CANCELLED,
                    resource_type="order", resource_id=order.id, changes=changes,
                )
            elif order.status == OrderBookStatus.EXPIRED and not terminal_before:
                await record_audit(
                    db, user_id=actor_user_id, action=ORDER_EXPIRED,
                    resource_type="order", resource_id=order.id, changes=changes,
                )
            else:
                await record_audit(
                    db, user_id=actor_user_id, action=ORDER_UPDATED,
                    resource_type="order", resource_id=order.id, changes=changes,
                )

    inventory = await lock_inventory_items(db, set(releases))
    for item_id, quantity in releases.items():
        item = inventory[item_id]
        reserved = _decimal(item.reserved_stock_mt)
        if reserved < quantity:
            raise HTTPException(
                status_code=409,
                detail="Inventory reservation is inconsistent; invalidation refused",
            )
        item.reserved_stock_mt = reserved - quantity
        item.current_stock_mt = _decimal(item.current_stock_mt) + quantity

    await rebuild_live_slice_benchmarks_for_keys(db, benchmark_keys)
    await record_audit(
        db,
        user_id=actor_user_id,
        action=MARKET_ACCESS_INVALIDATED,
        resource_type="organization",
        resource_id=organization_id,
        changes={
            "cancelled_trade_ids": [str(trade.id) for trade in trades],
            "affected_order_ids": [str(order.id) for order in orders],
            "reason": reason,
            "reference": reference,
        },
    )
    return [participant_market_event(
        event_type="market_access_invalidated",
        aggregate_type="organization",
        aggregate_id=organization_id,
        participant_org_ids=organization_ids,
        payload={
            "organization_id": str(organization_id),
            "cancelled_trade_count": len(trades),
            "affected_order_count": len(orders),
            "reference": reference,
        },
    )]

"""Lifecycle-correct executable-state invalidation after security revocation."""

from __future__ import annotations

from collections.abc import Iterable
from decimal import Decimal
from uuid import UUID

from fastapi import HTTPException
from sqlalchemy import or_, select
from sqlalchemy.exc import DBAPIError
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import selectinload

from app.models.marketplace import InventoryItem
from app.models.negotiation import Negotiation, NegotiationStatus
from app.models.orderbook import OrderBookOrder, OrderBookStatus
from app.models.rfq import QuoteStatus, RFQ, RFQQuote, RFQStatus
from app.services.activity import order_activity_provenance
from app.services.audit_actions import (
    NEGOTIATION_DECLINED,
    ORDER_CANCELLED,
    RFQ_CANCELLED,
    RFQ_QUOTE_WITHDRAWN,
)
from app.services.audit_service import record_audit
from app.services.availability_windows import normalize_availability_window
from app.services.event_bus import event_bus
from app.services.live_benchmarks import rebuild_live_slice_benchmarks_for_keys
from app.services.watchlist_events import _best_slice_price, emit_order_updated


class InvalidationResult(dict[str, int]):
    """Count mapping plus events that may be published only after commit."""

    def __init__(self) -> None:
        super().__init__(
            orders_cancelled=0,
            rfqs_cancelled=0,
            quotes_withdrawn=0,
            negotiations_declined=0,
        )
        self.events: list[tuple[str, str, dict]] = []


INVALIDATION_RETRY_AFTER_SECONDS = 1
LOCK_NOT_AVAILABLE_SQLSTATE = "55P03"


def _database_sqlstate(exc: DBAPIError) -> str | None:
    candidate = exc.orig
    for _ in range(3):
        for attribute in ("sqlstate", "pgcode"):
            value = getattr(candidate, attribute, None)
            if value:
                return str(value)
        candidate = getattr(candidate, "__cause__", None)
        if candidate is None:
            break
    return None


async def invalidate_execution_state_for_request(
    db: AsyncSession,
    **scope,
) -> InvalidationResult:
    """Translate expected NOWAIT contention into one rollback-safe API contract."""
    try:
        return await invalidate_execution_state(db, **scope)
    except DBAPIError as exc:
        if _database_sqlstate(exc) != LOCK_NOT_AVAILABLE_SQLSTATE:
            raise
        await db.rollback()
        raise HTTPException(
            status_code=409,
            detail={
                "code": "EXECUTION_INVALIDATION_BUSY",
                "message": "Execution state is being updated; retry this request",
                "retry_after_seconds": INVALIDATION_RETRY_AFTER_SECONDS,
            },
            headers={"Retry-After": str(INVALIDATION_RETRY_AFTER_SECONDS)},
        ) from exc


def release_unfilled_inventory_reservation(
    order: OrderBookOrder, inventory: InventoryItem | None
) -> Decimal:
    """Release only a linked active order's unfilled reservation."""
    if (
        inventory is None
        or getattr(order, "inventory_item_id", None) is None
        or order.status not in (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED)
    ):
        return Decimal("0.00")
    remaining = max(Decimal(str(order.remaining_quantity_mt or 0)), Decimal("0.00"))
    reserved = max(Decimal(str(inventory.reserved_stock_mt or 0)), Decimal("0.00"))
    released = min(remaining, reserved)
    inventory.reserved_stock_mt = reserved - released
    return released


async def _watchlist_before_state(db: AsyncSession, order: OrderBookOrder) -> dict[str, object]:
    return {
        "price_per_mt_usd": order.price_per_mt_usd,
        "remaining_quantity_mt": order.remaining_quantity_mt,
        "status": order.status,
        "availability_window": normalize_availability_window(order.availability_window),
        "delivery_point_id": order.delivery_point_id,
        "market_product": order.market_product,
        "slice_best_price_per_mt_usd": await _best_slice_price(
            db,
            market_product_code=order.market_product,
            delivery_point_id=order.delivery_point_id,
            availability_window_code=order.availability_window,
            side=order.side,
        ),
    }


def _scope(expressions: list[object]):
    return or_(*expressions)


async def invalidate_execution_state(
    db: AsyncSession,
    *,
    user_ids: Iterable[UUID] = (),
    organization_ids: Iterable[UUID] = (),
    order_ids: Iterable[UUID] = (),
    direct_order_owner_user_id: UUID | None = None,
    direct_order_organization_id: UUID | None = None,
    actor_user_id: UUID | None = None,
    reason: str = "security_admission_revoked",
    ip_address: str | None = None,
    request_id: str | None = None,
) -> InvalidationResult:
    """Lock, revalidate, and stage normal cancellation lifecycles.

    The caller owns commit/rollback and must call
    :func:`publish_execution_invalidation` only after a successful commit.
    All locks are NOWAIT and acquired in canonical entity/slice order.
    """
    users = tuple(sorted(set(user_ids), key=str))
    organizations = tuple(sorted(set(organization_ids), key=str))
    direct_orders = tuple(sorted(set(order_ids), key=str))
    result = InvalidationResult()
    if not users and not organizations and not direct_orders:
        return result

    order_scope: list[object] = []
    rfq_scope: list[object] = []
    quote_scope: list[object] = []
    negotiation_scope: list[object] = []
    if users:
        order_scope.append(OrderBookOrder.owner_user_id.in_(users))
        rfq_scope.append(RFQ.buyer_user_id.in_(users))
        quote_scope.append(RFQQuote.seller_user_id.in_(users))
        negotiation_scope.extend(
            (Negotiation.initiator_user_id.in_(users), Negotiation.counterparty_user_id.in_(users))
        )
    if organizations:
        order_scope.append(OrderBookOrder.organization_id.in_(organizations))
        rfq_scope.append(RFQ.buyer_org_id.in_(organizations))
        quote_scope.append(RFQQuote.seller_org_id.in_(organizations))
        negotiation_scope.extend(
            (Negotiation.initiator_org_id.in_(organizations), Negotiation.counterparty_org_id.in_(organizations))
        )
    if direct_orders:
        direct_filter = OrderBookOrder.id.in_(direct_orders)
        ownership_filters = []
        if direct_order_owner_user_id is not None:
            ownership_filters.append(OrderBookOrder.owner_user_id == direct_order_owner_user_id)
        if direct_order_organization_id is not None:
            ownership_filters.append(
                OrderBookOrder.owner_user_id.is_(None)
                & (OrderBookOrder.organization_id == direct_order_organization_id)
            )
        if ownership_filters:
            direct_filter = direct_filter & or_(*ownership_filters)
        order_scope.append(direct_filter)

    # Slice/order locks precede inventory locks. The full ordering is shared by
    # every invalidation invocation, preventing cross-tenant deadlock cycles.
    orders = (
        await db.execute(
            select(OrderBookOrder)
            .options(selectinload(OrderBookOrder.product), selectinload(OrderBookOrder.delivery_point))
            .where(
                OrderBookOrder.status.in_((OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED)),
                _scope(order_scope),
            )
            .order_by(
                OrderBookOrder.product_id,
                OrderBookOrder.delivery_point_id,
                OrderBookOrder.availability_window,
                OrderBookOrder.side,
                OrderBookOrder.id,
            )
            .with_for_update(nowait=True)
        )
    ).scalars().all()
    inventory_ids = sorted(
        {order.inventory_item_id for order in orders if order.inventory_item_id is not None}, key=str
    )
    inventories: dict[UUID, InventoryItem] = {}
    if inventory_ids:
        locked_inventory = (
            await db.execute(
                select(InventoryItem)
                .where(InventoryItem.id.in_(inventory_ids))
                .order_by(InventoryItem.id)
                .with_for_update(nowait=True)
            )
        ).scalars().all()
        inventories = {item.id: item for item in locked_inventory}

    before_states = [(order, await _watchlist_before_state(db, order)) for order in orders]
    benchmark_keys = []
    for order, _before in before_states:
        previous = order.status
        released = release_unfilled_inventory_reservation(
            order, inventories.get(order.inventory_item_id)
        )
        order.status = OrderBookStatus.CANCELLED
        benchmark_keys.append(
            (order.side, order.market_product, order.delivery_point_id, order.availability_window)
        )
        await record_audit(
            db,
            user_id=actor_user_id,
            action=ORDER_CANCELLED,
            resource_type="order",
            resource_id=order.id,
            changes={
                "status": {"from": previous.value, "to": OrderBookStatus.CANCELLED.value},
                "reason": reason,
                "released_reserved_stock_mt": str(released),
                "inventory_item_id": str(order.inventory_item_id) if order.inventory_item_id else None,
            },
            ip_address=ip_address,
            request_id=request_id,
        )
        result["orders_cancelled"] += 1
        result.events.append(
            (
                "orderbook",
                "order_cancelled",
                {
                    **order_activity_provenance(order),
                    "id": str(order.id),
                    "side": order.side.value,
                    "product_name": order.product_name,
                    "fuel_type": order.fuel_type,
                    "region": order.region,
                },
            )
        )
    if benchmark_keys:
        await rebuild_live_slice_benchmarks_for_keys(db, benchmark_keys)
        for order, before in before_states:
            await emit_order_updated(db, before=before, order=order)

    rfqs = []
    if rfq_scope:
        rfqs = (
            await db.execute(
            select(RFQ)
            .where(RFQ.status.in_((RFQStatus.OPEN, RFQStatus.QUOTED)), _scope(rfq_scope))
            .order_by(RFQ.product_id, RFQ.delivery_point_id, RFQ.availability_window, RFQ.id)
            .with_for_update(nowait=True)
            )
        ).scalars().all()
    rfq_ids = [rfq.id for rfq in rfqs]
    quote_filters = [_scope(quote_scope)] if quote_scope else []
    if rfq_ids:
        quote_filters.append(RFQQuote.rfq_id.in_(rfq_ids))
    quotes = []
    if quote_filters:
        quotes = (
            await db.execute(
            select(RFQQuote)
            .where(RFQQuote.status == QuoteStatus.PENDING, or_(*quote_filters))
            .order_by(RFQQuote.rfq_id, RFQQuote.id)
            .with_for_update(nowait=True)
            )
        ).scalars().all()
    for rfq in rfqs:
        previous = rfq.status
        rfq.status = RFQStatus.CANCELLED
        await record_audit(
            db, user_id=actor_user_id, action=RFQ_CANCELLED, resource_type="rfq",
            resource_id=rfq.id,
            changes={"status": {"from": previous.value, "to": RFQStatus.CANCELLED.value}, "reason": reason},
            ip_address=ip_address, request_id=request_id,
        )
        result["rfqs_cancelled"] += 1
    for quote in quotes:
        quote.status = QuoteStatus.WITHDRAWN
        await record_audit(
            db, user_id=actor_user_id, action=RFQ_QUOTE_WITHDRAWN,
            resource_type="rfq_quote", resource_id=quote.id,
            changes={"status": {"from": QuoteStatus.PENDING.value, "to": QuoteStatus.WITHDRAWN.value}, "reason": reason},
            ip_address=ip_address, request_id=request_id,
        )
        result["quotes_withdrawn"] += 1

    negotiations = []
    if negotiation_scope:
        negotiations = (
            await db.execute(
            select(Negotiation)
            .where(
                Negotiation.status.in_((NegotiationStatus.OPEN, NegotiationStatus.COUNTERED)),
                _scope(negotiation_scope),
            )
            .order_by(Negotiation.bid_order_id, Negotiation.ask_order_id, Negotiation.id)
            .with_for_update(nowait=True)
            )
        ).scalars().all()
    for negotiation in negotiations:
        previous = negotiation.status
        negotiation.status = NegotiationStatus.DECLINED
        await record_audit(
            db, user_id=actor_user_id, action=NEGOTIATION_DECLINED,
            resource_type="negotiation", resource_id=negotiation.id,
            changes={"status": {"from": previous.value, "to": NegotiationStatus.DECLINED.value}, "reason": reason},
            ip_address=ip_address, request_id=request_id,
        )
        result["negotiations_declined"] += 1
        result.events.append(
            ("negotiation", "negotiation_declined", {"id": str(negotiation.id), "reason": reason})
        )
    return result


async def publish_execution_invalidation(result: InvalidationResult) -> None:
    """Publish staged lifecycle events after the caller commits."""
    for channel, event_type, payload in result.events:
        await event_bus.publish(channel, event_type, payload)

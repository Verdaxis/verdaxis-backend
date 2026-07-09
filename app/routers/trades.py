import uuid
from datetime import datetime, UTC
from decimal import Decimal
from typing import Annotated

from fastapi import APIRouter, Depends, HTTPException, Request, status, Query
from sqlalchemy import select, or_, func
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import joinedload, selectinload
from uuid import UUID

from app.database import get_db
from app.routers.auth_simple import get_current_user
from app.models.user import User, UserRole
from app.models.orderbook import (
    OrderBookOrder,
    Trade,
    OrderSide,
    OrderBookStatus,
    TradeStatus,
    Initiator,
)
from app.models.notification import Notification, NotificationType
from app.schemas.orderbook import TradeCreate, TradeResponse, TradeDeliverPayload
from app.schemas.pagination import PaginatedResponse
from app.services.activity import trade_activity_provenance
from app.services.event_bus import event_bus
from app.services.watchlist_events import _best_slice_price, emit_order_updated
from app.services.execution_policy import order_is_execution_qualified
from app.services.live_benchmarks import rebuild_live_slice_benchmarks_for_keys
from app.services.demo_market import is_demo_market_organization
from app.services.audit_service import record_audit, request_audit_context
from app.services.audit_actions import (
    TRADE_CONFIRMED,
    TRADE_CREATED,
    TRADE_DECLINED,
    TRADE_DELIVERED,
    TRADE_PAID,
)
from app.schemas.errors import AUTH_RESPONSES

router = APIRouter(prefix="/trades", tags=["trades"], responses=AUTH_RESPONSES)

# One party reports delivery unilaterally, so the final price it sets must
# stay within this band around the confirmed trade price. Guards commission
# and GMV integrity until a two-sided delivery confirmation flow exists.
MAX_FINAL_PRICE_DEVIATION_PCT = Decimal("10")


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def build_trade_response(trade: Trade) -> TradeResponse:
    """Build a TradeResponse from a Trade ORM object with loaded relationships."""
    order = trade.ask_order or trade.bid_order

    buyer_name = trade.buyer.name if trade.buyer else ""
    seller_name = trade.seller.name if trade.seller else ""
    if trade.is_anonymous and trade.status == TradeStatus.PENDING_CONFIRMATION:
        buyer_name = "Anonymous"
        seller_name = "Anonymous"

    # Denormalize product/delivery_point info from the order
    product_id = None
    product_name = ""
    fuel_type = ""
    fuel_grade = ""
    delivery_point_id = None
    delivery_point_name = None
    region = ""

    if order:
        product_id = order.product_id
        product_name = order.product_name
        fuel_type = order.fuel_type
        fuel_grade = order.fuel_grade
        delivery_point_id = order.delivery_point_id
        delivery_point_name = order.delivery_point_name
        region = order.region

    return TradeResponse(
        id=trade.id,
        bid_order_id=trade.bid_order_id,
        ask_order_id=trade.ask_order_id,
        buyer_id=trade.buyer_id,
        seller_id=trade.seller_id,
        buyer_name=buyer_name,
        seller_name=seller_name,
        initiated_by=trade.initiated_by,
        is_anonymous=trade.is_anonymous,
        quantity_mt=trade.quantity_mt,
        price_per_mt_usd=trade.price_per_mt_usd,
        status=trade.status,
        final_quantity_mt=trade.final_quantity_mt,
        final_price_per_mt=trade.final_price_per_mt,
        final_total_usd=trade.final_total_usd,
        commission_rate_pct=trade.commission_rate_pct,
        commission_amount_usd=trade.commission_amount_usd,
        confirmed_at=trade.confirmed_at,
        delivered_at=trade.delivered_at,
        paid_at=trade.paid_at,
        created_at=trade.created_at,
        product_id=product_id,
        product_name=product_name,
        fuel_type=fuel_type,
        fuel_grade=fuel_grade,
        delivery_point_id=delivery_point_id,
        delivery_point_name=delivery_point_name,
        region=region,
    )


async def notify_org_users(
    db: AsyncSession,
    org_id: uuid.UUID,
    notif_type: NotificationType,
    title: str,
    message: str,
    data: dict = None,
):
    """Send a notification to every user in the given organization."""
    stmt = select(User).where(User.organization_id == org_id)
    result = await db.execute(stmt)
    users = result.scalars().all()
    for user in users:
        db.add(
            Notification(
                recipient_id=user.id,
                type=notif_type,
                title=title,
                message=message,
                data=data or {},
            )
        )


async def _load_trade(db: AsyncSession, trade_id: uuid.UUID, for_update: bool = False) -> Trade:
    """Reload a trade with all relationships needed for the response."""
    stmt = (
        select(Trade)
        .where(Trade.id == trade_id)
        .options(
            selectinload(Trade.buyer),
            selectinload(Trade.seller),
            selectinload(Trade.bid_order).selectinload(OrderBookOrder.product),
            selectinload(Trade.ask_order).selectinload(OrderBookOrder.product),
        )
    )
    if for_update:
        stmt = stmt.with_for_update()
    result = await db.execute(stmt)
    trade = result.unique().scalar_one_or_none()
    if trade is None:
        raise HTTPException(status_code=404, detail="Trade not found")
    return trade




async def _watchlist_before_state(db: AsyncSession, order: OrderBookOrder) -> dict[str, object]:
    return {
        "price_per_mt_usd": order.price_per_mt_usd,
        "remaining_quantity_mt": order.remaining_quantity_mt,
        "status": order.status,
        "slice_best_price_per_mt_usd": await _best_slice_price(
            db,
            market_product_code=order.market_product,
            delivery_point_id=order.delivery_point_id,
            availability_window_code=order.availability_window,
            side=order.side,
        ),
    }

# ---------------------------------------------------------------------------
# 1. POST / -- Hit an order to create a trade
# ---------------------------------------------------------------------------

@router.post("/", response_model=TradeResponse)
async def create_trade(
    payload: TradeCreate,
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
):
    if not current_user.organization_id:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="User must belong to an organization to trade",
        )

    # Lock the target order row to prevent concurrent over-fills.
    stmt = (
        select(OrderBookOrder)
        .options(selectinload(OrderBookOrder.product))
        .where(OrderBookOrder.id == payload.order_id)
        .with_for_update()
    )
    result = await db.execute(stmt)
    order = result.unique().scalar_one_or_none()

    if order is None:
        raise HTTPException(status_code=404, detail="Order not found")

    if order.status not in (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED):
        raise HTTPException(status_code=400, detail="Order is not available for trading")

    if order.expires_at and order.expires_at <= datetime.now(UTC):
        raise HTTPException(status_code=400, detail="Order has expired")

    if not order_is_execution_qualified(order):
        raise HTTPException(status_code=400, detail="Order is not execution-qualified")

    if is_demo_market_organization(order.organization_id):
        raise HTTPException(
            status_code=400,
            detail="Demo listings are preview liquidity and cannot be traded.",
        )

    # Determine sides
    if order.side == OrderSide.ASK:
        # User is the BUYER hitting a seller's ask
        buyer_org_id = current_user.organization_id
        seller_org_id = order.organization_id
        initiated_by = Initiator.BUYER
        ask_order_id = order.id
        bid_order_id = None
        counterparty_org_id = seller_org_id

        if current_user.role != UserRole.BUYER:
            raise HTTPException(
                status_code=403,
                detail="Only buyers can hit an ASK order",
            )
    else:
        # order.side == BID -- User is the SELLER hitting a buyer's bid
        buyer_org_id = order.organization_id
        seller_org_id = current_user.organization_id
        initiated_by = Initiator.SELLER
        bid_order_id = order.id
        ask_order_id = None
        counterparty_org_id = buyer_org_id

        if current_user.role != UserRole.SUPPLIER:
            raise HTTPException(
                status_code=403,
                detail="Only suppliers can hit a BID order",
            )

    # Prevent self-trade
    if order.organization_id == current_user.organization_id:
        raise HTTPException(status_code=400, detail="Cannot trade with your own order")

    # Quantity check
    if payload.quantity_mt > order.remaining_quantity_mt:
        raise HTTPException(
            status_code=400,
            detail=f"Requested quantity ({payload.quantity_mt}) exceeds remaining ({order.remaining_quantity_mt})",
        )

    before_state = await _watchlist_before_state(db, order)

    # Create the trade
    trade = Trade(
        bid_order_id=bid_order_id,
        ask_order_id=ask_order_id,
        buyer_id=buyer_org_id,
        seller_id=seller_org_id,
        initiated_by=initiated_by,
        quantity_mt=payload.quantity_mt,
        price_per_mt_usd=order.price_per_mt_usd,
        status=TradeStatus.PENDING_CONFIRMATION,
    )
    db.add(trade)

    # Update order remaining quantity and status
    order.remaining_quantity_mt -= payload.quantity_mt

    if order.remaining_quantity_mt == Decimal("0"):
        order.status = OrderBookStatus.FILLED
    elif order.status == OrderBookStatus.OPEN:
        order.status = OrderBookStatus.PARTIALLY_FILLED

    await rebuild_live_slice_benchmarks_for_keys(
        db,
        [(order.side, order.market_product, order.delivery_point_id, order.availability_window)],
    )

    # Flush to get the trade id
    await db.flush()
    await emit_order_updated(db, before=before_state, order=order)

    # Notify counterparty organization
    await notify_org_users(
        db,
        counterparty_org_id,
        NotificationType.ORDER_UPDATE,
        "New Trade Request",
        f"A new trade request for {payload.quantity_mt} MT at ${order.price_per_mt_usd}/MT has been placed.",
        {"trade_id": str(trade.id)},
    )

    await record_audit(
        db,
        user_id=current_user.id,
        action=TRADE_CREATED,
        resource_type="trade",
        resource_id=trade.id,
        changes={
            "order_id": str(order.id),
            "quantity_mt": str(payload.quantity_mt),
            "price_per_mt_usd": str(order.price_per_mt_usd),
        },
        **request_audit_context(request),
    )
    await db.commit()

    # Reload with relationships for response
    loaded_trade = await _load_trade(db, trade.id)

    # Emit SSE event for new trade
    _order = loaded_trade.ask_order or loaded_trade.bid_order
    await event_bus.publish("trades", "trade_created", {
        **trade_activity_provenance(loaded_trade),
        "id": str(loaded_trade.id),
        "status": loaded_trade.status.value,
        "quantity": str(loaded_trade.quantity_mt),
        "price": str(loaded_trade.price_per_mt_usd),
        "product_name": _order.product_name if _order else "",
        "fuel_type": _order.fuel_type if _order else "",
        "region": _order.region if _order else "",
    })

    return build_trade_response(loaded_trade)


# ---------------------------------------------------------------------------
# 2. GET /my -- List my trades (as buyer or seller)
# ---------------------------------------------------------------------------

@router.get("/my", response_model=PaginatedResponse[TradeResponse])
async def list_my_trades(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
    skip: int = Query(0, ge=0),
    limit: int = Query(20, ge=1, le=100),
):
    org_id = current_user.organization_id

    org_filter = or_(Trade.buyer_id == org_id, Trade.seller_id == org_id)

    # Count query (same filter, no pagination)
    count_query = select(func.count(Trade.id)).where(org_filter)
    total = (await db.execute(count_query)).scalar()

    # Data query with pagination
    stmt = (
        select(Trade)
        .where(org_filter)
        .options(
            joinedload(Trade.buyer),
            joinedload(Trade.seller),
            joinedload(Trade.bid_order),
            joinedload(Trade.ask_order),
        )
        .order_by(Trade.created_at.desc())
        .offset(skip)
        .limit(limit)
    )
    result = await db.execute(stmt)
    trades = result.unique().scalars().all()

    items = [build_trade_response(t) for t in trades]
    return PaginatedResponse(items=items, total=total, skip=skip, limit=limit)


# ---------------------------------------------------------------------------
# 3. PUT /{trade_id}/confirm -- Counterparty confirms the trade
# ---------------------------------------------------------------------------

@router.put("/{trade_id}/confirm", response_model=TradeResponse)
async def confirm_trade(
    trade_id: UUID,
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
):
    trade = await _load_trade(db, trade_id, for_update=True)
    org_id = current_user.organization_id

    if trade.status != TradeStatus.PENDING_CONFIRMATION:
        raise HTTPException(status_code=400, detail="Trade is not pending confirmation")

    # Only the counterparty (non-initiator) can confirm
    if trade.initiated_by == Initiator.BUYER:
        # Buyer initiated, so seller confirms
        if trade.seller_id != org_id:
            raise HTTPException(status_code=403, detail="Only the seller can confirm this trade")
        initiator_org_id = trade.buyer_id
    else:
        # Seller initiated, so buyer confirms
        if trade.buyer_id != org_id:
            raise HTTPException(status_code=403, detail="Only the buyer can confirm this trade")
        initiator_org_id = trade.seller_id

    trade.status = TradeStatus.CONFIRMED
    trade.confirmed_at = datetime.now(UTC)

    # Progress referrals to ACTIVE for users in buyer/seller orgs (single query)
    from app.models.referral import Referral, ReferralStatus
    ref_stmt = (
        select(Referral)
        .join(User, Referral.referred_user_id == User.id)
        .where(
            User.organization_id.in_([trade.buyer_id, trade.seller_id]),
            Referral.status == ReferralStatus.VERIFIED,
        )
    )
    now = datetime.now(UTC)
    for referral in (await db.execute(ref_stmt)).scalars():
        referral.status = ReferralStatus.ACTIVE
        referral.activated_at = now

    # Notify the initiator
    await notify_org_users(
        db,
        initiator_org_id,
        NotificationType.ORDER_UPDATE,
        "Trade Confirmed",
        f"Your trade for {trade.quantity_mt} MT has been confirmed by the counterparty.",
        {"trade_id": str(trade.id)},
    )

    await record_audit(
        db,
        user_id=current_user.id,
        action=TRADE_CONFIRMED,
        resource_type="trade",
        resource_id=trade.id,
        changes={"status": TradeStatus.CONFIRMED.value},
        **request_audit_context(request),
    )
    await db.commit()

    loaded_trade = await _load_trade(db, trade.id)

    # Emit SSE event for confirmed trade
    await event_bus.publish("trades", "trade_confirmed", {
        **trade_activity_provenance(loaded_trade),
        "id": str(loaded_trade.id),
        "status": loaded_trade.status.value,
        "quantity": str(loaded_trade.quantity_mt),
        "price": str(loaded_trade.price_per_mt_usd),
    })

    return build_trade_response(loaded_trade)


# ---------------------------------------------------------------------------
# 4. PUT /{trade_id}/decline -- Counterparty declines the trade
# ---------------------------------------------------------------------------

@router.put("/{trade_id}/decline", response_model=TradeResponse)
async def decline_trade(
    trade_id: UUID,
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
):
    # Load trade with order relationships for quantity restore
    trade = await _load_trade(db, trade_id, for_update=True)
    org_id = current_user.organization_id

    if trade.status != TradeStatus.PENDING_CONFIRMATION:
        raise HTTPException(status_code=400, detail="Trade is not pending confirmation")

    # Only the counterparty (non-initiator) can decline
    if trade.initiated_by == Initiator.BUYER:
        if trade.seller_id != org_id:
            raise HTTPException(status_code=403, detail="Only the seller can decline this trade")
        initiator_org_id = trade.buyer_id
    else:
        if trade.buyer_id != org_id:
            raise HTTPException(status_code=403, detail="Only the buyer can decline this trade")
        initiator_org_id = trade.seller_id

    trade.status = TradeStatus.DECLINED

    # Restore the order's remaining quantity. The eager-loaded relationship
    # row is not covered by the trade's FOR UPDATE lock, so re-select it
    # locked before mutating remaining_quantity_mt.
    order = trade.ask_order or trade.bid_order
    if order is not None:
        result = await db.execute(
            select(OrderBookOrder)
            .where(OrderBookOrder.id == order.id)
            .with_for_update()
            .execution_options(populate_existing=True)
        )
        order = result.scalar_one()
        if order.status in (OrderBookStatus.CANCELLED, OrderBookStatus.EXPIRED):
            # The resting order was withdrawn while the trade was pending;
            # declining must not revive it as live liquidity.
            order = None
    before_state = await _watchlist_before_state(db, order) if order is not None else None
    if order is not None:
        order.remaining_quantity_mt += trade.quantity_mt

        if order.remaining_quantity_mt == order.quantity_mt:
            order.status = OrderBookStatus.OPEN
        else:
            order.status = OrderBookStatus.PARTIALLY_FILLED
        await rebuild_live_slice_benchmarks_for_keys(
            db,
            [(order.side, order.market_product, order.delivery_point_id, order.availability_window)],
        )
        if before_state is not None:
            await emit_order_updated(db, before=before_state, order=order)

    # Notify the initiator
    await notify_org_users(
        db,
        initiator_org_id,
        NotificationType.ORDER_UPDATE,
        "Trade Declined",
        f"Your trade request for {trade.quantity_mt} MT has been declined.",
        {"trade_id": str(trade.id)},
    )

    await record_audit(
        db,
        user_id=current_user.id,
        action=TRADE_DECLINED,
        resource_type="trade",
        resource_id=trade.id,
        changes={"status": TradeStatus.DECLINED.value},
        **request_audit_context(request),
    )
    await db.commit()

    loaded_trade = await _load_trade(db, trade.id)
    return build_trade_response(loaded_trade)


# ---------------------------------------------------------------------------
# 5. PUT /{trade_id}/deliver -- Mark trade as delivered with final qty/price
# ---------------------------------------------------------------------------

@router.put("/{trade_id}/deliver", response_model=TradeResponse)
async def deliver_trade(
    trade_id: UUID,
    payload: TradeDeliverPayload,
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
):
    trade = await _load_trade(db, trade_id, for_update=True)
    org_id = current_user.organization_id

    if trade.status != TradeStatus.CONFIRMED:
        raise HTTPException(status_code=400, detail="Trade must be confirmed before delivery")

    # Either party can mark as delivered
    if org_id not in (trade.buyer_id, trade.seller_id):
        raise HTTPException(status_code=403, detail="Not authorized for this trade")
    if payload.final_quantity_mt > trade.quantity_mt:
        raise HTTPException(
            status_code=400,
            detail="final_quantity_mt cannot exceed originally traded quantity",
        )
    price_deviation_pct = (
        abs(payload.final_price_per_mt - trade.price_per_mt_usd)
        / trade.price_per_mt_usd
        * Decimal("100")
    )
    if price_deviation_pct > MAX_FINAL_PRICE_DEVIATION_PCT:
        raise HTTPException(
            status_code=400,
            detail=(
                f"final_price_per_mt deviates more than {MAX_FINAL_PRICE_DEVIATION_PCT}% "
                "from the confirmed trade price"
            ),
        )

    trade.final_quantity_mt = payload.final_quantity_mt
    trade.final_price_per_mt = payload.final_price_per_mt
    trade.final_total_usd = payload.final_quantity_mt * payload.final_price_per_mt
    trade.delivered_at = datetime.now(UTC)
    trade.status = TradeStatus.DELIVERED

    # Calculate commission on the trade (stored on the Trade record itself for Phase 1)
    trade.commission_amount_usd = trade.final_total_usd * (trade.commission_rate_pct / Decimal("100"))

    # Notify the other party
    counterparty_org_id = trade.seller_id if org_id == trade.buyer_id else trade.buyer_id

    await notify_org_users(
        db,
        counterparty_org_id,
        NotificationType.ORDER_UPDATE,
        "Trade Delivered",
        f"Trade for {trade.final_quantity_mt} MT has been marked as delivered.",
        {"trade_id": str(trade.id)},
    )

    await record_audit(
        db,
        user_id=current_user.id,
        action=TRADE_DELIVERED,
        resource_type="trade",
        resource_id=trade.id,
        changes={
            "status": TradeStatus.DELIVERED.value,
            "final_quantity_mt": str(trade.final_quantity_mt),
            "final_price_per_mt": str(trade.final_price_per_mt),
            "final_total_usd": str(trade.final_total_usd),
        },
        **request_audit_context(request),
    )
    await db.commit()

    loaded_trade = await _load_trade(db, trade.id)

    # Emit SSE event for delivered trade
    await event_bus.publish("trades", "trade_delivered", {
        **trade_activity_provenance(loaded_trade),
        "id": str(loaded_trade.id),
        "status": loaded_trade.status.value,
        "final_quantity": str(loaded_trade.final_quantity_mt),
        "final_price": str(loaded_trade.final_price_per_mt),
        "final_total": str(loaded_trade.final_total_usd),
    })

    return build_trade_response(loaded_trade)


# ---------------------------------------------------------------------------
# 6. POST /{trade_id}/pay -- Mark trade as paid
# ---------------------------------------------------------------------------

@router.post("/{trade_id}/pay", response_model=TradeResponse)
async def pay_trade(
    trade_id: UUID,
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
):
    trade = await _load_trade(db, trade_id, for_update=True)
    org_id = current_user.organization_id

    if trade.status != TradeStatus.DELIVERED:
        raise HTTPException(status_code=400, detail="Trade must be delivered before payment")

    # Only the seller can mark as paid
    if trade.seller_id != org_id:
        raise HTTPException(status_code=403, detail="Only the seller can mark a trade as paid")

    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(status_code=403, detail="Only suppliers can mark trades as paid")

    trade.status = TradeStatus.PAID
    trade.paid_at = datetime.now(UTC)

    # Notify the buyer
    await notify_org_users(
        db,
        trade.buyer_id,
        NotificationType.ORDER_UPDATE,
        "Trade Paid",
        f"Payment for trade of {trade.quantity_mt} MT has been confirmed.",
        {"trade_id": str(trade.id)},
    )

    await record_audit(
        db,
        user_id=current_user.id,
        action=TRADE_PAID,
        resource_type="trade",
        resource_id=trade.id,
        changes={"status": TradeStatus.PAID.value},
        **request_audit_context(request),
    )
    await db.commit()

    loaded_trade = await _load_trade(db, trade.id)

    # Emit SSE event for paid trade
    await event_bus.publish("trades", "trade_paid", {
        **trade_activity_provenance(loaded_trade),
        "id": str(loaded_trade.id),
        "status": loaded_trade.status.value,
        "quantity": str(loaded_trade.quantity_mt),
        "price": str(loaded_trade.price_per_mt_usd),
    })

    return build_trade_response(loaded_trade)

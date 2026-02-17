import uuid
from datetime import datetime
from decimal import Decimal
from typing import Annotated, List

from fastapi import APIRouter, Depends, HTTPException, status
from sqlalchemy import select, or_
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

router = APIRouter(prefix="/trades", tags=["trades"])


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def build_trade_response(trade: Trade) -> TradeResponse:
    """Build a TradeResponse from a Trade ORM object with loaded relationships."""
    order = trade.ask_order or trade.bid_order
    return TradeResponse(
        id=trade.id,
        bid_order_id=trade.bid_order_id,
        ask_order_id=trade.ask_order_id,
        buyer_id=trade.buyer_id,
        seller_id=trade.seller_id,
        buyer_name=trade.buyer.name if trade.buyer else "",
        seller_name=trade.seller.name if trade.seller else "",
        initiated_by=trade.initiated_by,
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
        fuel_type=order.fuel_type if order else "",
        fuel_grade=order.fuel_grade if order else None,
        region=order.region if order else "",
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
            selectinload(Trade.bid_order),
            selectinload(Trade.ask_order),
        )
    )
    if for_update:
        stmt = stmt.with_for_update()
    result = await db.execute(stmt)
    trade = result.unique().scalar_one_or_none()
    if trade is None:
        raise HTTPException(status_code=404, detail="Trade not found")
    return trade


# ---------------------------------------------------------------------------
# 1. POST / -- Hit an order to create a trade
# ---------------------------------------------------------------------------

@router.post("/", response_model=TradeResponse)
async def create_trade(
    payload: TradeCreate,
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
        .where(OrderBookOrder.id == payload.order_id)
        .with_for_update()
    )
    result = await db.execute(stmt)
    order = result.unique().scalar_one_or_none()

    if order is None:
        raise HTTPException(status_code=404, detail="Order not found")

    if order.status not in (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED):
        raise HTTPException(status_code=400, detail="Order is not available for trading")

    if order.expires_at and order.expires_at <= datetime.utcnow():
        raise HTTPException(status_code=400, detail="Order has expired")

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

    # Flush to get the trade id
    await db.flush()

    # Notify counterparty organization
    await notify_org_users(
        db,
        counterparty_org_id,
        NotificationType.ORDER_UPDATE,
        "New Trade Request",
        f"A new trade request for {payload.quantity_mt} MT at ${order.price_per_mt_usd}/MT has been placed.",
        {"trade_id": str(trade.id)},
    )

    await db.commit()

    # Reload with relationships for response
    loaded_trade = await _load_trade(db, trade.id)
    return build_trade_response(loaded_trade)


# ---------------------------------------------------------------------------
# 2. GET /my -- List my trades (as buyer or seller)
# ---------------------------------------------------------------------------

@router.get("/my", response_model=List[TradeResponse])
async def list_my_trades(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
):
    org_id = current_user.organization_id

    stmt = (
        select(Trade)
        .where(or_(Trade.buyer_id == org_id, Trade.seller_id == org_id))
        .options(
            joinedload(Trade.buyer),
            joinedload(Trade.seller),
            joinedload(Trade.bid_order),
            joinedload(Trade.ask_order),
        )
        .order_by(Trade.created_at.desc())
    )
    result = await db.execute(stmt)
    trades = result.unique().scalars().all()

    return [build_trade_response(t) for t in trades]


# ---------------------------------------------------------------------------
# 3. PUT /{trade_id}/confirm -- Counterparty confirms the trade
# ---------------------------------------------------------------------------

@router.put("/{trade_id}/confirm", response_model=TradeResponse)
async def confirm_trade(
    trade_id: UUID,
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
    trade.confirmed_at = datetime.utcnow()

    # Notify the initiator
    await notify_org_users(
        db,
        initiator_org_id,
        NotificationType.ORDER_UPDATE,
        "Trade Confirmed",
        f"Your trade for {trade.quantity_mt} MT has been confirmed by the counterparty.",
        {"trade_id": str(trade.id)},
    )

    await db.commit()

    loaded_trade = await _load_trade(db, trade.id)
    return build_trade_response(loaded_trade)


# ---------------------------------------------------------------------------
# 4. PUT /{trade_id}/decline -- Counterparty declines the trade
# ---------------------------------------------------------------------------

@router.put("/{trade_id}/decline", response_model=TradeResponse)
async def decline_trade(
    trade_id: UUID,
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

    # Restore the order's remaining quantity
    order = trade.ask_order or trade.bid_order
    if order is not None:
        order.remaining_quantity_mt += trade.quantity_mt

        if order.remaining_quantity_mt == order.quantity_mt:
            order.status = OrderBookStatus.OPEN
        else:
            order.status = OrderBookStatus.PARTIALLY_FILLED

    # Notify the initiator
    await notify_org_users(
        db,
        initiator_org_id,
        NotificationType.ORDER_UPDATE,
        "Trade Declined",
        f"Your trade request for {trade.quantity_mt} MT has been declined.",
        {"trade_id": str(trade.id)},
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

    trade.final_quantity_mt = payload.final_quantity_mt
    trade.final_price_per_mt = payload.final_price_per_mt
    trade.final_total_usd = payload.final_quantity_mt * payload.final_price_per_mt
    trade.delivered_at = datetime.utcnow()
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

    await db.commit()

    loaded_trade = await _load_trade(db, trade.id)
    return build_trade_response(loaded_trade)


# ---------------------------------------------------------------------------
# 6. POST /{trade_id}/pay -- Mark trade as paid
# ---------------------------------------------------------------------------

@router.post("/{trade_id}/pay", response_model=TradeResponse)
async def pay_trade(
    trade_id: UUID,
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
    trade.paid_at = datetime.utcnow()

    # Notify the buyer
    await notify_org_users(
        db,
        trade.buyer_id,
        NotificationType.ORDER_UPDATE,
        "Trade Paid",
        f"Payment for trade of {trade.quantity_mt} MT has been confirmed.",
        {"trade_id": str(trade.id)},
    )

    await db.commit()

    loaded_trade = await _load_trade(db, trade.id)
    return build_trade_response(loaded_trade)

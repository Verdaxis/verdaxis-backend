"""Live matchmaking router based on the user's own active orders."""
from typing import Annotated
from uuid import UUID

from fastapi import APIRouter, Depends, HTTPException
from sqlalchemy import or_, select
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import selectinload

from app.database import get_db
from app.models.matchmaking import MatchStatus, MatchSuggestion
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import User, UserRole
from app.routers.auth_simple import get_current_user
from app.middleware.execution import require_execution_eligible_user
from app.services.execution_policy import order_is_execution_qualified
from app.services.matchmaking import compute_match_score

router = APIRouter(prefix="/matchmaking", tags=["matchmaking"])

MAX_SUGGESTIONS = 20


@router.get("/suggestions")
async def list_suggestions(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
):
    """Return live match suggestions keyed off the user's own open orders."""
    if current_user.role == UserRole.BUYER:
        source_side = OrderSide.BID
        candidate_side = OrderSide.ASK
    elif current_user.role == UserRole.SUPPLIER:
        source_side = OrderSide.ASK
        candidate_side = OrderSide.BID
    else:
        return []

    if current_user.organization_id is None:
        return []

    source_stmt = (
        select(OrderBookOrder)
        .where(
            OrderBookOrder.organization_id == current_user.organization_id,
            OrderBookOrder.side == source_side,
            OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
        )
        .options(selectinload(OrderBookOrder.product), selectinload(OrderBookOrder.delivery_point))
    )
    source_orders = [order for order in (await db.execute(source_stmt)).scalars().all() if order_is_execution_qualified(order)]
    if not source_orders:
        return []

    dismissed_stmt = select(MatchSuggestion.bid_order_id, MatchSuggestion.ask_order_id).where(
        MatchSuggestion.recipient_org_id == current_user.organization_id,
        MatchSuggestion.status == MatchStatus.DISMISSED,
    )
    dismissed_result = await db.execute(dismissed_stmt)
    dismissed_order_ids: set[UUID] = set()
    for bid_id, ask_id in dismissed_result.all():
        if bid_id:
            dismissed_order_ids.add(bid_id)
        if ask_id:
            dismissed_order_ids.add(ask_id)

    candidate_stmt = (
        select(OrderBookOrder)
        .where(
            OrderBookOrder.side == candidate_side,
            OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
            OrderBookOrder.organization_id != current_user.organization_id,
            OrderBookOrder.off_spec.is_(False),
        )
        .options(selectinload(OrderBookOrder.product), selectinload(OrderBookOrder.delivery_point))
    )
    candidate_orders = [
        order
        for order in (await db.execute(candidate_stmt)).scalars().all()
        if order.id not in dismissed_order_ids and order_is_execution_qualified(order)
    ]
    if not candidate_orders:
        return []

    best_by_candidate: dict[UUID, tuple[OrderBookOrder, OrderBookOrder, float, list[str]]] = {}
    for source_order in source_orders:
        if not source_order.market_product or not source_order.delivery_point_id:
            continue
        for candidate in candidate_orders:
            score_points, reasons = compute_match_score(
                target_market_product=source_order.market_product,
                candidate_market_product=candidate.market_product,
                target_delivery_point_id=str(source_order.delivery_point_id) if source_order.delivery_point_id else None,
                candidate_delivery_point_id=str(candidate.delivery_point_id) if candidate.delivery_point_id else None,
                target_price=source_order.price_per_mt_usd,
                candidate_price=candidate.price_per_mt_usd,
                target_qty=source_order.remaining_quantity_mt,
                candidate_qty=candidate.remaining_quantity_mt,
                target_availability_window=source_order.availability_window,
                candidate_availability_window=candidate.availability_window,
                target_certification_scheme=source_order.certification_scheme,
                candidate_off_spec=candidate.off_spec,
                candidate_side=candidate.side.value,
                candidate_certification_declared=candidate.certification_declared,
                candidate_certification_scheme=candidate.certification_scheme,
                candidate_specification_standard=candidate.specification_standard,
                candidate_msds_available=candidate.msds_available,
            )
            if score_points <= 0:
                continue
            existing = best_by_candidate.get(candidate.id)
            if existing is None or score_points > existing[2]:
                best_by_candidate[candidate.id] = (source_order, candidate, float(score_points), reasons)

    ranked = sorted(best_by_candidate.values(), key=lambda item: item[2], reverse=True)[:MAX_SUGGESTIONS]

    results = []
    for source_order, candidate, score, reasons in ranked:
        if candidate.side == OrderSide.ASK:
            bid_order_id = source_order.id if source_order.side == OrderSide.BID else None
            ask_order_id = candidate.id
            bid_order = source_order if source_order.side == OrderSide.BID else None
            ask_order = candidate
        else:
            bid_order_id = candidate.id
            ask_order_id = source_order.id if source_order.side == OrderSide.ASK else None
            bid_order = candidate
            ask_order = source_order if source_order.side == OrderSide.ASK else None

        results.append({
            "id": str(candidate.id),
            "bid_order_id": str(bid_order_id) if bid_order_id else None,
            "ask_order_id": str(ask_order_id) if ask_order_id else None,
            "score": score,
            "match_reasons": reasons,
            "status": "SUGGESTED",
            "recipient_org_id": str(current_user.organization_id),
            "created_at": candidate.created_at.isoformat() if candidate.created_at else None,
            "bid_order": bid_order,
            "ask_order": ask_order,
        })

    return results


@router.patch("/suggestions/{order_id}/dismiss")
async def dismiss_suggestion(
    order_id: UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_execution_eligible_user)],
):
    """Dismiss a recommendation by order ID."""
    order = (await db.execute(select(OrderBookOrder).where(OrderBookOrder.id == order_id))).scalar_one_or_none()
    if not order:
        raise HTTPException(status_code=404, detail="Order not found")

    existing_stmt = select(MatchSuggestion).where(
        MatchSuggestion.recipient_org_id == current_user.organization_id,
        MatchSuggestion.status == MatchStatus.DISMISSED,
        or_(
            MatchSuggestion.bid_order_id == order_id,
            MatchSuggestion.ask_order_id == order_id,
        ),
    )
    if (await db.execute(existing_stmt)).scalar_one_or_none():
        return {"status": "already_dismissed"}

    dismissal = MatchSuggestion(
        bid_order_id=order.id,
        ask_order_id=order.id,
        score=0,
        match_reasons=[],
        status=MatchStatus.DISMISSED,
        recipient_org_id=current_user.organization_id,
    )
    db.add(dismissal)
    await db.commit()
    return {"status": "dismissed"}

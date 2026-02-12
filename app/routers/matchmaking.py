"""
Matchmaking router.
- GET /matchmaking/suggestions -- list suggestions for current user's org
- POST /matchmaking/generate/{order_id} -- trigger match generation for an order
- PATCH /matchmaking/suggestions/{id}/dismiss -- dismiss a suggestion
"""
from typing import Annotated
from uuid import UUID

from fastapi import APIRouter, Depends, HTTPException
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import joinedload

from app.database import get_db
from app.routers.auth_simple import get_current_user
from app.models.user import User
from app.models.orderbook import OrderBookOrder, OrderSide, OrderBookStatus
from app.models.matchmaking import MatchSuggestion, MatchStatus
from app.models.notification import Notification, NotificationType
from app.services.matchmaking import compute_match_score

router = APIRouter(prefix="/matchmaking", tags=["matchmaking"])


@router.get("/suggestions")
async def list_suggestions(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
):
    """List match suggestions for current user's organization."""
    stmt = (
        select(MatchSuggestion)
        .where(
            MatchSuggestion.recipient_org_id == current_user.organization_id,
            MatchSuggestion.status.in_([MatchStatus.SUGGESTED, MatchStatus.VIEWED]),
        )
        .options(
            joinedload(MatchSuggestion.bid_order),
            joinedload(MatchSuggestion.ask_order),
        )
        .order_by(MatchSuggestion.score.desc())
        .limit(20)
    )
    result = await db.execute(stmt)
    suggestions = result.unique().scalars().all()
    return suggestions


@router.post("/generate/{order_id}")
async def generate_matches(
    order_id: UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
):
    """
    Generate match suggestions for a given order against all
    compatible orders on the opposite side.
    """
    stmt = select(OrderBookOrder).where(OrderBookOrder.id == order_id)
    result = await db.execute(stmt)
    source_order = result.scalar_one_or_none()
    if not source_order:
        raise HTTPException(status_code=404, detail="Order not found")
    if source_order.organization_id != current_user.organization_id:
        raise HTTPException(status_code=403, detail="Not your order")

    opposite_side = OrderSide.ASK if source_order.side == OrderSide.BID else OrderSide.BID
    opposite_stmt = (
        select(OrderBookOrder)
        .where(
            OrderBookOrder.side == opposite_side,
            OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
            OrderBookOrder.organization_id != source_order.organization_id,
        )
    )
    opposite_result = await db.execute(opposite_stmt)
    candidates = opposite_result.scalars().all()

    created = 0
    for candidate in candidates:
        if source_order.side == OrderSide.BID:
            bid, ask = source_order, candidate
        else:
            bid, ask = candidate, source_order

        score, reasons = compute_match_score(
            bid_fuel=bid.fuel_type, ask_fuel=ask.fuel_type,
            bid_region=bid.region, ask_region=ask.region,
            bid_price=bid.price_per_mt_usd, ask_price=ask.price_per_mt_usd,
            bid_qty=bid.remaining_quantity_mt, ask_qty=ask.remaining_quantity_mt,
            bid_delivery_start=bid.delivery_window_start, bid_delivery_end=bid.delivery_window_end,
            ask_delivery_start=ask.delivery_window_start, ask_delivery_end=ask.delivery_window_end,
        )

        if score >= 30:
            suggestion = MatchSuggestion(
                bid_order_id=bid.id,
                ask_order_id=ask.id,
                score=score,
                match_reasons=reasons,
                recipient_org_id=current_user.organization_id,
            )
            db.add(suggestion)
            created += 1

    if created > 0:
        db.add(Notification(
            recipient_id=current_user.id,
            type=NotificationType.MATCH_SUGGESTION,
            title="New Match Suggestions",
            message=f"We found {created} potential match{'es' if created > 1 else ''} for your order.",
            data={"order_id": str(order_id), "match_count": created},
        ))

    await db.commit()
    return {"matches_created": created}


@router.patch("/suggestions/{suggestion_id}/dismiss")
async def dismiss_suggestion(
    suggestion_id: UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
):
    """Dismiss a match suggestion."""
    stmt = select(MatchSuggestion).where(
        MatchSuggestion.id == suggestion_id,
        MatchSuggestion.recipient_org_id == current_user.organization_id,
    )
    result = await db.execute(stmt)
    suggestion = result.scalar_one_or_none()
    if not suggestion:
        raise HTTPException(status_code=404, detail="Suggestion not found")
    suggestion.status = MatchStatus.DISMISSED
    await db.commit()
    return {"status": "dismissed"}

"""
Watchlist-driven matchmaking router.

Suggestions are computed LIVE from the user's watchlist entries — no stored
suggestion rows.  Each watchlist entry (product_id + optional delivery_point_id)
is matched against open opposite-side orders.

- GET  /matchmaking/suggestions               — live recommendations
- PATCH /matchmaking/suggestions/{id}/dismiss  — dismiss an order (by order ID)
"""
from typing import Annotated
from uuid import UUID

from fastapi import APIRouter, Depends, HTTPException
from sqlalchemy import select, and_, or_
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import selectinload

from app.database import get_db
from app.routers.auth_simple import get_current_user
from app.models.user import User, UserRole
from app.models.orderbook import OrderBookOrder, OrderSide, OrderBookStatus
from app.models.matchmaking import MatchSuggestion, MatchStatus
from app.models.watchlist import Watchlist, WatchlistEntry
from app.services.availability_windows import SPOT_WINDOW, normalize_availability_window

router = APIRouter(prefix="/matchmaking", tags=["matchmaking"])

MAX_SUGGESTIONS = 20


@router.get("/suggestions")
async def list_suggestions(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
):
    """
    Live match suggestions based on the user's watchlist entries.

    For each watchlist entry (product + optional delivery point), find open
    opposite-side orders, score them, and return the top results.
    """
    # 1. Determine which side of the orderbook to show
    if current_user.role == UserRole.BUYER:
        target_side = OrderSide.ASK
    elif current_user.role == UserRole.SUPPLIER:
        target_side = OrderSide.BID
    else:
        # ADMIN or unknown — return empty
        return []

    # 2. Fetch all watchlist entries for this user
    wl_stmt = (
        select(WatchlistEntry)
        .join(Watchlist, WatchlistEntry.watchlist_id == Watchlist.id)
        .where(Watchlist.user_id == current_user.id)
    )
    wl_result = await db.execute(wl_stmt)
    entries = wl_result.scalars().all()

    if not entries:
        return []

    # 3. Build unique (product_id, delivery_point_id) combos
    combos: set[tuple[UUID, UUID | None]] = set()
    for entry in entries:
        combos.add((entry.product_id, entry.delivery_point_id))

    # 4. Fetch dismissed order IDs for this org (from MatchSuggestion table)
    dismissed_stmt = select(MatchSuggestion.bid_order_id, MatchSuggestion.ask_order_id).where(
        MatchSuggestion.recipient_org_id == current_user.organization_id,
        MatchSuggestion.status == MatchStatus.DISMISSED,
    )
    dismissed_result = await db.execute(dismissed_stmt)
    dismissed_order_ids: set[UUID] = set()
    for bid_id, ask_id in dismissed_result.all():
        dismissed_order_ids.add(bid_id)
        dismissed_order_ids.add(ask_id)

    # 5. Build filter conditions from watchlist combos
    combo_filters = []
    for product_id, delivery_point_id in combos:
        if delivery_point_id is not None:
            combo_filters.append(
                and_(
                    OrderBookOrder.product_id == product_id,
                    OrderBookOrder.delivery_point_id == delivery_point_id,
                )
            )
        else:
            # No location constraint — match all locations for this product
            combo_filters.append(OrderBookOrder.product_id == product_id)

    if not combo_filters:
        return []

    # 6. Query opposite-side open orders matching any watchlist combo
    order_stmt = (
        select(OrderBookOrder)
        .where(
            OrderBookOrder.side == target_side,
            OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
            OrderBookOrder.organization_id != current_user.organization_id,
            or_(*combo_filters),
        )
        .options(
            selectinload(OrderBookOrder.product),
            selectinload(OrderBookOrder.delivery_point),
        )
    )
    order_result = await db.execute(order_stmt)
    candidates = order_result.scalars().all()

    # 7. Filter out dismissed orders
    candidates = [c for c in candidates if c.id not in dismissed_order_ids]

    if not candidates:
        return []

    # 8. Score each candidate
    #    We need a "synthetic" order from the user's side to score against.
    #    Use the watchlist combo metadata + generous defaults.
    scored = []
    for order in candidates:
        # Find the watchlist combo that matched this order
        order_product_id = order.product_id
        order_dp_id = order.delivery_point_id

        # Score using the order's own data against itself (perfect fuel + region match).
        # What differentiates scores is price/volume/delivery window overlap.
        # Since we don't have a user's bid to compare against, score based on
        # how well the order fits the market (completeness of data).
        score_points = 30  # Base: fuel type matched via watchlist
        reasons = ["fuel_match"]

        # Region match (if watchlist entry had a delivery point)
        matched_with_location = any(
            dp_id is not None and dp_id == order_dp_id
            for pid, dp_id in combos
            if pid == order_product_id
        )
        if matched_with_location:
            score_points += 25
            reasons.append("region_match")
        elif order_dp_id is not None:
            # Watchlist had no location but order has one — partial credit
            score_points += 10
            reasons.append("region_available")

        # Volume — reward larger quantities (more meaningful)
        qty = float(order.remaining_quantity_mt or 0)
        if qty >= 1000:
            score_points += 10
            reasons.append("volume_significant")
        elif qty >= 100:
            score_points += 5
            reasons.append("volume_available")

        # Availability window — reward prompt liquidity slightly higher
        normalized_window = normalize_availability_window(str(order.availability_window))
        if normalized_window == SPOT_WINDOW:
            score_points += 10
            reasons.append("spot_window")
        else:
            score_points += 5
            reasons.append("availability_defined")

        # Price available
        if order.price_per_mt_usd and float(order.price_per_mt_usd) > 0:
            score_points += 15
            reasons.append("price_available")

        # Certifications / green premium
        if order.certifications:
            score_points += 5
            reasons.append("certified")

        # Cap at 100
        score_points = min(score_points, 100)

        scored.append((order, score_points, reasons))

    # 9. Sort by score descending, take top N
    scored.sort(key=lambda x: x[1], reverse=True)
    top = scored[:MAX_SUGGESTIONS]

    # 10. Build response in MatchSuggestion-compatible shape
    results = []
    for order, score, reasons in top:
        # Construct bid/ask IDs based on which side the order is
        if target_side == OrderSide.ASK:
            bid_order_id = None
            ask_order_id = order.id
            bid_order = None
            ask_order = order
        else:
            bid_order_id = order.id
            ask_order_id = None
            bid_order = order
            ask_order = None

        results.append({
            "id": str(order.id),
            "bid_order_id": str(bid_order_id) if bid_order_id else None,
            "ask_order_id": str(ask_order_id) if ask_order_id else None,
            "score": score,
            "match_reasons": reasons,
            "status": "SUGGESTED",
            "recipient_org_id": str(current_user.organization_id),
            "created_at": order.created_at.isoformat() if order.created_at else None,
            "bid_order": bid_order,
            "ask_order": ask_order,
        })

    return results


@router.patch("/suggestions/{order_id}/dismiss")
async def dismiss_suggestion(
    order_id: UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
):
    """
    Dismiss a recommendation by order ID.

    Creates a DISMISSED record in MatchSuggestion so the order won't appear
    in future live suggestions for this org.
    """
    # Verify the order exists
    order_stmt = select(OrderBookOrder).where(OrderBookOrder.id == order_id)
    order_result = await db.execute(order_stmt)
    order = order_result.scalar_one_or_none()
    if not order:
        raise HTTPException(status_code=404, detail="Order not found")

    # Check if already dismissed
    existing_stmt = select(MatchSuggestion).where(
        MatchSuggestion.recipient_org_id == current_user.organization_id,
        MatchSuggestion.status == MatchStatus.DISMISSED,
        or_(
            MatchSuggestion.bid_order_id == order_id,
            MatchSuggestion.ask_order_id == order_id,
        ),
    )
    existing_result = await db.execute(existing_stmt)
    if existing_result.scalar_one_or_none():
        return {"status": "already_dismissed"}

    # Create a dismissal record
    if order.side == OrderSide.BID:
        dismissal = MatchSuggestion(
            bid_order_id=order.id,
            ask_order_id=order.id,  # Placeholder — we just need the ID for filtering
            score=0,
            match_reasons=[],
            status=MatchStatus.DISMISSED,
            recipient_org_id=current_user.organization_id,
        )
    else:
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

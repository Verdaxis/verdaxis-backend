"""
Public demand signals endpoint for suppliers.
Aggregates open BID orders into anonymized demand signals by fuel_type + region.
"""
from typing import Optional, List
from datetime import datetime

from fastapi import APIRouter, Depends, Query
from sqlalchemy import select, func
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.models.orderbook import OrderBookOrder, OrderSide, OrderBookStatus
from app.schemas.demand import DemandSignal, UrgencyLevel

router = APIRouter(prefix="/demand", tags=["demand"])


def _classify_urgency(availability_window: str, delivery_start) -> UrgencyLevel:
    if availability_window == "Spot":
        return UrgencyLevel.HIGH
    if delivery_start:
        days_until = (delivery_start - datetime.utcnow().date()).days
        if days_until <= 30:
            return UrgencyLevel.HIGH
        elif days_until <= 90:
            return UrgencyLevel.MEDIUM
    # Forward deliveries
    if "Forward" in (availability_window or ""):
        return UrgencyLevel.LOW
    return UrgencyLevel.MEDIUM


@router.get("", response_model=List[DemandSignal])
async def get_demand_signals(
    fuel_type: Optional[str] = Query(None),
    region: Optional[str] = Query(None),
    db: AsyncSession = Depends(get_db),
):
    """
    Public: anonymized demand signals from buyer bids.
    Aggregated by fuel_type + region.
    """
    stmt = (
        select(
            OrderBookOrder.fuel_type,
            OrderBookOrder.region,
            func.sum(OrderBookOrder.remaining_quantity_mt).label("total_volume"),
            func.max(OrderBookOrder.price_per_mt_usd).label("max_price"),
            func.count(OrderBookOrder.id).label("bid_count"),
            func.min(OrderBookOrder.availability_window).label("earliest_window"),
            func.min(OrderBookOrder.delivery_window_start).label("earliest_delivery_start"),
            func.max(OrderBookOrder.created_at).label("latest_created"),
        )
        .where(
            OrderBookOrder.side == OrderSide.BID,
            OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
        )
        .group_by(OrderBookOrder.fuel_type, OrderBookOrder.region)
    )

    if fuel_type:
        stmt = stmt.where(OrderBookOrder.fuel_type.ilike(f"%{fuel_type}%"))
    if region:
        stmt = stmt.where(OrderBookOrder.region.ilike(f"%{region}%"))

    result = await db.execute(stmt)
    rows = result.all()

    signals = []
    for row in rows:
        urgency = _classify_urgency(
            row.earliest_window,
            row.earliest_delivery_start,
        )
        signals.append(
            DemandSignal(
                fuel_type=row.fuel_type,
                region=row.region,
                volume_mt=row.total_volume,
                max_price_per_mt=row.max_price,
                urgency=urgency,
                bid_count=row.bid_count,
                earliest_delivery=row.earliest_window or "Spot",
                created_at=row.latest_created,
            )
        )

    # Sort by urgency (HIGH first), then volume descending
    urgency_order = {UrgencyLevel.HIGH: 0, UrgencyLevel.MEDIUM: 1, UrgencyLevel.LOW: 2}
    signals.sort(key=lambda s: (urgency_order.get(s.urgency, 2), -s.volume_mt))

    return signals

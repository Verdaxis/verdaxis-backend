"""
Public demand signals endpoint for suppliers.
Aggregates open BID orders into anonymized demand signals by fuel_type + region.
"""
from typing import Optional, List
from datetime import date

from fastapi import APIRouter, Depends, Query
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.models.orderbook import OrderBookOrder, OrderSide, OrderBookStatus
from app.models.catalog import Product, DeliveryPoint
from app.schemas.demand import DemandSignal, UrgencyLevel
from app.services.availability_windows import (
    SPOT_WINDOW,
    availability_window_display_label,
    normalize_availability_window,
    window_start_date,
)

router = APIRouter(prefix="/demand", tags=["demand"])


def _classify_urgency(availability_window: str) -> UrgencyLevel:
    normalized = normalize_availability_window(availability_window)
    if normalized == SPOT_WINDOW:
        return UrgencyLevel.HIGH

    days_until = (window_start_date(normalized, today=date.today()) - date.today()).days
    if days_until <= 30:
        return UrgencyLevel.HIGH
    if days_until <= 90:
        return UrgencyLevel.MEDIUM
    return UrgencyLevel.LOW


@router.get("", response_model=List[DemandSignal])
async def get_demand_signals(
    fuel_type: Optional[str] = Query(None),
    region: Optional[str] = Query(None),
    db: AsyncSession = Depends(get_db),
):
    """
    Public: anonymized demand signals from buyer bids.
    Aggregated by tradable market product + delivery point + availability window.
    """
    stmt = (
        select(
            Product.fuel_type.label("fuel_type"),
            Product.market_product.label("market_product_code"),
            DeliveryPoint.id.label("delivery_point_id"),
            DeliveryPoint.name.label("delivery_point_name"),
            DeliveryPoint.region.label("region"),
            OrderBookOrder.remaining_quantity_mt.label("remaining_quantity_mt"),
            OrderBookOrder.price_per_mt_usd.label("price_per_mt_usd"),
            OrderBookOrder.availability_window.label("availability_window"),
            OrderBookOrder.created_at.label("created_at"),
        )
        .join(Product, OrderBookOrder.product_id == Product.id)
        .outerjoin(DeliveryPoint, OrderBookOrder.delivery_point_id == DeliveryPoint.id)
        .where(
            OrderBookOrder.side == OrderSide.BID,
            OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
        )
    )

    if fuel_type:
        stmt = stmt.where(Product.fuel_type.ilike(f"%{fuel_type}%"))
    if region:
        stmt = stmt.where(DeliveryPoint.region.ilike(f"%{region}%"))

    result = await db.execute(stmt)
    rows = result.all()

    groups: dict[tuple[str, str, str], dict[str, object]] = {}
    for row in rows:
        normalized_window = normalize_availability_window(str(row.availability_window))
        key = (
            str(row.market_product_code or ""),
            str(row.delivery_point_id or ""),
            normalized_window,
        )
        if key not in groups:
            groups[key] = {
                "fuel_type": row.fuel_type or "",
                "region": row.region or "",
                "market_product_code": row.market_product_code,
                "delivery_point_id": row.delivery_point_id,
                "delivery_point_name": row.delivery_point_name,
                "volume_mt": row.remaining_quantity_mt,
                "max_price_per_mt": row.price_per_mt_usd,
                "bid_count": 1,
                "earliest_window": normalized_window,
                "created_at": row.created_at,
            }
            continue

        group = groups[key]
        group["volume_mt"] += row.remaining_quantity_mt
        group["max_price_per_mt"] = max(group["max_price_per_mt"], row.price_per_mt_usd)
        group["bid_count"] += 1
        if row.created_at > group["created_at"]:
            group["created_at"] = row.created_at

    signals = [
        DemandSignal(
            fuel_type=group["fuel_type"],
            region=group["region"],
            market_product_code=group["market_product_code"],
            delivery_point_id=group["delivery_point_id"],
            delivery_point_name=group["delivery_point_name"],
            availability_window_code=group["earliest_window"],
            volume_mt=group["volume_mt"],
            max_price_per_mt=group["max_price_per_mt"],
            urgency=_classify_urgency(group["earliest_window"]),
            bid_count=group["bid_count"],
            earliest_delivery=availability_window_display_label(group["earliest_window"]),
            created_at=group["created_at"],
        )
        for group in groups.values()
    ]

    # Sort by urgency (HIGH first), then volume descending
    urgency_order = {UrgencyLevel.HIGH: 0, UrgencyLevel.MEDIUM: 1, UrgencyLevel.LOW: 2}
    signals.sort(key=lambda s: (urgency_order.get(s.urgency, 2), -s.volume_mt))

    return signals

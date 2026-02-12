"""
Public price discovery endpoint.
Aggregates confirmed/delivered/paid trades into price summaries by fuel_type + region.
No authentication required -- this feeds the public price ticker.
"""
from datetime import datetime, timedelta
from decimal import Decimal
from typing import Optional

from fastapi import APIRouter, Depends, Query
from sqlalchemy import select, func
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.models.orderbook import Trade, TradeStatus, OrderBookOrder
from app.schemas.orderbook import PriceSummary, PriceDiscoveryResponse

router = APIRouter(prefix="/prices", tags=["price-discovery"])


async def aggregate_trade_prices(
    db: AsyncSession,
    fuel_type: Optional[str] = None,
    region: Optional[str] = None,
    hours: int = 24,
) -> list[PriceSummary]:
    """
    Aggregate confirmed+ trades from the last `hours` hours into
    PriceSummary objects grouped by (fuel_type, region).

    Derives fuel_type and region from the linked OrderBookOrder
    (via ask_order or bid_order).
    """
    cutoff = datetime.utcnow() - timedelta(hours=hours)
    valid_statuses = [
        TradeStatus.CONFIRMED,
        TradeStatus.DELIVERED,
        TradeStatus.PAID,
    ]

    stmt = (
        select(
            OrderBookOrder.fuel_type.label("fuel_type"),
            OrderBookOrder.region.label("region"),
            func.max(Trade.price_per_mt_usd).label("high"),
            func.min(Trade.price_per_mt_usd).label("low"),
            func.avg(Trade.price_per_mt_usd).label("avg_price"),
            func.sum(Trade.quantity_mt).label("total_volume"),
            func.count(Trade.id).label("trade_count"),
            func.max(Trade.created_at).label("last_trade_at"),
        )
        .join(
            OrderBookOrder,
            (Trade.ask_order_id == OrderBookOrder.id) | (Trade.bid_order_id == OrderBookOrder.id),
        )
        .where(
            Trade.status.in_(valid_statuses),
            Trade.created_at >= cutoff,
        )
    )

    if fuel_type:
        stmt = stmt.where(OrderBookOrder.fuel_type.ilike(f"%{fuel_type}%"))
    if region:
        stmt = stmt.where(OrderBookOrder.region.ilike(f"%{region}%"))

    stmt = stmt.group_by(OrderBookOrder.fuel_type, OrderBookOrder.region)

    result = await db.execute(stmt)
    rows = result.all()

    summaries: list[PriceSummary] = []
    for row in rows:
        summaries.append(
            PriceSummary(
                fuel_type=row.fuel_type,
                region=row.region,
                last_price=row.high,
                avg_price_24h=Decimal(str(round(row.avg_price, 2))) if row.avg_price else None,
                high_24h=row.high,
                low_24h=row.low,
                volume_24h=row.total_volume or Decimal("0"),
                trade_count_24h=row.trade_count or 0,
                price_change_pct=None,
                last_trade_at=row.last_trade_at,
            )
        )

    return summaries


@router.get("", response_model=PriceDiscoveryResponse)
async def get_price_summaries(
    fuel_type: Optional[str] = Query(None, description="Filter by fuel type"),
    region: Optional[str] = Query(None, description="Filter by region"),
    hours: int = Query(24, ge=1, le=168, description="Lookback window in hours"),
    db: AsyncSession = Depends(get_db),
):
    """
    Public endpoint: aggregated trade prices by fuel_type + region.
    No auth required. Feeds the public-site price ticker.
    """
    summaries = await aggregate_trade_prices(db, fuel_type=fuel_type, region=region, hours=hours)
    return PriceDiscoveryResponse(
        summaries=summaries,
        generated_at=datetime.utcnow(),
    )

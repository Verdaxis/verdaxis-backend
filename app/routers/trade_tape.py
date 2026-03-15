"""Public, read-only anonymized trade tape endpoint."""

from datetime import datetime, timedelta, timezone
from decimal import Decimal
from typing import Optional

from fastapi import APIRouter, Depends, Query
from sqlalchemy import select, func
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import joinedload

from app.database import get_db
from app.models.orderbook import Trade, TradeStatus, OrderBookOrder
from app.schemas.trade_tape import TradeTapeEntry, TradeTapeResponse

router = APIRouter(prefix="/trade-tape", tags=["trade-tape"])


def _is_market_hours(now: datetime) -> bool:
    """Return True if current UTC hour is within 08:00-18:00."""
    return 8 <= now.hour < 18


def _build_tape_entry(trade: Trade) -> TradeTapeEntry:
    """Build an anonymized tape entry from a Trade with loaded relationships."""
    order = trade.ask_order or trade.bid_order

    fuel_type = ""
    fuel_grade = ""
    region = ""
    availability_window = ""

    if order:
        fuel_type = order.fuel_type
        fuel_grade = order.fuel_grade
        region = order.region
        availability_window = order.availability_window.value if order.availability_window else ""

    return TradeTapeEntry(
        id=str(trade.id).replace("-", "")[:8],
        fuel_type=fuel_type,
        fuel_grade=fuel_grade,
        region=region,
        quantity_mt=trade.quantity_mt,
        price_per_mt_usd=trade.price_per_mt_usd,
        total_usd=trade.quantity_mt * trade.price_per_mt_usd,
        confirmed_at=trade.confirmed_at,
        availability_window=availability_window,
    )


@router.get("", response_model=TradeTapeResponse)
async def get_trade_tape(
    db: AsyncSession = Depends(get_db),
    fuel_type: Optional[str] = Query(None, description="Filter by fuel type"),
    region: Optional[str] = Query(None, description="Filter by region"),
    skip: int = Query(0, ge=0),
    limit: int = Query(50, ge=1, le=200),
):
    """
    Public trade tape — anonymized confirmed trades from the last 24 hours.

    During market hours (08:00-18:00 UTC) trades are shown in real-time.
    Outside market hours, trades are delayed by 1 hour.
    """
    now = datetime.now(timezone.utc)
    market_hours = _is_market_hours(now)

    if market_hours:
        # Real-time: trades confirmed in the last 24h
        cutoff = now - timedelta(hours=24)
        confirmed_before = None
    else:
        # Delayed: trades confirmed in last 25h but only those older than 1h
        cutoff = now - timedelta(hours=25)
        confirmed_before = now - timedelta(hours=1)

    # Base filter: confirmed trades after cutoff
    conditions = [
        Trade.status == TradeStatus.CONFIRMED,
        Trade.confirmed_at >= cutoff,
    ]
    if confirmed_before is not None:
        conditions.append(Trade.confirmed_at <= confirmed_before)

    # Build base query with joins for filtering
    base_query = (
        select(Trade)
        .outerjoin(OrderBookOrder, (Trade.ask_order_id == OrderBookOrder.id) | (Trade.bid_order_id == OrderBookOrder.id))
        .where(*conditions)
        .options(
            joinedload(Trade.ask_order),
            joinedload(Trade.bid_order),
        )
    )

    # Apply optional filters via joined OrderBookOrder
    if fuel_type is not None:
        from app.models.catalog import Product
        base_query = base_query.join(Product, OrderBookOrder.product_id == Product.id).where(
            Product.fuel_type == fuel_type
        )
    if region is not None:
        from app.models.catalog import DeliveryPoint
        base_query = base_query.join(DeliveryPoint, OrderBookOrder.delivery_point_id == DeliveryPoint.id).where(
            DeliveryPoint.region == region
        )

    # Count query
    count_stmt = select(func.count()).select_from(base_query.subquery())
    total = (await db.execute(count_stmt)).scalar() or 0

    # Data query with pagination
    data_stmt = (
        base_query
        .order_by(Trade.confirmed_at.desc())
        .offset(skip)
        .limit(limit)
    )
    result = await db.execute(data_stmt)
    trades = result.unique().scalars().all()

    items = [_build_tape_entry(t) for t in trades]

    return TradeTapeResponse(items=items, total=total, market_hours=market_hours)

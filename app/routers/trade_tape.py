"""Public, read-only anonymized trade tape endpoint."""

from datetime import datetime, timedelta, timezone
from typing import Optional
from uuid import UUID

from fastapi import APIRouter, Depends, Query
from sqlalchemy import func, or_, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.models.orderbook import Trade
from app.schemas.trade_tape import TradeTapeEntry, TradeTapeResponse
from app.services.availability_windows import normalize_availability_window
from app.services.market_provenance import public_trade_evidence_clause, trade_market_provenance

router = APIRouter(prefix="/trade-tape", tags=["trade-tape"])


def _is_market_hours(now: datetime) -> bool:
    """Verdaxis physical fuel markets are presented without exchange-session delay."""
    return True


def _build_tape_entry(trade: Trade, *, expose_delivery_point: bool = False) -> TradeTapeEntry:
    """Build an anonymized entry exclusively from immutable trade snapshots."""
    provenance = trade_market_provenance(trade)
    is_demo_trade = provenance["demo_status"] == "DEMO_ONLY"
    product_id = trade.product_id if expose_delivery_point else None
    delivery_point_id = trade.delivery_point_id if expose_delivery_point else None
    delivery_point_name = trade.delivery_point_name if expose_delivery_point else None
    region = (trade.delivery_point_name or "") if expose_delivery_point else (trade.delivery_point_region or "")
    availability_window = normalize_availability_window(str(trade.availability_window))

    return TradeTapeEntry(
        id=str(trade.id).replace("-", "")[:8],
        product_id=product_id,
        market_product=trade.market_product,
        fuel_type=trade.fuel_type or "",
        fuel_grade=trade.fuel_grade or "",
        delivery_point_id=delivery_point_id,
        delivery_point_name=delivery_point_name,
        region=region,
        quantity_mt=trade.quantity_mt,
        price_per_mt_usd=trade.price_per_mt_usd,
        total_usd=trade.quantity_mt * trade.price_per_mt_usd,
        confirmed_at=trade.confirmed_at,
        availability_window=availability_window,
        is_demo_trade=is_demo_trade,
        scope=("DELIVERY_POINT" if expose_delivery_point and delivery_point_id else ("REGION" if region else "UNKNOWN")),
        provenance_kind=provenance["source_kind"],
        source_kind=provenance["source_kind"],
        demo_status=provenance["demo_status"],
    )


@router.get("", response_model=TradeTapeResponse)
async def get_trade_tape(
    db: AsyncSession = Depends(get_db),
    fuel_type: Optional[str] = Query(None, description="Filter by fuel type"),
    market_product: Optional[str] = Query(None, description="Filter by canonical market product"),
    delivery_point_id: Optional[UUID] = Query(None, description="Filter by exact delivery point ID"),
    region: Optional[str] = Query(None, description="Filter by region"),
    availability_window: Optional[str] = Query(None, description="Filter by availability window"),
    skip: int = Query(0, ge=0),
    limit: int = Query(50, ge=1, le=200),
):
    """
    Public trade tape — anonymized confirmed trades from the last 7 days.

    Confirmed trades are shown without an exchange-hours delay; the lookback
    window is intentionally kept separate from market availability semantics.
    """
    now = datetime.now(timezone.utc)
    market_hours = _is_market_hours(now)

    cutoff = now - timedelta(days=7)

    # Join a single canonical display order per trade. Prefer the ASK order when present,
    # otherwise fall back to the BID order. This avoids double-counting trades that have both.
    base_query = (
        select(Trade)
        .where(
            public_trade_evidence_clause(
                Trade,
                confirmed_since=cutoff,
            ),
        )
    )

    # Apply optional filters via immutable trade snapshots. Product joins are
    # display/filter metadata only; aggregation identity is Trade.product_id.
    if fuel_type is not None or market_product is not None:
        if fuel_type is not None:
            base_query = base_query.where(Trade.fuel_type == fuel_type)
        if market_product is not None:
            base_query = base_query.where(Trade.market_product == market_product)
    if delivery_point_id is not None:
        base_query = base_query.where(Trade.delivery_point_id == delivery_point_id)
    if region is not None:
        base_query = base_query.where(
            or_(Trade.delivery_point_region == region, Trade.delivery_point_name == region)
        )
    if availability_window is not None:
        base_query = base_query.where(Trade.availability_window == normalize_availability_window(availability_window))

    # Count query
    count_stmt = select(func.count()).select_from(
        base_query.with_only_columns(Trade.id).order_by(None).distinct().subquery()
    )
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

    items = [_build_tape_entry(t, expose_delivery_point=delivery_point_id is not None) for t in trades]

    return TradeTapeResponse(items=items, total=total, market_hours=market_hours)

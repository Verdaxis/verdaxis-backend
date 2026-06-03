"""Public, read-only anonymized trade tape endpoint."""

from datetime import datetime, timedelta, timezone
from decimal import Decimal
from typing import Optional

from fastapi import APIRouter, Depends, Query
from sqlalchemy import select, func
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import joinedload, aliased

from app.database import get_db
from app.models.orderbook import Trade, TradeStatus, OrderBookOrder
from app.schemas.trade_tape import TradeTapeEntry, TradeTapeResponse
from app.services.availability_windows import normalize_availability_window
from app.services.demo_market import is_demo_market_organization

router = APIRouter(prefix="/trade-tape", tags=["trade-tape"])


def _is_market_hours(now: datetime) -> bool:
    """Verdaxis physical fuel markets are presented without exchange-session delay."""
    return True


def _build_tape_entry(trade: Trade) -> TradeTapeEntry:
    """Build an anonymized tape entry from a Trade with loaded relationships."""
    order = trade.ask_order or trade.bid_order

    market_product = None
    fuel_type = ""
    fuel_grade = ""
    region = ""
    availability_window = ""

    if order:
        market_product = order.market_product
        fuel_type = order.fuel_type
        fuel_grade = order.fuel_grade
        region = order.delivery_point_name or order.region
        availability_window = normalize_availability_window(str(order.availability_window)) if order.availability_window else ""

    return TradeTapeEntry(
        id=str(trade.id).replace("-", "")[:8],
        market_product=market_product,
        fuel_type=fuel_type,
        fuel_grade=fuel_grade,
        region=region,
        quantity_mt=trade.quantity_mt,
        price_per_mt_usd=trade.price_per_mt_usd,
        total_usd=trade.quantity_mt * trade.price_per_mt_usd,
        confirmed_at=trade.confirmed_at,
        availability_window=availability_window,
        is_demo_trade=(
            is_demo_market_organization(trade.buyer_id)
            and is_demo_market_organization(trade.seller_id)
        ),
    )


@router.get("", response_model=TradeTapeResponse)
async def get_trade_tape(
    db: AsyncSession = Depends(get_db),
    fuel_type: Optional[str] = Query(None, description="Filter by fuel type"),
    market_product: Optional[str] = Query(None, description="Filter by canonical market product"),
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

    # Base filter: confirmed trades after cutoff
    conditions = [
        Trade.status.in_([TradeStatus.CONFIRMED, TradeStatus.DELIVERED, TradeStatus.PAID]),
        Trade.confirmed_at >= cutoff,
    ]
    tape_order = aliased(OrderBookOrder)

    # Join a single canonical display order per trade. Prefer the ASK order when present,
    # otherwise fall back to the BID order. This avoids double-counting trades that have both.
    base_query = (
        select(Trade)
        .outerjoin(tape_order, tape_order.id == func.coalesce(Trade.ask_order_id, Trade.bid_order_id))
        .where(*conditions)
        .options(
            joinedload(Trade.ask_order),
            joinedload(Trade.bid_order),
        )
    )

    # Apply optional filters via the canonical display order.
    if fuel_type is not None or market_product is not None:
        from app.models.catalog import Product, derive_market_product
        base_query = base_query.join(Product, tape_order.product_id == Product.id)
        if fuel_type is not None:
            base_query = base_query.where(Product.fuel_type == fuel_type)
        if market_product is not None:
            product_ids = [
                product.id
                for product in (await db.execute(select(Product))).scalars().all()
                if derive_market_product(product.name, product.fuel_type, product.fuel_grade)
                and derive_market_product(product.name, product.fuel_type, product.fuel_grade).value == market_product
            ]
            if not product_ids:
                return TradeTapeResponse(items=[], total=0, market_hours=market_hours)
            base_query = base_query.where(Product.id.in_(product_ids))
    if region is not None:
        from app.models.catalog import DeliveryPoint
        from sqlalchemy import or_
        base_query = base_query.join(DeliveryPoint, tape_order.delivery_point_id == DeliveryPoint.id).where(
            or_(DeliveryPoint.region == region, DeliveryPoint.name == region)
        )
    if availability_window is not None:
        base_query = base_query.where(tape_order.availability_window == normalize_availability_window(availability_window))

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

    items = [_build_tape_entry(t) for t in trades]

    return TradeTapeResponse(items=items, total=total, market_hours=market_hours)

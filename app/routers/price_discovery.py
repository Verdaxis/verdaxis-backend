"""
Public price discovery endpoint.
Aggregates confirmed/delivered/paid trades into price summaries by product + delivery_point.
No authentication required -- this feeds the public price ticker.
"""
import csv
import io
from datetime import datetime, date, timedelta, UTC
from decimal import Decimal
from typing import Optional, Literal as _Literal
from uuid import UUID

from fastapi import APIRouter, Depends, HTTPException, Query
from fastapi.responses import StreamingResponse
from sqlalchemy import select, func, and_, or_
from sqlalchemy.ext.asyncio import AsyncSession

from fastapi import Request as _Request
from app.rate_limit import limiter
from app.database import get_db
from app.models.orderbook import Trade, TradeStatus, OrderBookOrder
from app.models.catalog import Product, DeliveryPoint, derive_market_product
from app.schemas.orderbook import (
    PriceSummary,
    PriceDiscoveryResponse,
    ReferencePriceItem,
    ReferencePriceResponse,
)
from app.services.availability_windows import normalize_availability_window

router = APIRouter(prefix="/prices", tags=["price-discovery"])


def _trade_order_join_condition():
    """Join a trade to whichever order carries market metadata."""
    return OrderBookOrder.id == func.coalesce(Trade.ask_order_id, Trade.bid_order_id)


def _market_product_filter_clause(market_product: Optional[str]):
    if not market_product:
        return None

    normalized = market_product.strip().upper()
    lower_name = func.lower(Product.name)
    lower_type = func.lower(Product.fuel_type)
    lower_grade = func.lower(Product.fuel_grade)

    clauses = {
        "BIO_METHANOL": or_(
            lower_name.in_(["bio methanol", "methanol green"]),
            and_(lower_type == "methanol", lower_grade.in_(["bio", "green"])),
        ),
        "E_METHANOL": or_(
            lower_name == "e-methanol",
            and_(lower_type == "methanol", lower_grade.in_(["e", "synthetic"])),
        ),
        "BIO_ETHANOL": or_(
            lower_name.in_(["bio ethanol", "ethanol green"]),
            and_(lower_type == "ethanol", lower_grade.in_(["bio", "green"])),
        ),
        "SYNTHETIC_ETHANOL": or_(
            lower_name == "synthetic ethanol",
            and_(lower_type == "ethanol", lower_grade == "synthetic"),
        ),
    }

    if normalized not in clauses:
        raise ValueError("market_product must be one of BIO_METHANOL, E_METHANOL, BIO_ETHANOL, SYNTHETIC_ETHANOL")

    return clauses[normalized]


def _derive_market_product_value(product_name: str | None, fuel_type: str | None, fuel_grade: str | None) -> str | None:
    derived = derive_market_product(product_name or "", fuel_type or "", fuel_grade or "")
    return derived.value if derived else None


def _normalize_window_value(value: object) -> str:
    if not isinstance(value, str) or not value.strip():
        return "SPOT"
    try:
        return normalize_availability_window(value)
    except ValueError:
        return "SPOT"


def _validate_query_filters(market_product: Optional[str], availability_window: Optional[str]) -> tuple[Optional[str], Optional[str]]:
    normalized_market_product = market_product.strip().upper() if market_product else None
    if normalized_market_product:
        _market_product_filter_clause(normalized_market_product)

    normalized_window = None
    if availability_window:
        normalized_window = normalize_availability_window(availability_window)

    return normalized_market_product, normalized_window


def _coerce_trade_date(value: object) -> date:
    if isinstance(value, datetime):
        return value.date()
    if isinstance(value, date):
        return value
    if isinstance(value, str):
        return date.fromisoformat(value[:10])
    raise ValueError("reference price query returned an invalid trade date")


async def aggregate_trade_prices(
    db: AsyncSession,
    product_id: Optional[UUID] = None,
    delivery_point_id: Optional[UUID] = None,
    fuel_type: Optional[str] = None,
    region: Optional[str] = None,
    market_product: Optional[str] = None,
    availability_window: Optional[str] = None,
    hours: int = 24,
) -> list[PriceSummary]:
    """
    Aggregate confirmed+ trades from the last `hours` hours into
    PriceSummary objects grouped by (product_id, delivery_point_id, availability_window).

    Derives product info from joined Product/DeliveryPoint tables.
    """
    cutoff = datetime.now(UTC) - timedelta(hours=hours)
    valid_statuses = [
        TradeStatus.CONFIRMED,
        TradeStatus.DELIVERED,
        TradeStatus.PAID,
    ]
    normalized_window = normalize_availability_window(availability_window) if availability_window else None
    market_product_clause = _market_product_filter_clause(market_product)

    aggregate_stmt = (
        select(
            OrderBookOrder.product_id,
            Product.name.label("product_name"),
            Product.fuel_type.label("fuel_type"),
            Product.fuel_grade.label("fuel_grade"),
            OrderBookOrder.availability_window.label("availability_window"),
            OrderBookOrder.delivery_point_id,
            DeliveryPoint.name.label("delivery_point_name"),
            DeliveryPoint.region.label("region"),
            func.max(Trade.price_per_mt_usd).label("high"),
            func.min(Trade.price_per_mt_usd).label("low"),
            func.avg(Trade.price_per_mt_usd).label("avg_price"),
            func.sum(Trade.quantity_mt).label("total_volume"),
            func.count(Trade.id).label("trade_count"),
            func.max(Trade.created_at).label("last_trade_at"),
        )
        .join(
            OrderBookOrder,
            _trade_order_join_condition(),
        )
        .join(Product, OrderBookOrder.product_id == Product.id)
        .outerjoin(DeliveryPoint, OrderBookOrder.delivery_point_id == DeliveryPoint.id)
        .where(
            Trade.status.in_(valid_statuses),
            Trade.created_at >= cutoff,
        )
    )

    if product_id:
        aggregate_stmt = aggregate_stmt.where(OrderBookOrder.product_id == product_id)
    if delivery_point_id:
        aggregate_stmt = aggregate_stmt.where(OrderBookOrder.delivery_point_id == delivery_point_id)
    if fuel_type:
        aggregate_stmt = aggregate_stmt.where(Product.fuel_type.ilike(f"%{fuel_type}%"))
    if region:
        aggregate_stmt = aggregate_stmt.where(DeliveryPoint.region.ilike(f"%{region}%"))
    if market_product_clause is not None:
        aggregate_stmt = aggregate_stmt.where(market_product_clause)
    if normalized_window:
        aggregate_stmt = aggregate_stmt.where(OrderBookOrder.availability_window == normalized_window)

    aggregate_stmt = aggregate_stmt.group_by(
        OrderBookOrder.product_id, Product.name, Product.fuel_type, Product.fuel_grade,
        OrderBookOrder.availability_window,
        OrderBookOrder.delivery_point_id, DeliveryPoint.name, DeliveryPoint.region,
    )

    result = await db.execute(aggregate_stmt)
    rows = result.all()

    # Latest price subquery
    latest_price_stmt = (
        select(
            OrderBookOrder.product_id,
            OrderBookOrder.delivery_point_id,
            OrderBookOrder.availability_window.label("availability_window"),
            Trade.price_per_mt_usd.label("last_price"),
            Trade.created_at.label("last_trade_at"),
            func.row_number().over(
                partition_by=(
                    OrderBookOrder.product_id,
                    OrderBookOrder.delivery_point_id,
                    OrderBookOrder.availability_window,
                ),
                order_by=Trade.created_at.desc(),
            ).label("rn"),
        )
        .join(
            OrderBookOrder,
            _trade_order_join_condition(),
        )
        .join(Product, OrderBookOrder.product_id == Product.id)
        .outerjoin(DeliveryPoint, OrderBookOrder.delivery_point_id == DeliveryPoint.id)
        .where(
            Trade.status.in_(valid_statuses),
            Trade.created_at >= cutoff,
        )
    )
    if product_id:
        latest_price_stmt = latest_price_stmt.where(OrderBookOrder.product_id == product_id)
    if delivery_point_id:
        latest_price_stmt = latest_price_stmt.where(OrderBookOrder.delivery_point_id == delivery_point_id)
    if fuel_type:
        latest_price_stmt = latest_price_stmt.where(Product.fuel_type.ilike(f"%{fuel_type}%"))
    if region:
        latest_price_stmt = latest_price_stmt.where(DeliveryPoint.region.ilike(f"%{region}%"))
    if market_product_clause is not None:
        latest_price_stmt = latest_price_stmt.where(market_product_clause)
    if normalized_window:
        latest_price_stmt = latest_price_stmt.where(OrderBookOrder.availability_window == normalized_window)

    latest_subquery = latest_price_stmt.subquery()
    latest_result = await db.execute(
        select(
            latest_subquery.c.product_id,
            latest_subquery.c.delivery_point_id,
            latest_subquery.c.availability_window,
            latest_subquery.c.last_price,
            latest_subquery.c.last_trade_at,
        ).where(latest_subquery.c.rn == 1)
    )
    latest_rows = latest_result.all()
    latest_by_market = {
        (
            row.product_id,
            row.delivery_point_id,
            _normalize_window_value(row.availability_window),
        ): row
        for row in latest_rows
    }

    summaries: list[PriceSummary] = []
    for row in rows:
        row_window = _normalize_window_value(row.availability_window)
        latest = latest_by_market.get((row.product_id, row.delivery_point_id, row_window))
        summaries.append(
            PriceSummary(
                product_id=row.product_id,
                product_name=row.product_name or "",
                market_product=_derive_market_product_value(row.product_name, row.fuel_type, row.fuel_grade),
                fuel_type=row.fuel_type or "",
                delivery_point_id=row.delivery_point_id,
                delivery_point_name=row.delivery_point_name,
                availability_window=row_window,
                region=row.region or "",
                last_price=latest.last_price if latest else row.high,
                avg_price_24h=Decimal(str(round(row.avg_price, 2))) if row.avg_price else None,
                high_24h=row.high,
                low_24h=row.low,
                volume_24h=row.total_volume or Decimal("0"),
                trade_count_24h=row.trade_count or 0,
                price_change_pct=None,
                last_trade_at=latest.last_trade_at if latest else row.last_trade_at,
            )
        )

    return summaries


@router.get("", response_model=PriceDiscoveryResponse)
@limiter.limit("60/minute")
async def get_price_summaries(
    request: _Request,
    product_id: Optional[UUID] = Query(None, description="Filter by product ID"),
    delivery_point_id: Optional[UUID] = Query(None, description="Filter by delivery point ID"),
    fuel_type: Optional[str] = Query(None, description="Filter by fuel type"),
    region: Optional[str] = Query(None, description="Filter by region"),
    market_product: Optional[str] = Query(None, description="Filter by canonical market product"),
    availability_window: Optional[str] = Query(None, description="Filter by availability window"),
    hours: int = Query(24, ge=1, le=168, description="Lookback window in hours"),
    db: AsyncSession = Depends(get_db),
):
    """
    Public endpoint: aggregated trade prices by product + delivery_point.
    No auth required. Feeds the public-site price ticker.
    """
    try:
        validated_market_product, validated_window = _validate_query_filters(market_product, availability_window)
    except ValueError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc

    summaries = await aggregate_trade_prices(
        db,
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        fuel_type=fuel_type,
        region=region,
        market_product=validated_market_product,
        availability_window=validated_window,
        hours=hours,
    )
    return PriceDiscoveryResponse(
        summaries=summaries,
        generated_at=datetime.now(UTC),
    )


async def compute_reference_prices(
    db: AsyncSession,
    date_from: Optional[date] = None,
    date_to: Optional[date] = None,
    product_id: Optional[UUID] = None,
    delivery_point_id: Optional[UUID] = None,
    fuel_type: Optional[str] = None,
    region: Optional[str] = None,
    market_product: Optional[str] = None,
    availability_window: Optional[str] = None,
) -> list[ReferencePriceItem]:
    """
    Compute daily VWAP reference prices from confirmed+ trades.
    VWAP = sum(price * quantity) / sum(quantity), grouped by product_id, delivery_point_id, date.
    """
    valid_statuses = [
        TradeStatus.CONFIRMED,
        TradeStatus.DELIVERED,
        TradeStatus.PAID,
    ]
    normalized_window = normalize_availability_window(availability_window) if availability_window else None
    market_product_clause = _market_product_filter_clause(market_product)

    trade_date_expr = func.date(Trade.created_at)
    trade_date = trade_date_expr.label("trade_date")

    stmt = (
        select(
            OrderBookOrder.product_id,
            Product.name.label("product_name"),
            Product.fuel_type.label("fuel_type"),
            Product.fuel_grade.label("fuel_grade"),
            OrderBookOrder.availability_window.label("availability_window"),
            OrderBookOrder.delivery_point_id,
            DeliveryPoint.name.label("delivery_point_name"),
            DeliveryPoint.region.label("region"),
            trade_date,
            func.sum(Trade.price_per_mt_usd * Trade.quantity_mt).label("weighted_sum"),
            func.sum(Trade.quantity_mt).label("total_volume"),
            func.count(Trade.id).label("trade_count"),
        )
        .join(
            OrderBookOrder,
            _trade_order_join_condition(),
        )
        .join(Product, OrderBookOrder.product_id == Product.id)
        .outerjoin(DeliveryPoint, OrderBookOrder.delivery_point_id == DeliveryPoint.id)
        .where(Trade.status.in_(valid_statuses))
    )

    if date_from:
        stmt = stmt.where(trade_date_expr >= date_from.isoformat())
    if date_to:
        stmt = stmt.where(trade_date_expr <= date_to.isoformat())
    if product_id:
        stmt = stmt.where(OrderBookOrder.product_id == product_id)
    if delivery_point_id:
        stmt = stmt.where(OrderBookOrder.delivery_point_id == delivery_point_id)
    if fuel_type:
        stmt = stmt.where(Product.fuel_type.ilike(f"%{fuel_type}%"))
    if region:
        stmt = stmt.where(DeliveryPoint.region.ilike(f"%{region}%"))
    if market_product_clause is not None:
        stmt = stmt.where(market_product_clause)
    if normalized_window:
        stmt = stmt.where(OrderBookOrder.availability_window == normalized_window)

    stmt = stmt.group_by(
        OrderBookOrder.product_id, Product.name, Product.fuel_type, Product.fuel_grade,
        OrderBookOrder.availability_window,
        OrderBookOrder.delivery_point_id, DeliveryPoint.name, DeliveryPoint.region,
        trade_date,
    ).order_by(trade_date.desc())

    result = await db.execute(stmt)
    rows = result.all()

    items: list[ReferencePriceItem] = []
    for row in rows:
        total_vol = row.total_volume or Decimal("0")
        weighted = row.weighted_sum or Decimal("0")
        vwap = Decimal(str(round(weighted / total_vol, 2))) if total_vol > 0 else Decimal("0")
        fuel_grade = getattr(row, "fuel_grade", "")
        if not isinstance(fuel_grade, str):
            fuel_grade = ""
        availability_window = getattr(row, "availability_window", "SPOT")
        if not isinstance(availability_window, str):
            availability_window = "SPOT"
        items.append(
            ReferencePriceItem(
                product_id=row.product_id,
                product_name=row.product_name or "",
                market_product=(
                    derived.value
                    if (derived := derive_market_product(row.product_name or "", row.fuel_type or "", fuel_grade))
                    else None
                ),
                fuel_type=row.fuel_type or "",
                delivery_point_id=row.delivery_point_id,
                delivery_point_name=row.delivery_point_name,
                availability_window=availability_window,
                region=row.region or "",
                vwap_usd=vwap,
                total_volume_mt=total_vol,
                trade_count=row.trade_count or 0,
                date=_coerce_trade_date(row.trade_date),
            )
        )

    return items


_CSV_COLUMNS = [
    "date", "product_name", "market_product", "availability_window",
    "fuel_type", "delivery_point_name", "region", "vwap_usd", "volume_mt", "trade_count",
]


def _items_to_csv(items: list[ReferencePriceItem]) -> str:
    """Serialise a list of ReferencePriceItem objects into a CSV string."""
    buf = io.StringIO()
    writer = csv.DictWriter(buf, fieldnames=_CSV_COLUMNS)
    writer.writeheader()
    for item in items:
        writer.writerow({
            "date": str(item.date),
            "product_name": item.product_name or "",
            "market_product": item.market_product or "",
            "availability_window": item.availability_window or "SPOT",
            "fuel_type": item.fuel_type or "",
            "delivery_point_name": item.delivery_point_name or "",
            "region": item.region or "",
            "vwap_usd": str(item.vwap_usd),
            "volume_mt": str(item.total_volume_mt),
            "trade_count": str(item.trade_count),
        })
    return buf.getvalue()


@router.get("/reference", response_model=ReferencePriceResponse)
@limiter.limit("30/minute")
async def get_reference_prices(
    request: _Request,
    product_id: Optional[UUID] = Query(None, description="Filter by product ID"),
    delivery_point_id: Optional[UUID] = Query(None, description="Filter by delivery point ID"),
    fuel_type: Optional[str] = Query(None, description="Filter by fuel type"),
    region: Optional[str] = Query(None, description="Filter by region"),
    market_product: Optional[str] = Query(None, description="Filter by canonical market product"),
    availability_window: Optional[str] = Query(None, description="Filter by availability window"),
    date_from: Optional[date] = Query(None, alias="from", description="Start date (inclusive), e.g. 2026-01-01"),
    date_to: Optional[date] = Query(None, alias="to", description="End date (inclusive), e.g. 2026-03-01"),
    visibility: _Literal["internal", "external"] = Query("external", description="VWAP tier: internal (platform) or external (public benchmark)"),
    db: AsyncSession = Depends(get_db),
):
    """
    Public endpoint: daily VWAP reference prices by product + delivery_point.
    No auth required. Calculates Volume-Weighted Average Price from confirmed+ trades.
    Supports date range filtering, product/delivery_point/fuel_type/region filters, and visibility tier.
    """
    try:
        validated_market_product, validated_window = _validate_query_filters(market_product, availability_window)
    except ValueError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc

    prices = await compute_reference_prices(
        db,
        date_from=date_from,
        date_to=date_to,
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        fuel_type=fuel_type,
        region=region,
        market_product=validated_market_product,
        availability_window=validated_window,
    )
    for item in prices:
        item.visibility = visibility
    return ReferencePriceResponse(
        prices=prices,
        generated_at=datetime.now(UTC),
    )


@router.get("/reference/export")
@limiter.limit("10/minute")
async def export_reference_prices_csv(
    request: _Request,
    product_id: Optional[UUID] = Query(None, description="Filter by product ID"),
    delivery_point_id: Optional[UUID] = Query(None, description="Filter by delivery point ID"),
    fuel_type: Optional[str] = Query(None, description="Filter by fuel type"),
    region: Optional[str] = Query(None, description="Filter by region"),
    market_product: Optional[str] = Query(None, description="Filter by canonical market product"),
    availability_window: Optional[str] = Query(None, description="Filter by availability window"),
    from_date: Optional[date] = Query(None, description="Start date (inclusive), e.g. 2026-01-01"),
    to_date: Optional[date] = Query(None, description="End date (inclusive), e.g. 2026-03-01"),
    format: str = Query("csv", description="Export format (currently only csv is supported)"),
    db: AsyncSession = Depends(get_db),
):
    """
    Export daily VWAP reference prices as a CSV download.
    No auth required. Accepts the same filters as /reference.
    CSV columns: date, product_name, market_product, availability_window,
                 fuel_type, delivery_point_name, region, vwap_usd, volume_mt, trade_count
    """
    try:
        validated_market_product, validated_window = _validate_query_filters(market_product, availability_window)
    except ValueError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc

    prices = await compute_reference_prices(
        db,
        date_from=from_date,
        date_to=to_date,
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        fuel_type=fuel_type,
        region=region,
        market_product=validated_market_product,
        availability_window=validated_window,
    )
    csv_content = _items_to_csv(prices)
    filename = "verdaxis_reference_prices.csv"
    return StreamingResponse(
        iter([csv_content]),
        media_type="text/csv",
        headers={"Content-Disposition": f'attachment; filename="{filename}"'},
    )

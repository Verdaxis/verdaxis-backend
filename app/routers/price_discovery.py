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
from app.models.orderbook import Trade, TradeStatus
from app.market_catalog import MARKET_PRODUCT_CODES
from app.models.user import OrganizationProvenance
from app.schemas.orderbook import (
    PriceSummary,
    PriceDiscoveryResponse,
    ReferencePriceItem,
    ReferencePriceResponse,
)
from app.schemas.market_activity import (
    MarketDemoStatus,
    MarketScope,
    MarketSourceKind,
)
from app.services.availability_windows import normalize_availability_window
from app.services.market_provenance import (
    MarketEvidenceScope,
    formal_trade_snapshot_clause,
    select_aggregate_evidence,
    trade_evidence_clause,
)

router = APIRouter(prefix="/prices", tags=["price-discovery"])


def _market_product_filter_clause(market_product: Optional[str]):
    if not market_product:
        return None

    normalized = market_product.strip().upper()
    if normalized not in MARKET_PRODUCT_CODES:
        raise ValueError(f"market_product must be one of {', '.join(MARKET_PRODUCT_CODES)}")
    return Trade.market_product == normalized


def _trade_market_product_filter_clause(market_product: Optional[str]):
    """Filter immutable trade snapshots, never mutable order/product rows."""
    if not market_product:
        return None
    normalized = market_product.strip().upper()
    if normalized not in MARKET_PRODUCT_CODES:
        raise ValueError(f"market_product must be one of {', '.join(MARKET_PRODUCT_CODES)}")
    return Trade.market_product == normalized


def _normalize_window_value(value: object) -> str:
    if not isinstance(value, str):
        raise ValueError("formal trade evidence has no canonical availability window")
    return normalize_availability_window(value)


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


def _resolve_date_alias(
    preferred: Optional[date],
    deprecated: Optional[date],
    *,
    preferred_name: str,
    deprecated_name: str,
) -> Optional[date]:
    if preferred is not None and deprecated is not None and preferred != deprecated:
        raise ValueError(f"{preferred_name} and {deprecated_name} must match when both are supplied")
    return preferred if preferred is not None else deprecated


def _validate_reference_date_range(
    date_from: Optional[date],
    date_to: Optional[date],
    *,
    from_name: str = "date_from",
    to_name: str = "date_to",
) -> None:
    if date_from is not None and date_to is not None and date_from > date_to:
        raise ValueError(f"{from_name} must be before or equal to {to_name}")


def _resolve_reference_date_range(
    date_from: Optional[date],
    date_to: Optional[date],
    from_alias: Optional[date],
    to_alias: Optional[date],
) -> tuple[Optional[date], Optional[date]]:
    resolved_from = _resolve_date_alias(
        date_from,
        from_alias,
        preferred_name="date_from",
        deprecated_name="from",
    )
    resolved_to = _resolve_date_alias(
        date_to,
        to_alias,
        preferred_name="date_to",
        deprecated_name="to",
    )
    _validate_reference_date_range(resolved_from, resolved_to)
    return resolved_from, resolved_to


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
    """Aggregate immutable trade snapshots inside separate REAL/DEMO scopes."""
    cutoff = datetime.now(UTC) - timedelta(hours=hours)
    normalized_window = normalize_availability_window(availability_window) if availability_window else None
    market_product_clause = _trade_market_product_filter_clause(market_product)
    summaries: list[PriceSummary] = []
    event_time = Trade.confirmed_at

    def filtered(filters):
        scoped = list(filters)
        if product_id:
            scoped.append(Trade.product_id == product_id)
        if delivery_point_id:
            scoped.append(Trade.delivery_point_id == delivery_point_id)
        if fuel_type:
            scoped.append(Trade.fuel_type.ilike(f"%{fuel_type}%"))
        if region:
            scoped.append(Trade.delivery_point_region.ilike(f"%{region}%"))
        if market_product_clause is not None:
            scoped.append(market_product_clause)
        if normalized_window:
            scoped.append(Trade.availability_window == normalized_window)
        return scoped

    for evidence_scope in (MarketEvidenceScope.REAL, MarketEvidenceScope.DEMO):
        evidence_clause = trade_evidence_clause(
            Trade,
            evidence_scope,
            confirmed_since=cutoff,
        )
        common_filters = filtered([
            evidence_clause,
        ])

        identity_columns = (
            Trade.product_id,
            Trade.product_name,
            Trade.fuel_type,
            Trade.fuel_grade,
            Trade.market_product,
            Trade.availability_window,
            Trade.delivery_point_id,
            Trade.delivery_point_name,
            Trade.delivery_point_region,
        )
        aggregate_stmt = (
            select(
                *identity_columns,
                func.max(Trade.price_per_mt_usd).label("high"),
                func.min(Trade.price_per_mt_usd).label("low"),
                func.avg(Trade.price_per_mt_usd).label("avg_price"),
                func.sum(Trade.quantity_mt).label("total_volume"),
                func.count(Trade.id).label("trade_count"),
                func.max(event_time).label("last_trade_at"),
            )
            .where(*common_filters)
            .group_by(*identity_columns)
        )
        rows = (await db.execute(aggregate_stmt)).all()

        latest_price_stmt = select(
            Trade.product_id,
            Trade.market_product,
            Trade.delivery_point_id,
            Trade.availability_window,
            Trade.price_per_mt_usd.label("last_price"),
            event_time.label("last_trade_at"),
            func.row_number().over(
                partition_by=(
                    Trade.product_id,
                    Trade.market_product,
                    Trade.delivery_point_id,
                    Trade.availability_window,
                ),
                order_by=event_time.desc(),
            ).label("rn"),
        ).where(*common_filters)
        latest_subquery = latest_price_stmt.subquery()
        latest_rows = (
            await db.execute(
                select(latest_subquery).where(latest_subquery.c.rn == 1)
            )
        ).all()
        latest_by_market = {
            (
                row.product_id,
                row.market_product,
                row.delivery_point_id,
                _normalize_window_value(row.availability_window),
            ): row
            for row in latest_rows
        }

        for row in rows:
            row_window = _normalize_window_value(row.availability_window)
            latest = latest_by_market.get(
                (row.product_id, row.market_product, row.delivery_point_id, row_window)
            )
            observed_at = latest.last_trade_at if latest else row.last_trade_at
            trade_count = int(row.trade_count or 0)
            selection = select_aggregate_evidence(
                real_count=(trade_count if evidence_scope == MarketEvidenceScope.REAL else 0),
                demo_count=(trade_count if evidence_scope == MarketEvidenceScope.DEMO else 0),
                unknown_count=0,
                real_source=MarketSourceKind.CONFIRMED_TRADE,
            )
            summaries.append(
                PriceSummary(
                    product_id=row.product_id,
                    product_name=row.product_name or "",
                    market_product=row.market_product,
                    fuel_type=row.fuel_type or "",
                    delivery_point_id=row.delivery_point_id,
                    delivery_point_name=row.delivery_point_name,
                    availability_window=row_window,
                    region=row.delivery_point_region or "",
                    last_price=latest.last_price if latest else row.high,
                    avg_price_24h=(
                        Decimal(str(round(row.avg_price, 2)))
                        if row.avg_price is not None
                        else None
                    ),
                    high_24h=row.high,
                    low_24h=row.low,
                    volume_24h=row.total_volume or Decimal("0"),
                    trade_count_24h=trade_count,
                    price_change_pct=None,
                    last_trade_at=observed_at,
                    source_kind=selection.source_kind,
                    scope=(
                        MarketScope.DELIVERY_POINT
                        if row.delivery_point_id
                        else MarketScope.UNKNOWN
                    ),
                    demo_status=selection.demo_status,
                    is_reference=False,
                    observed_at=observed_at,
                    real_trade_count_24h=(
                        trade_count if evidence_scope == MarketEvidenceScope.REAL else 0
                    ),
                    demo_trade_count_24h=(
                        trade_count if evidence_scope == MarketEvidenceScope.DEMO else 0
                    ),
                    unknown_trade_count_24h=0,
                )
            )

    # Legacy mismatched or UNKNOWN snapshots are reported as quarantine
    # metadata only. Their prices and volumes never enter public economics.
    buyer_class = func.coalesce(Trade.buyer_provenance, OrganizationProvenance.UNKNOWN.value)
    seller_class = func.coalesce(Trade.seller_provenance, OrganizationProvenance.UNKNOWN.value)
    forbidden = (OrganizationProvenance.TEST.value, OrganizationProvenance.CANARY.value)
    quarantine_clause = and_(
        ~buyer_class.in_(forbidden),
        ~seller_class.in_(forbidden),
        or_(
            buyer_class == OrganizationProvenance.UNKNOWN.value,
            seller_class == OrganizationProvenance.UNKNOWN.value,
            buyer_class != seller_class,
        ),
    )
    quarantine_filters = filtered(
        [
            formal_trade_snapshot_clause(Trade),
            Trade.status.in_([TradeStatus.CONFIRMED, TradeStatus.DELIVERED, TradeStatus.PAID]),
            Trade.confirmed_at.is_not(None),
            event_time >= cutoff,
            quarantine_clause,
        ]
    )
    identity_columns = (
        Trade.product_id,
        Trade.product_name,
        Trade.fuel_type,
        Trade.fuel_grade,
        Trade.market_product,
        Trade.availability_window,
        Trade.delivery_point_id,
        Trade.delivery_point_name,
        Trade.delivery_point_region,
    )
    quarantine_rows = (
        await db.execute(
            select(
                *identity_columns,
                func.count(Trade.id).label("unknown_count"),
                func.max(event_time).label("observed_at"),
            )
            .where(*quarantine_filters)
            .group_by(*identity_columns)
        )
    ).all()
    for row in quarantine_rows:
        unknown_count = int(row.unknown_count or 0)
        selection = select_aggregate_evidence(
            real_count=0,
            demo_count=0,
            unknown_count=unknown_count,
            real_source=MarketSourceKind.CONFIRMED_TRADE,
        )
        summaries.append(
            PriceSummary(
                product_id=row.product_id,
                product_name=row.product_name or "",
                market_product=row.market_product,
                fuel_type=row.fuel_type or "",
                delivery_point_id=row.delivery_point_id,
                delivery_point_name=row.delivery_point_name,
                availability_window=_normalize_window_value(row.availability_window),
                region=row.delivery_point_region or "",
                last_price=None,
                avg_price_24h=None,
                high_24h=None,
                low_24h=None,
                volume_24h=Decimal("0"),
                trade_count_24h=0,
                last_trade_at=None,
                source_kind=selection.source_kind,
                scope=(
                    MarketScope.DELIVERY_POINT
                    if row.delivery_point_id
                    else MarketScope.UNKNOWN
                ),
                demo_status=selection.demo_status,
                is_reference=False,
                observed_at=row.observed_at,
                unknown_trade_count_24h=unknown_count,
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
    _validate_reference_date_range(date_from, date_to)

    normalized_window = normalize_availability_window(availability_window) if availability_window else None
    market_product_clause = _trade_market_product_filter_clause(market_product)

    event_time = Trade.confirmed_at
    trade_date_expr = func.date(event_time)
    trade_date = trade_date_expr.label("trade_date")

    stmt = (
        select(
            Trade.product_id,
            Trade.product_name,
            Trade.fuel_type,
            Trade.fuel_grade,
            Trade.market_product,
            Trade.availability_window,
            Trade.delivery_point_id,
            Trade.delivery_point_name,
            Trade.delivery_point_region.label("region"),
            trade_date,
            func.sum(Trade.price_per_mt_usd * Trade.quantity_mt).label("weighted_sum"),
            func.sum(Trade.quantity_mt).label("total_volume"),
            func.count(Trade.id).label("trade_count"),
        )
        .where(
            trade_evidence_clause(
                Trade,
                MarketEvidenceScope.REAL,
            ),
        )
    )

    if date_from:
        stmt = stmt.where(trade_date_expr >= date_from)
    if date_to:
        stmt = stmt.where(trade_date_expr <= date_to)
    if product_id:
        stmt = stmt.where(Trade.product_id == product_id)
    if delivery_point_id:
        stmt = stmt.where(Trade.delivery_point_id == delivery_point_id)
    if fuel_type:
        stmt = stmt.where(Trade.fuel_type.ilike(f"%{fuel_type}%"))
    if region:
        stmt = stmt.where(Trade.delivery_point_region.ilike(f"%{region}%"))
    if market_product_clause is not None:
        stmt = stmt.where(market_product_clause)
    if normalized_window:
        stmt = stmt.where(Trade.availability_window == normalized_window)

    stmt = stmt.group_by(
        Trade.product_id, Trade.product_name, Trade.fuel_type, Trade.fuel_grade, Trade.market_product,
        Trade.availability_window,
        Trade.delivery_point_id, Trade.delivery_point_name, Trade.delivery_point_region,
        trade_date,
    ).order_by(trade_date.desc())

    result = await db.execute(stmt)
    rows = result.all()

    items: list[ReferencePriceItem] = []
    for row in rows:
        total_vol = row.total_volume or Decimal("0")
        weighted = row.weighted_sum or Decimal("0")
        vwap = Decimal(str(round(weighted / total_vol, 2))) if total_vol > 0 else Decimal("0")
        availability_window = _normalize_window_value(row.availability_window)
        items.append(
            ReferencePriceItem(
                product_id=row.product_id,
                product_name=row.product_name or "",
                market_product=row.market_product,
                fuel_type=row.fuel_type or "",
                delivery_point_id=row.delivery_point_id,
                delivery_point_name=row.delivery_point_name,
                availability_window=availability_window,
                region=row.region or "",
                vwap_usd=vwap,
                total_volume_mt=total_vol,
                trade_count=row.trade_count or 0,
                date=_coerce_trade_date(row.trade_date),
                source_kind=MarketSourceKind.CONFIRMED_TRADE,
                scope=MarketScope.DELIVERY_POINT,
                demo_status=MarketDemoStatus.REAL_ONLY,
                is_reference=True,
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
            "availability_window": item.availability_window,
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
    date_from: Optional[date] = Query(None, description="Start date (inclusive), e.g. 2026-01-01"),
    date_to: Optional[date] = Query(None, description="End date (inclusive), e.g. 2026-03-01"),
    from_alias: Optional[date] = Query(None, alias="from", deprecated=True, description="Deprecated alias for date_from"),
    to_alias: Optional[date] = Query(None, alias="to", deprecated=True, description="Deprecated alias for date_to"),
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
        validated_date_from, validated_date_to = _resolve_reference_date_range(
            date_from,
            date_to,
            from_alias,
            to_alias,
        )
    except ValueError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc

    prices = await compute_reference_prices(
        db,
        date_from=validated_date_from,
        date_to=validated_date_to,
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
    No auth required. Accepts the same market filters as /reference and export-specific from_date/to_date date filters.
    CSV columns: date, product_name, market_product, availability_window,
                 fuel_type, delivery_point_name, region, vwap_usd, volume_mt, trade_count
    """
    try:
        validated_market_product, validated_window = _validate_query_filters(market_product, availability_window)
        _validate_reference_date_range(from_date, to_date, from_name="from_date", to_name="to_date")
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

"""
Public demand signals endpoint for suppliers.
Aggregates open BID orders into anonymized demand signals by fuel_type + region.
"""
from typing import Annotated, Optional, List
from datetime import date

from fastapi import APIRouter, Depends, Query
from sqlalchemy import func, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.models.orderbook import OrderBookOrder, OrderSide, OrderBookStatus
from app.models.catalog import Product, DeliveryPoint
from app.models.user import OrganizationProvenance
from app.schemas.demand import DemandSignal, UrgencyLevel
from app.services.availability_windows import (
    SPOT_WINDOW,
    availability_window_display_label,
    normalize_availability_window,
    window_start_date,
)
from app.schemas.market_activity import MarketScope, MarketSourceKind
from app.services.market_data_eligibility import (
    active_market_catalog_clauses,
    canonical_market_product_expression,
    current_public_order_clause,
)
from app.services.market_provenance import select_aggregate_evidence

router = APIRouter(prefix="/demand", tags=["demand"])
MAX_DEMAND_GROUPS = 512


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
    limit: Annotated[int, Query(ge=1, le=MAX_DEMAND_GROUPS)] = 256,
    db: AsyncSession = Depends(get_db),
):
    """
    Public: anonymized demand signals from buyer bids.
    Aggregated by tradable market product + delivery point + availability window.
    """
    canonical_product = canonical_market_product_expression(Product)
    identity_columns = (
        canonical_product,
        DeliveryPoint.id,
        DeliveryPoint.name,
        DeliveryPoint.region,
        OrderBookOrder.availability_window,
        OrderBookOrder.provenance,
    )
    stmt = (
        select(
            canonical_product.label("market_product"),
            DeliveryPoint.id.label("delivery_point_id"),
            DeliveryPoint.name.label("delivery_point_name"),
            DeliveryPoint.region.label("region"),
            OrderBookOrder.availability_window.label("availability_window"),
            OrderBookOrder.provenance.label("provenance"),
            func.sum(OrderBookOrder.remaining_quantity_mt).label("volume_mt"),
            func.max(OrderBookOrder.price_per_mt_usd).label("max_price_per_mt"),
            func.count(OrderBookOrder.id).label("order_count"),
            func.max(OrderBookOrder.created_at).label("created_at"),
        )
        .join(Product, OrderBookOrder.product_id == Product.id)
        .join(DeliveryPoint, OrderBookOrder.delivery_point_id == DeliveryPoint.id)
        .where(
            *active_market_catalog_clauses(Product, DeliveryPoint),
            OrderBookOrder.side == OrderSide.BID,
            OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
            current_public_order_clause(OrderBookOrder),
            OrderBookOrder.provenance.in_(
                (
                    OrganizationProvenance.REAL.value,
                    OrganizationProvenance.DEMO.value,
                    OrganizationProvenance.UNKNOWN.value,
                )
            ),
        )
        .group_by(*identity_columns)
        .order_by(func.max(OrderBookOrder.created_at).desc(), canonical_product)
        .limit(limit)
    )

    if fuel_type:
        stmt = stmt.where(Product.fuel_type.ilike(f"%{fuel_type}%"))
    if region:
        stmt = stmt.where(DeliveryPoint.region.ilike(f"%{region}%"))

    result = await db.execute(stmt)
    signals = []
    for row in result.all():
        normalized_window = normalize_availability_window(str(row.availability_window))
        provenance = (
            row.provenance
            if isinstance(row.provenance, OrganizationProvenance)
            else OrganizationProvenance(str(row.provenance))
        )
        count = int(row.order_count or 0)
        selection = select_aggregate_evidence(
            real_count=count if provenance == OrganizationProvenance.REAL else 0,
            demo_count=count if provenance == OrganizationProvenance.DEMO else 0,
            unknown_count=count if provenance == OrganizationProvenance.UNKNOWN else 0,
            real_source=MarketSourceKind.LIVE_ORDER,
        )
        signals.append(DemandSignal(
            fuel_type=row.market_product,
            region=row.region or "",
            market_product_code=row.market_product,
            delivery_point_id=row.delivery_point_id,
            delivery_point_name=row.delivery_point_name,
            availability_window_code=normalized_window,
            volume_mt=None if selection.scope is None else row.volume_mt,
            max_price_per_mt=None if selection.scope is None else row.max_price_per_mt,
            urgency=_classify_urgency(normalized_window),
            bid_count=0 if selection.scope is None else count,
            earliest_delivery=availability_window_display_label(normalized_window),
            created_at=row.created_at,
            source_kind=selection.source_kind,
            scope=MarketScope.DELIVERY_POINT if row.delivery_point_id else MarketScope.UNKNOWN,
            demo_status=selection.demo_status,
            unknown_count=selection.unknown_count,
        ))

    # Sort by urgency (HIGH first), then volume descending
    urgency_order = {UrgencyLevel.HIGH: 0, UrgencyLevel.MEDIUM: 1, UrgencyLevel.LOW: 2}
    signals.sort(
        key=lambda signal: (
            urgency_order.get(signal.urgency, 2),
            signal.source_kind == MarketSourceKind.UNKNOWN,
            -(signal.volume_mt or 0),
        )
    )

    return signals

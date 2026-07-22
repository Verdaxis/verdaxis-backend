"""
Public fuel availability endpoint.
Aggregates supplier inventory data per port to show availability levels.
"""
from typing import Optional, List
from fastapi import APIRouter, Depends, Query
from sqlalchemy import and_, case, func, or_, select
from sqlalchemy.ext.asyncio import AsyncSession
from geoalchemy2 import Geometry, functions as geo_func
from decimal import Decimal

from app.database import get_db
from app.models.marketplace import InventoryItem
from app.models.port import Port
from app.models.catalog import DeliveryPoint, Product
from app.models.user import Organization, OrganizationProvenance, User
from app.schemas.availability import PortFuelAvailability, AvailabilityLevel
from app.schemas.market_activity import MarketScope, MarketSourceKind
from app.services.market_data_eligibility import (
    active_market_catalog_clauses,
    canonical_market_product_expression,
)
from app.services.market_provenance import (
    MarketEvidenceScope,
    organization_evidence_clause,
    select_aggregate_evidence,
)

router = APIRouter(prefix="/availability", tags=["availability"])


def _classify_availability(total_stock: Decimal) -> AvailabilityLevel:
    if total_stock > 1000:
        return AvailabilityLevel.AVAILABLE
    elif total_stock > 0:
        return AvailabilityLevel.LIMITED
    return AvailabilityLevel.NONE


@router.get("", response_model=List[PortFuelAvailability])
async def get_fuel_availability(
    fuel_type: Optional[str] = Query(None, description="Filter by fuel type (e.g. Methanol)"),
    db: AsyncSession = Depends(get_db),
):
    """
    Public: aggregated fuel availability by port.
    Returns green/yellow/red availability levels for each port.
    """
    canonical_product = canonical_market_product_expression(Product)
    real_evidence = organization_evidence_clause(
        Organization,
        User,
        MarketEvidenceScope.REAL,
    )
    demo_evidence = organization_evidence_clause(
        Organization,
        User,
        MarketEvidenceScope.DEMO,
    )
    unknown_evidence = and_(
        Organization.provenance.not_in(
            (
                OrganizationProvenance.TEST.value,
                OrganizationProvenance.CANARY.value,
            )
        ),
        func.coalesce(or_(real_evidence, demo_evidence), False).is_(False),
    )
    stmt = (
        select(
            InventoryItem.port_id,
            Port.name.label("port_name"),
            Product.name.label("product_name"),
            Product.fuel_type.label("product_fuel_type"),
            Product.fuel_grade.label("fuel_grade"),
            canonical_product.label("market_product"),
            func.sum(
                case((real_evidence, InventoryItem.current_stock_mt), else_=0)
            ).label("real_stock"),
            func.sum(
                case((demo_evidence, InventoryItem.current_stock_mt), else_=0)
            ).label("demo_stock"),
            func.count(
                func.distinct(case((real_evidence, InventoryItem.supplier_id)))
            ).label("real_supplier_count"),
            func.count(
                func.distinct(case((demo_evidence, InventoryItem.supplier_id)))
            ).label("demo_supplier_count"),
            func.count(
                func.distinct(case((unknown_evidence, InventoryItem.supplier_id)))
            ).label("unknown_count"),
            func.avg(case((real_evidence, InventoryItem.price_per_mt_usd))).label(
                "real_avg_price"
            ),
            func.avg(case((demo_evidence, InventoryItem.price_per_mt_usd))).label(
                "demo_avg_price"
            ),
            geo_func.ST_X(Port.location.cast(Geometry)).label("lng"),
            geo_func.ST_Y(Port.location.cast(Geometry)).label("lat"),
        )
        .join(Port, InventoryItem.port_id == Port.id)
        .join(DeliveryPoint, DeliveryPoint.name == Port.name)
        .join(Product, Product.name == InventoryItem.product_name)
        .join(Organization, Organization.id == InventoryItem.supplier_id)
        .outerjoin(
            User,
            and_(
                User.id == InventoryItem.owner_user_id,
                User.organization_id == Organization.id,
            ),
        )
        .where(
            Port.is_active.is_(True),
            *active_market_catalog_clauses(Product, DeliveryPoint),
            Organization.provenance.not_in(
                (
                    OrganizationProvenance.TEST.value,
                    OrganizationProvenance.CANARY.value,
                )
            ),
        )
        .group_by(
            InventoryItem.port_id,
            Port.name,
            Product.id,
            Product.name,
            Product.fuel_type,
            Product.fuel_grade,
            Port.location,
        )
    )

    if fuel_type:
        pattern = f"%{fuel_type}%"
        stmt = stmt.where(
            or_(
                Product.name.ilike(pattern),
                Product.fuel_type.ilike(pattern),
                Product.fuel_grade.ilike(pattern),
            )
        )

    result = await db.execute(stmt)
    rows = result.all()

    availability_list = []
    for row in rows:
        market_product_code = row.market_product
        # The SQL predicate is authoritative; retain a defensive boundary in
        # case a legacy view or mocked adapter returns an unmapped product.
        if market_product_code is None:
            continue
        real_count = int(row.real_supplier_count or 0)
        demo_count = int(row.demo_supplier_count or 0)
        unknown_count = int(row.unknown_count or 0)
        selection = select_aggregate_evidence(
            real_count=real_count,
            demo_count=demo_count,
            unknown_count=unknown_count,
            real_source=MarketSourceKind.LIVE_INVENTORY,
        )
        prefix = selection.value_prefix
        total = (
            Decimal(str(getattr(row, f"{prefix}_stock") or 0))
            if prefix is not None
            else None
        )
        supplier_count = (
            int(getattr(row, f"{prefix}_supplier_count") or 0)
            if prefix is not None
            else 0
        )
        average = (
            getattr(row, f"{prefix}_avg_price") if prefix is not None else None
        )
        availability_list.append(
            PortFuelAvailability(
                port_id=row.port_id,
                port_name=row.port_name,
                lat=row.lat,
                lng=row.lng,
                fuel_type=market_product_code,
                market_product_code=market_product_code,
                total_stock_mt=total,
                supplier_count=supplier_count,
                availability_level=(
                    _classify_availability(total)
                    if total is not None
                    else AvailabilityLevel.NONE
                ),
                avg_price_per_mt=(
                    Decimal(str(round(average, 2))) if average is not None else None
                ),
                source_kind=selection.source_kind,
                scope=MarketScope.DELIVERY_POINT,
                demo_status=selection.demo_status,
                unknown_count=unknown_count,
            )
        )

    return availability_list

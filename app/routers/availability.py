"""
Public fuel availability endpoint.
Aggregates supplier inventory data per port to show availability levels.
"""
from typing import Optional, List
from fastapi import APIRouter, Depends, Query
from sqlalchemy import select, func
from sqlalchemy.ext.asyncio import AsyncSession
from geoalchemy2 import Geometry, functions as geo_func
from decimal import Decimal

from app.database import get_db
from app.models.marketplace import InventoryItem
from app.models.port import Port
from app.schemas.availability import PortFuelAvailability, AvailabilityLevel

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
    stmt = (
        select(
            InventoryItem.port_id,
            Port.name.label("port_name"),
            InventoryItem.fuel_type,
            func.sum(InventoryItem.current_stock_mt).label("total_stock"),
            func.count(func.distinct(InventoryItem.supplier_id)).label("supplier_count"),
            func.avg(InventoryItem.price_per_mt_usd).label("avg_price"),
            geo_func.ST_X(Port.location.cast(Geometry)).label("lng"),
            geo_func.ST_Y(Port.location.cast(Geometry)).label("lat"),
        )
        .join(Port, InventoryItem.port_id == Port.id)
        .group_by(InventoryItem.port_id, Port.name, InventoryItem.fuel_type, Port.location)
    )

    if fuel_type:
        stmt = stmt.where(InventoryItem.fuel_type.ilike(f"%{fuel_type}%"))

    result = await db.execute(stmt)
    rows = result.all()

    availability_list = []
    for row in rows:
        total = Decimal(str(row.total_stock or 0))
        availability_list.append(
            PortFuelAvailability(
                port_id=row.port_id,
                port_name=row.port_name,
                lat=row.lat,
                lng=row.lng,
                fuel_type=row.fuel_type.value if hasattr(row.fuel_type, 'value') else str(row.fuel_type),
                total_stock_mt=total,
                supplier_count=row.supplier_count or 0,
                availability_level=_classify_availability(total),
                avg_price_per_mt=Decimal(str(round(row.avg_price, 2))) if row.avg_price else None,
            )
        )

    return availability_list

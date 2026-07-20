"""Catalog API — public product and delivery point listings."""
from fastapi import APIRouter, Depends
from sqlalchemy import case, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.market_catalog import (
    CANONICAL_PRODUCTS,
    DELIVERY_POINT_DISPLAY_ORDER,
)
from app.models.catalog import Product, DeliveryPoint
from app.schemas.catalog import ProductResponse, DeliveryPointResponse
from app.services.market_data_eligibility import (
    canonical_delivery_point_clause,
    canonical_product_clause,
)

router = APIRouter(prefix="/catalog", tags=["catalog"])

@router.get("/products", response_model=list[ProductResponse])
async def list_products(db: AsyncSession = Depends(get_db)):
    """List all active products."""
    display_order = case(
        {spec.id: index for index, spec in enumerate(CANONICAL_PRODUCTS, start=1)},
        value=Product.id,
        else_=999,
    )
    result = await db.execute(
        select(Product)
        .where(canonical_product_clause(Product))
        .order_by(display_order, Product.name)
    )
    return result.scalars().all()


@router.get("/delivery-points", response_model=list[DeliveryPointResponse])
async def list_delivery_points(db: AsyncSession = Depends(get_db)):
    """List all active delivery points."""
    display_order = case(
        dict(DELIVERY_POINT_DISPLAY_ORDER),
        value=DeliveryPoint.name,
        else_=999,
    )
    result = await db.execute(
        select(DeliveryPoint)
        .where(canonical_delivery_point_clause(DeliveryPoint))
        .order_by(display_order, DeliveryPoint.name)
    )
    return result.scalars().all()

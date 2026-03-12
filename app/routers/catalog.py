"""Catalog API — public product and delivery point listings."""
from fastapi import APIRouter, Depends
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.models.catalog import Product, DeliveryPoint
from app.schemas.catalog import ProductResponse, DeliveryPointResponse

router = APIRouter(prefix="/catalog", tags=["catalog"])


@router.get("/products", response_model=list[ProductResponse])
async def list_products(db: AsyncSession = Depends(get_db)):
    """List all active products."""
    result = await db.execute(
        select(Product).where(Product.is_active.is_(True)).order_by(Product.name)
    )
    return result.scalars().all()


@router.get("/delivery-points", response_model=list[DeliveryPointResponse])
async def list_delivery_points(db: AsyncSession = Depends(get_db)):
    """List all active delivery points."""
    result = await db.execute(
        select(DeliveryPoint).where(DeliveryPoint.is_active.is_(True)).order_by(DeliveryPoint.name)
    )
    return result.scalars().all()

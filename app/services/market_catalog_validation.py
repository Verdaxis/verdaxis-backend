"""Database validation against the exact runtime market catalog contract."""
from __future__ import annotations

from uuid import UUID

from fastapi import HTTPException
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.catalog import DeliveryPoint, Product
from app.services.market_data_eligibility import (
    canonical_delivery_point_clause,
    canonical_product_clause,
)


async def require_canonical_market_slice(
    db: AsyncSession,
    *,
    product_id: UUID,
    delivery_point_id: UUID | None,
) -> tuple[Product, DeliveryPoint]:
    if delivery_point_id is None:
        raise HTTPException(status_code=400, detail="delivery_point_id is required")
    product = (
        await db.execute(
            select(Product).where(
                Product.id == product_id,
                canonical_product_clause(Product),
            )
        )
    ).scalar_one_or_none()
    if product is None:
        raise HTTPException(status_code=400, detail="Invalid product_id")
    point = (
        await db.execute(
            select(DeliveryPoint).where(
                DeliveryPoint.id == delivery_point_id,
                canonical_delivery_point_clause(DeliveryPoint),
            )
        )
    ).scalar_one_or_none()
    if point is None:
        raise HTTPException(status_code=400, detail="Invalid delivery_point_id")
    return product, point

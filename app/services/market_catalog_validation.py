"""Database validation against the exact runtime market catalog contract."""
from __future__ import annotations

from uuid import UUID

from fastapi import HTTPException
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.catalog import DeliveryPoint, Product
from app.market_catalog import PRODUCTS_BY_ID, PRODUCTS_BY_CODE
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
    spec = PRODUCTS_BY_ID.get(product_id)
    if (
        spec is not None
        and spec.available_delivery_point_ids is not None
        and delivery_point_id not in spec.available_delivery_point_ids
    ):
        raise HTTPException(status_code=400, detail="This product is available for Singapore only")
    return product, point


def require_orderbook_product(product: Product) -> None:
    """Keep RFQ-only products out of all executable order entry routes."""
    spec = PRODUCTS_BY_ID.get(getattr(product, "id", None))
    if spec is None:
        spec = PRODUCTS_BY_CODE.get(getattr(product, "market_product", None))
    if spec is not None and spec.execution_mode == "RFQ_ONLY":
        raise HTTPException(status_code=400, detail="This product is RFQ-only. Use the RFQ workspace.")

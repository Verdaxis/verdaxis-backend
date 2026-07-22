"""Seed data for Product and DeliveryPoint catalogs.

Uses deterministic UUIDs so other tasks and tests can reference them by ID.
"""
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.market_catalog import (
    CANONICAL_DELIVERY_POINTS,
    CANONICAL_PRODUCTS,
)
from app.models.catalog import Product, DeliveryPoint


PRODUCTS = [
    Product(
        id=spec.id,
        name=spec.name,
        fuel_type=spec.fuel_type,
        fuel_grade=spec.fuel_grade,
        unit=spec.unit,
        min_lot_size=spec.min_lot_size,
        spec_description=spec.spec_description,
    )
    for spec in CANONICAL_PRODUCTS
]


# ---------------------------------------------------------------------------
# Delivery point catalog
# ---------------------------------------------------------------------------
DELIVERY_POINTS = [
    DeliveryPoint(
        id=spec.id,
        name=spec.name,
        region=spec.region,
        timezone=spec.timezone,
    )
    for spec in CANONICAL_DELIVERY_POINTS
]


# ---------------------------------------------------------------------------
# Public ID accessors — other tasks can import these
# ---------------------------------------------------------------------------
PRODUCT_IDS = {p.name: p.id for p in PRODUCTS}
DELIVERY_POINT_IDS = {dp.name: dp.id for dp in DELIVERY_POINTS}


async def seed_catalog(db: AsyncSession) -> None:
    """Insert and normalize active catalog records."""
    existing_products = (await db.execute(select(Product))).scalars().all()
    existing_products_by_id = {product.id: product for product in existing_products}
    approved_product_ids = {product.id for product in PRODUCTS}

    for p in PRODUCTS:
        existing = existing_products_by_id.get(p.id)
        if existing is None:
            db.add(Product(
                id=p.id,
                name=p.name,
                fuel_type=p.fuel_type,
                fuel_grade=p.fuel_grade,
                unit=p.unit,
                min_lot_size=p.min_lot_size,
                spec_description=p.spec_description,
                is_active=True,
            ))
            continue
        existing.name = p.name
        existing.fuel_type = p.fuel_type
        existing.fuel_grade = p.fuel_grade
        existing.unit = p.unit
        existing.min_lot_size = p.min_lot_size
        existing.spec_description = p.spec_description
        existing.is_active = True

    for existing in existing_products:
        if existing.id not in approved_product_ids:
            existing.is_active = False

    existing_dps = (await db.execute(select(DeliveryPoint))).scalars().all()
    existing_dp_by_id = {dp.id: dp for dp in existing_dps}
    approved_dp_ids = {dp.id for dp in DELIVERY_POINTS}

    for dp in DELIVERY_POINTS:
        existing = existing_dp_by_id.get(dp.id)
        if existing is None:
            db.add(DeliveryPoint(
                id=dp.id,
                name=dp.name,
                region=dp.region,
                timezone=dp.timezone,
                is_active=True,
            ))
            continue
        existing.name = dp.name
        existing.region = dp.region
        existing.timezone = dp.timezone
        existing.is_active = True

    for existing in existing_dps:
        if existing.id not in approved_dp_ids:
            existing.is_active = False

    await db.commit()

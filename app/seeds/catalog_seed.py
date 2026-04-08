"""Seed data for Product and DeliveryPoint catalogs.

Uses deterministic UUIDs so other tasks and tests can reference them by ID.
"""
import uuid
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.catalog import Product, DeliveryPoint


# ---------------------------------------------------------------------------
# Deterministic UUIDs — namespace-based so they're stable across runs
# ---------------------------------------------------------------------------
_NS = uuid.UUID("a1b2c3d4-e5f6-7890-abcd-ef1234567890")


def _product_id(name: str) -> uuid.UUID:
    return uuid.uuid5(_NS, f"product:{name}")


def _dp_id(name: str) -> uuid.UUID:
    return uuid.uuid5(_NS, f"delivery_point:{name}")


# ---------------------------------------------------------------------------
# Product catalog
# ---------------------------------------------------------------------------
PRODUCTS = [
    Product(
        id=_product_id("Bio Ethanol"),
        name="Bio Ethanol",
        fuel_type="Ethanol",
        fuel_grade="Bio",
        unit="MT",
        min_lot_size=200,
        spec_description="Second-generation bioethanol from waste feedstocks",
    ),
    Product(
        id=_product_id("Bio Methanol"),
        name="Bio Methanol",
        fuel_type="Methanol",
        fuel_grade="Bio",
        unit="MT",
        min_lot_size=200,
        spec_description="Bio-methanol produced from biogenic feedstocks for marine fuel use",
    ),
    Product(
        id=_product_id("e-Methanol"),
        name="e-Methanol",
        fuel_type="Methanol",
        fuel_grade="E",
        unit="MT",
        min_lot_size=200,
        spec_description="Synthetic methanol produced from renewable hydrogen and captured CO2",
    ),
    Product(
        id=_product_id("Synthetic Ethanol"),
        name="Synthetic Ethanol",
        fuel_type="Ethanol",
        fuel_grade="Synthetic",
        unit="MT",
        min_lot_size=200,
        spec_description="Synthetic ethanol produced via power-to-liquids or equivalent synthetic pathways",
    ),
]


# ---------------------------------------------------------------------------
# Delivery point catalog
# ---------------------------------------------------------------------------
DELIVERY_POINTS = [
    DeliveryPoint(
        id=_dp_id("ARA"),
        name="ARA",
        region="Europe",
        timezone="Europe/Amsterdam",
    ),
    DeliveryPoint(
        id=_dp_id("Singapore"),
        name="Singapore",
        region="Asia",
        timezone="Asia/Singapore",
    ),
    DeliveryPoint(
        id=_dp_id("Fujairah"),
        name="Fujairah",
        region="Middle East",
        timezone="Asia/Dubai",
    ),
    DeliveryPoint(
        id=_dp_id("Houston"),
        name="Houston",
        region="Americas",
        timezone="America/Chicago",
    ),
    DeliveryPoint(
        id=_dp_id("Rotterdam"),
        name="Rotterdam",
        region="Europe",
        timezone="Europe/Amsterdam",
    ),
]


# ---------------------------------------------------------------------------
# Public ID accessors — other tasks can import these
# ---------------------------------------------------------------------------
PRODUCT_IDS = {p.name: p.id for p in PRODUCTS}
DELIVERY_POINT_IDS = {dp.name: dp.id for dp in DELIVERY_POINTS}


async def seed_catalog(db: AsyncSession) -> None:
    """Insert all catalog records, skipping any that already exist."""
    # Check existing products
    existing_products = (await db.execute(select(Product.id))).scalars().all()
    existing_product_ids = set(existing_products)

    for p in PRODUCTS:
        if p.id not in existing_product_ids:
            db.add(Product(
                id=p.id,
                name=p.name,
                fuel_type=p.fuel_type,
                fuel_grade=p.fuel_grade,
                unit=p.unit,
                min_lot_size=p.min_lot_size,
                spec_description=p.spec_description,
            ))

    # Check existing delivery points
    existing_dps = (await db.execute(select(DeliveryPoint.id))).scalars().all()
    existing_dp_ids = set(existing_dps)

    for dp in DELIVERY_POINTS:
        if dp.id not in existing_dp_ids:
            db.add(DeliveryPoint(
                id=dp.id,
                name=dp.name,
                region=dp.region,
                timezone=dp.timezone,
            ))

    await db.commit()

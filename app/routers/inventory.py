from decimal import Decimal
from typing import Any, Annotated, List
from uuid import UUID

from fastapi import APIRouter, Depends, HTTPException, Request
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.future import select
from sqlalchemy.exc import DBAPIError
from sqlalchemy.orm import selectinload

from app.database import get_db
from app.models.catalog import DeliveryPoint, Product
from app.models.marketplace import InventoryItem, FuelType as ModelFuelType
from app.models.orderbook import (
    OrderBookOrder,
    OrderBookStatus,
    OrderSide,
)
from app.schemas.marketplace import InventoryCreate, InventoryItemUpdate, InventoryResponse
from app.models.user import User, UserRole
from app.routers.auth_simple import get_current_user
from app.services.availability_windows import SPOT_WINDOW
from app.services.audit_service import record_audit, request_audit_context
from app.services.audit_actions import INVENTORY_PUBLISHED
import logging

logger = logging.getLogger(__name__)

router = APIRouter()

SUPPLIER_METADATA_FIELDS = (
    "certification_declared",
    "certification_scheme",
    "specification_standard",
    "msds_available",
    "carbon_intensity_gco2_mj",
    "carbon_intensity_method",
    "feedstock",
    "origin",
    "off_spec",
    "off_spec_notes",
)


async def _resolve_catalog_product(db: AsyncSession, item: InventoryItem) -> Product | None:
    """Map an inventory item to the most defensible active catalog product."""
    product_name = (item.product_name or "").strip()
    if product_name:
        exact_stmt = select(Product).where(
            Product.is_active.is_(True),
            Product.name == product_name,
        )
        exact_result = await db.execute(exact_stmt)
        exact_product = exact_result.scalar_one_or_none()
        if exact_product:
            return exact_product

    raw_fuel_type = item.fuel_type.value if hasattr(item.fuel_type, "value") else str(item.fuel_type)
    fuel_type_candidates = [raw_fuel_type]
    if raw_fuel_type.upper() == "LSMGO":
        fuel_type_candidates.append("MGO")

    stmt = (
        select(Product)
        .where(
            Product.is_active.is_(True),
            Product.fuel_type.in_(fuel_type_candidates),
        )
        .order_by(Product.name)
    )
    result = await db.execute(stmt)
    products = result.scalars().all()
    if not products:
        return None

    preferred_grades = ["Green", "Bio"] if item.is_certified else ["Conventional"]
    preferred_grades.extend(["Conventional", "Green", "Bio"])
    for grade in preferred_grades:
        for product in products:
            if product.fuel_grade == grade:
                return product

    return products[0]


async def _resolve_delivery_point(db: AsyncSession, item: InventoryItem) -> DeliveryPoint | None:
    """Map an inventory item port to a delivery point when the names line up."""
    port = getattr(item, "port", None)
    port_name = getattr(port, "name", None)
    if not port_name:
        return None

    stmt = select(DeliveryPoint).where(
        DeliveryPoint.is_active.is_(True),
        DeliveryPoint.name == port_name.strip(),
    )
    result = await db.execute(stmt)
    return result.scalar_one_or_none()


@router.get("/inventory", response_model=List[InventoryResponse])
async def list_inventory(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)]
):
    if current_user.role != UserRole.SUPPLIER:
        # Buyers might see aggregated inventory, but for now strict scoping
        raise HTTPException(status_code=403, detail="Forbidden")

    stmt = select(InventoryItem).where(InventoryItem.supplier_id == current_user.organization_id)
    result = await db.execute(stmt)
    items = result.scalars().all()
    return items

@router.post("/inventory", response_model=InventoryResponse)
async def add_inventory(
    item: InventoryCreate,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)]
):
    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(status_code=403, detail="Only suppliers can manage inventory")

    if not current_user.organization_id:
        raise HTTPException(status_code=400, detail="User has no organization. Please complete onboarding first.")

    try:
        # Convert schema data to plain dict with string enum values
        # to avoid cross-module enum class mismatches
        item_data = item.model_dump()
        item_data['fuel_type'] = ModelFuelType(item.fuel_type.value)

        db_item = InventoryItem(
            **item_data,
            supplier_id=current_user.organization_id
        )

        db.add(db_item)
        await db.commit()
        await db.refresh(db_item)
        return db_item
    except HTTPException:
        raise
    except DBAPIError:
        raise
    except Exception as e:
        await db.rollback()
        logger.error("inventory_create_failed", extra={"error_class": type(e).__name__})
        raise HTTPException(status_code=500, detail=f"Failed to create inventory item: {str(e)}")

@router.patch("/inventory/{item_id}", response_model=InventoryResponse)
async def update_inventory(
    item_id: UUID,
    updates: InventoryItemUpdate,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)]
):
    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(status_code=403, detail="Only suppliers can manage inventory")

    stmt = select(InventoryItem).where(
        InventoryItem.id == item_id,
        InventoryItem.supplier_id == current_user.organization_id
    )
    result = await db.execute(stmt)
    item = result.scalar_one_or_none()

    if not item:
        raise HTTPException(status_code=404, detail="Inventory item not found")

    update_data = updates.model_dump(exclude_unset=True)
    for field, value in update_data.items():
        setattr(item, field, value)

    from datetime import datetime, UTC
    item.updated_at = datetime.now(UTC)

    await db.commit()
    await db.refresh(item)
    return item

@router.delete("/inventory/{item_id}", status_code=204)
async def delete_inventory(
    item_id: UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)]
):
    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(status_code=403, detail="Only suppliers can manage inventory")

    stmt = select(InventoryItem).where(
        InventoryItem.id == item_id,
        InventoryItem.supplier_id == current_user.organization_id
    )
    result = await db.execute(stmt)
    item = result.scalar_one_or_none()

    if not item:
        raise HTTPException(status_code=404, detail="Inventory item not found")

    await db.delete(item)
    await db.commit()

@router.post("/inventory/{item_id}/publish")
async def publish_inventory_item(
    item_id: UUID,
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
):
    """Convert an inventory item into an ASK listing on the unified orderbook."""
    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(status_code=403, detail="Only suppliers can publish inventory")

    if not current_user.organization_id:
        raise HTTPException(status_code=400, detail="User has no organization")

    stmt = select(InventoryItem).options(selectinload(InventoryItem.port)).where(
        InventoryItem.id == item_id,
        InventoryItem.supplier_id == current_user.organization_id,
    )
    result = await db.execute(stmt)
    item = result.scalar_one_or_none()

    if not item:
        raise HTTPException(status_code=404, detail="Inventory item not found")

    quantity = Decimal(str(item.current_stock_mt or 0))
    if quantity <= 0:
        raise HTTPException(status_code=400, detail="Inventory quantity must be positive")

    if item.price_per_mt_usd is None:
        raise HTTPException(status_code=400, detail="Inventory item missing price")

    product = await _resolve_catalog_product(db, item)
    if not product:
        raise HTTPException(status_code=400, detail="Unable to map inventory item to a catalog product")

    delivery_point = await _resolve_delivery_point(db, item)

    listing = OrderBookOrder(
        organization_id=current_user.organization_id,
        side=OrderSide.ASK,
        product_id=product.id,
        delivery_point_id=delivery_point.id if delivery_point else None,
        port_id=item.port_id,
        quantity_mt=quantity,
        remaining_quantity_mt=quantity,
        price_per_mt_usd=Decimal(str(item.price_per_mt_usd)),
        availability_window=SPOT_WINDOW,
        certifications=["INVENTORY_CERTIFIED"] if item.is_certified else [],
        status=OrderBookStatus.OPEN,
        **{field: getattr(item, field) for field in SUPPLIER_METADATA_FIELDS},
    )
    db.add(listing)
    await db.flush()
    await record_audit(
        db,
        user_id=current_user.id,
        action=INVENTORY_PUBLISHED,
        resource_type="inventory",
        resource_id=item.id,
        changes={
            "inventory_item_id": str(item.id),
            "listing_id": str(listing.id),
            "quantity_mt": str(listing.quantity_mt),
            "price_per_mt_usd": str(listing.price_per_mt_usd),
            "status": listing.status.value,
        },
        **request_audit_context(request),
    )
    await db.commit()
    await db.refresh(listing)

    return {"status": "published", "listing_id": str(listing.id)}


def _listing_payload(order: OrderBookOrder, match_count: int = 0) -> dict[str, Any]:
    return {
        "id": str(order.id),
        "product_name": order.product_name,
        "market_product": order.market_product,
        "fuel_type": order.fuel_type,
        "fuel_grade": order.fuel_grade,
        "quantity_mt": str(order.quantity_mt),
        "price_per_mt_usd": str(order.price_per_mt_usd),
        "region": order.region,
        "availability_window": order.availability_window,
        "certifications": order.certifications or [],
        "certification_declared": order.certification_declared,
        "certification_scheme": order.certification_scheme,
        "specification_standard": order.specification_standard,
        "msds_available": order.msds_available,
        "carbon_intensity_gco2_mj": str(order.carbon_intensity_gco2_mj) if order.carbon_intensity_gco2_mj is not None else None,
        "carbon_intensity_method": order.carbon_intensity_method,
        "feedstock": order.feedstock,
        "origin": order.origin,
        "off_spec": order.off_spec,
        "off_spec_notes": order.off_spec_notes,
        "status": order.status.value if hasattr(order.status, "value") else str(order.status),
        "match_count": match_count,
    }


@router.get("/listings")
async def list_public_listings(db: Annotated[AsyncSession, Depends(get_db)]):
    """Public catalog of open ASK listings."""
    stmt = (
        select(OrderBookOrder)
        .where(
            OrderBookOrder.side == OrderSide.ASK,
            OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
        )
        .order_by(OrderBookOrder.created_at.desc())
    )
    result = await db.execute(stmt)
    listings = result.scalars().all()
    return [_listing_payload(order) for order in listings]


@router.get("/listings/my")
async def list_my_listings(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
):
    """Supplier's own ASK listings with trade counts."""
    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(status_code=403, detail="Only suppliers can view own listings")
    if not current_user.organization_id:
        return []

    stmt = (
        select(OrderBookOrder)
        .options(selectinload(OrderBookOrder.ask_trades))
        .where(
            OrderBookOrder.organization_id == current_user.organization_id,
            OrderBookOrder.side == OrderSide.ASK,
        )
        .order_by(OrderBookOrder.created_at.desc())
    )
    result = await db.execute(stmt)
    listings = result.scalars().all()
    return [_listing_payload(order, match_count=len(order.ask_trades)) for order in listings]

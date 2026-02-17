from decimal import Decimal
from typing import Any, Annotated, List
from uuid import UUID

from fastapi import APIRouter, Depends, HTTPException
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.future import select
from sqlalchemy.orm import selectinload

from app.database import get_db
from app.models.marketplace import InventoryItem, FuelType as ModelFuelType
from app.models.orderbook import (
    AvailabilityWindow,
    FuelGrade,
    OrderBookOrder,
    OrderBookStatus,
    OrderSide,
)
from app.schemas.marketplace import InventoryCreate, InventoryItemUpdate, InventoryResponse
from app.models.user import User, UserRole
from app.core.auth import get_current_user
import logging

logger = logging.getLogger(__name__)

router = APIRouter()

@router.get("/inventory", response_model=List[InventoryResponse])
async def list_inventory(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)]
):
    if current_user.role != UserRole.SUPPLIER:
        # Buyers might see aggregated inventory, but for now strict scoping
        raise HTTPException(status_code=403, detail="Access restricted to Suppliers")

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
    except Exception as e:
        await db.rollback()
        logger.error(f"Failed to create inventory item: {e}", exc_info=True)
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

    from datetime import datetime
    item.updated_at = datetime.utcnow()

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
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
):
    """Convert an inventory item into an ASK listing on the unified orderbook."""
    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(status_code=403, detail="Only suppliers can publish inventory")

    if not current_user.organization_id:
        raise HTTPException(status_code=400, detail="User has no organization")

    stmt = select(InventoryItem).where(
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

    listing = OrderBookOrder(
        organization_id=current_user.organization_id,
        side=OrderSide.ASK,
        fuel_type=item.fuel_type.value if hasattr(item.fuel_type, "value") else str(item.fuel_type),
        fuel_grade=FuelGrade.CONVENTIONAL,
        region=item.port_id or "Unknown",
        port_id=item.port_id,
        quantity_mt=quantity,
        remaining_quantity_mt=quantity,
        price_per_mt_usd=Decimal(str(item.price_per_mt_usd)),
        availability_window=AvailabilityWindow.SPOT,
        certifications=["INVENTORY_CERTIFIED"] if item.is_certified else [],
        status=OrderBookStatus.OPEN,
    )
    db.add(listing)
    await db.commit()
    await db.refresh(listing)

    return {"status": "published", "listing_id": str(listing.id)}


def _listing_payload(order: OrderBookOrder, match_count: int = 0) -> dict[str, Any]:
    return {
        "id": str(order.id),
        "fuel_type": order.fuel_type,
        "quantity_mt": str(order.quantity_mt),
        "price_per_mt_usd": str(order.price_per_mt_usd),
        "region": order.region,
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

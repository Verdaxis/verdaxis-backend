from fastapi import APIRouter, Depends, HTTPException
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.future import select
from app.database import get_db
from app.models.marketplace import InventoryItem
from app.schemas.marketplace import InventoryCreate, InventoryResponse
from app.models.user import User, UserRole
from app.core.auth import get_current_user
from typing import List, Annotated
from uuid import UUID

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

    db_item = InventoryItem(
        **item.model_dump(),
        supplier_id=current_user.organization_id
    )
    
    db.add(db_item)
    await db.commit()
    await db.refresh(db_item)
    return db_item

@router.post("/inventory/{item_id}/publish", response_model=InventoryResponse)
async def publish_inventory_to_listing(
    item_id: UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)]
):
    """
    Publishes an internal inventory item as a PublicListing.
    """
    from app.models.rfq import PublicListing, FuelGrade, AvailabilityWindow, TierLabel, ListingStatus
    
    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(status_code=403, detail="Only suppliers can publish inventory")

    stmt = select(InventoryItem).where(
        InventoryItem.id == item_id,
        InventoryItem.supplier_id == current_user.organization_id
    )
    result = await db.execute(stmt)
    item = result.scalar_one_or_none()
    
    if not item:
        raise HTTPException(status_code=404, detail="Inventory item not found")

    # Create PublicListing from InventoryItem
    # Note: We use some defaults for fields not present in inventory
    new_listing = PublicListing(
        supplier_id=current_user.organization_id,
        region="Global",  # Default region, could be derived from port if available
        fuel_type=item.fuel_type.value,
        fuel_grade=FuelGrade.CONVENTIONAL,
        quantity_mt=item.current_stock_mt,
        price_per_mt_usd=item.price_per_mt_usd,
        availability_window=AvailabilityWindow.SPOT,
        tier_label=TierLabel.REGIONAL_SUPPLIER,
        status=ListingStatus.ACTIVE
    )
    
    db.add(new_listing)
    await db.commit()
    
    return item

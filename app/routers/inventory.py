from fastapi import APIRouter, Depends, HTTPException
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.future import select
from app.database import get_db
from app.models.marketplace import InventoryItem
from app.schemas.marketplace import InventoryCreate, InventoryItemUpdate, InventoryResponse
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


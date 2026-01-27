from fastapi import APIRouter, Depends, HTTPException
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.future import select
from app.database import get_db
from app.models.port import Vessel
from app.schemas.vessel import VesselResponse
from app.core.auth import get_current_user
from app.models.user import User
from typing import List, Annotated

router = APIRouter()

@router.get("/vessels", response_model=List[VesselResponse])
async def list_my_vessels(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)]
):
    # Only return vessels belonging to the user's organization
    stmt = select(Vessel).where(Vessel.organization_id == current_user.organization_id)
    result = await db.execute(stmt)
    vessels = result.scalars().all()
    return vessels

@router.get("/vessels/{vessel_id}", response_model=VesselResponse)
async def get_vessel(
    vessel_id: str,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)]
):
    stmt = select(Vessel).where(
        Vessel.id == vessel_id,
        Vessel.organization_id == current_user.organization_id
    )
    result = await db.execute(stmt)
    vessel = result.scalar_one_or_none()
    
    if not vessel:
        raise HTTPException(status_code=404, detail="Vessel not found")
        
    return vessel

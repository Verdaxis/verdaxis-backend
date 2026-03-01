from fastapi import APIRouter, Depends, HTTPException
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.future import select
from app.database import get_db
from app.models.port import Vessel
from app.schemas.vessel import VesselResponse
from app.routers.auth_simple import get_current_user
from app.models.user import User, UserRole
from typing import List, Annotated
from geoalchemy2 import Geography, Geometry, functions as func

router = APIRouter()

@router.get("/vessels", response_model=List[VesselResponse])
async def list_my_vessels(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)]
):
    # Select Vessel and extract lat/lng
    # Only return vessels belonging to the user's organization, unless the user is an ADMIN
    stmt = select(
        Vessel,
        func.ST_X(Vessel.current_location.cast(Geometry)).label("lng"),
        func.ST_Y(Vessel.current_location.cast(Geometry)).label("lat"),
        func.ST_X(Vessel.previous_location.cast(Geometry)).label("prev_lng"),
        func.ST_Y(Vessel.previous_location.cast(Geometry)).label("prev_lat")
    )
    
    if current_user.role != UserRole.ADMIN:
        if current_user.organization_id is None:
            return []
        stmt = stmt.where(Vessel.organization_id == current_user.organization_id)
    
    result = await db.execute(stmt)
    rows = result.all()
    
    out = []
    for row in rows:
        vessel = row[0]
        vessel.lng = row[1]
        vessel.lat = row[2]
        vessel.prev_lng = row[3]
        vessel.prev_lat = row[4]
        vessel.current_location = None # Prevent serialization error
        vessel.previous_location = None
        
        # Convert UUIDs to strings
        if vessel.id:
            vessel.id = str(vessel.id)
        if vessel.organization_id:
            vessel.organization_id = str(vessel.organization_id)
            
        out.append(vessel)
        
    return out

@router.get("/vessels/{vessel_id}", response_model=VesselResponse)
async def get_vessel(
    vessel_id: str,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)]
):
    stmt = select(
        Vessel,
        func.ST_X(Vessel.current_location.cast(Geometry)).label("lng"),
        func.ST_Y(Vessel.current_location.cast(Geometry)).label("lat"),
        func.ST_X(Vessel.previous_location.cast(Geometry)).label("prev_lng"),
        func.ST_Y(Vessel.previous_location.cast(Geometry)).label("prev_lat")
    ).where(Vessel.id == vessel_id)
    
    if current_user.role != UserRole.ADMIN:
        stmt = stmt.where(Vessel.organization_id == current_user.organization_id)
        
    result = await db.execute(stmt)
    row = result.first()
    
    if not row:
        raise HTTPException(status_code=404, detail="Vessel not found")
        
    vessel = row[0]
    vessel.lng = row[1]
    vessel.lat = row[2]
    vessel.prev_lng = row[3]
    vessel.prev_lat = row[4]
    vessel.current_location = None
    vessel.previous_location = None
    
    # Convert UUIDs to strings
    if vessel.id:
        vessel.id = str(vessel.id)
    if vessel.organization_id:
        vessel.organization_id = str(vessel.organization_id)
        
    return vessel

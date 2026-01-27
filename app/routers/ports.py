from fastapi import APIRouter, Depends, HTTPException, Query
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.future import select
from sqlalchemy.orm import selectinload
from app.database import get_db
from app.models.port import Port
from app.schemas.port import PortResponse
from typing import List, Annotated
from geoalchemy2 import functions as func

router = APIRouter()

@router.get("/ports", response_model=List[PortResponse])
async def list_ports(
    db: Annotated[AsyncSession, Depends(get_db)]
):
    stmt = select(Port).options(selectinload(Port.intelligence))
    result = await db.execute(stmt)
    ports = result.scalars().all()
    
    # Transform Geography to lat/lng for frontend (if needed)
    # Note: Proper Geo handling usually requires ST_AsText or similar in query
    return ports

@router.get("/ports/{port_id}", response_model=PortResponse)
async def get_port(
    port_id: str,
    db: Annotated[AsyncSession, Depends(get_db)]
):
    stmt = select(Port).where(Port.id == port_id).options(selectinload(Port.intelligence))
    result = await db.execute(stmt)
    port = result.scalar_one_or_none()
    
    if not port:
        raise HTTPException(status_code=404, detail="Port not found")
    
    return port

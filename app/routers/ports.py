from fastapi import APIRouter, Depends, HTTPException, Query
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.future import select
from sqlalchemy.orm import selectinload
from app.database import get_db
from app.models.port import Port
from app.schemas.port import PortResponse
from typing import List, Annotated
from geoalchemy2 import Geography, Geometry, functions as func

router = APIRouter()

@router.get("/ports", response_model=List[PortResponse])
async def list_ports(
    db: Annotated[AsyncSession, Depends(get_db)]
):
    # Select Port and its intelligence, and extract lat/lng from location geography
    stmt = select(
        Port,
        func.ST_X(Port.location.cast(Geometry)).label("lng"),
        func.ST_Y(Port.location.cast(Geometry)).label("lat")
    ).options(selectinload(Port.intelligence))
    
    result = await db.execute(stmt)
    rows = result.all()
    
    ports_out = []
    for row in rows:
        port = row[0]
        port.lng = row[1]
        port.lat = row[2]
        # We also need to nullify the 'location' field as it's a WKBElement that Pydantic can't serialize
        port.location = None 
        ports_out.append(port)
    
    return ports_out

@router.get("/ports/{port_id}", response_model=PortResponse)
async def get_port(
    port_id: str,
    db: Annotated[AsyncSession, Depends(get_db)]
):
    stmt = select(
        Port,
        func.ST_X(Port.location.cast(Geometry)).label("lng"),
        func.ST_Y(Port.location.cast(Geometry)).label("lat")
    ).where(Port.id == port_id).options(selectinload(Port.intelligence))
    
    result = await db.execute(stmt)
    row = result.first()
    
    if not row:
        raise HTTPException(status_code=404, detail="Port not found")
    
    port = row[0]
    port.lng = row[1]
    port.lat = row[2]
    port.location = None
    
    return port

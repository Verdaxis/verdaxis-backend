from pydantic import BaseModel
from typing import Optional
from enum import Enum
from datetime import datetime
from app.schemas.port import GeoLocation

class VesselBase(BaseModel):
    name: str
    imo_number: str
    vessel_type: Optional[str] = None
    flag_state: Optional[str] = None
    dwt: Optional[float] = None
    
    cii_rating: Optional[str] = None
    eu_ets_status: Optional[str] = None
    fueleu_status: Optional[str] = None
    
    # In a real app, this would come from PostGIS, here simplified
    # In a real app, this would come from PostGIS, here simplified
    current_location: Optional[str] = None 
    previous_location: Optional[str] = None

    # Virtual fields
    lat: Optional[float] = None
    lng: Optional[float] = None
    prev_lat: Optional[float] = None
    prev_lng: Optional[float] = None

class VesselCreate(VesselBase):
    organization_id: Optional[str] = None

class VesselResponse(VesselBase):
    id: str
    organization_id: Optional[str] = None
    updated_at: datetime
    
    class Config:
        from_attributes = True

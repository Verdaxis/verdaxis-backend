from pydantic import BaseModel, Field
from typing import Optional, List
from enum import Enum
from datetime import datetime

class CongestionLevel(str, Enum):
    Low = "Low"
    Moderate = "Moderate"
    High = "High"

class GeoLocation(BaseModel):
    lat: float
    lng: float

class PortIntelligenceBase(BaseModel):
    congestion_level: Optional[CongestionLevel] = None
    methanol_price_avg: Optional[float] = None
    biofuel_price_avg: Optional[float] = None
    captured_at: datetime

class PortBase(BaseModel):
    id: str
    name: str
    country: str
    location: Optional[str] = None  # WKT or similar representation if needed
    timezone: Optional[str] = None
    is_active: bool = True

    # Virtual fields for frontend convenience
    lat: Optional[float] = None
    lng: Optional[float] = None

class PortResponse(PortBase):
    intelligence: Optional[PortIntelligenceBase] = None
    
    class Config:
        from_attributes = True

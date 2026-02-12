from pydantic import BaseModel, Field
from typing import Optional
from uuid import UUID
from datetime import datetime, date
from decimal import Decimal


class ProducerProjectCreate(BaseModel):
    name: str = Field(..., min_length=1)
    fuel_type: str = Field(..., min_length=1)
    capacity_kt_per_year: Optional[Decimal] = None
    country: str = Field(..., min_length=1)
    region: Optional[str] = None
    lat: Optional[float] = None
    lng: Optional[float] = None
    cod_date: Optional[date] = None
    cod_year: Optional[int] = None
    status: str = "ANNOUNCED"
    data_source: Optional[str] = None
    gena_project_id: Optional[str] = None
    organization_id: Optional[UUID] = None
    feedstock: Optional[str] = None
    technology: Optional[str] = None
    carbon_intensity_gco2_mj: Optional[Decimal] = None
    notes: Optional[str] = None


class ProducerProjectResponse(BaseModel):
    id: UUID
    name: str
    fuel_type: str
    capacity_kt_per_year: Optional[Decimal] = None
    country: str
    region: Optional[str] = None
    lat: Optional[float] = None
    lng: Optional[float] = None
    cod_date: Optional[date] = None
    cod_year: Optional[int] = None
    status: str
    data_source: Optional[str] = None
    gena_project_id: Optional[str] = None
    organization_id: Optional[UUID] = None
    feedstock: Optional[str] = None
    technology: Optional[str] = None
    carbon_intensity_gco2_mj: Optional[Decimal] = None
    notes: Optional[str] = None
    created_at: datetime

    class Config:
        from_attributes = True

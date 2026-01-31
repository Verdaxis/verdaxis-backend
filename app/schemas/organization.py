from pydantic import BaseModel
from typing import Optional
from uuid import UUID
from enum import Enum
from app.models.user import OrgType

class OrganizationBase(BaseModel):
    name: str
    type: OrgType
    tax_id: Optional[str] = None
    country_code: Optional[str] = None

class OrganizationCreate(OrganizationBase):
    pass

class OrganizationResponse(OrganizationBase):
    id: UUID
    domain: Optional[str] = None
    verification_status: str

    class Config:
        from_attributes = True

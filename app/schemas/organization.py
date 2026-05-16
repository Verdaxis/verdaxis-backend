from pydantic import BaseModel
from pydantic import field_validator
from typing import Optional
from uuid import UUID
from enum import Enum
from app.models.user import OrgType, TierLabel


SIGNUP_ORG_TYPES = {
    OrgType.SHIPPING_LINE,
    OrgType.SHIP_MANAGER,
    OrgType.FUEL_BUYER,
    OrgType.CHARTERER,
    OrgType.FUEL_SUPPLIER,
}

class OrganizationBase(BaseModel):
    name: str
    type: OrgType
    tax_id: Optional[str] = None
    country_code: Optional[str] = None

class OrganizationCreate(OrganizationBase):
    @field_validator("type")
    @classmethod
    def validate_signup_org_type(cls, value: OrgType) -> OrgType:
        if value not in SIGNUP_ORG_TYPES:
            allowed = ", ".join(sorted(org_type.value for org_type in SIGNUP_ORG_TYPES))
            raise ValueError(f"Organization type must be one of: {allowed}")
        return value

class OrganizationResponse(OrganizationBase):
    id: UUID
    domain: Optional[str] = None
    verification_status: str
    supplier_tier: Optional[TierLabel] = None

    class Config:
        from_attributes = True

from typing import Optional
from uuid import UUID

from pydantic import BaseModel, field_validator

from app.models.user import OrgType, TierLabel, UserRole


BUY_SIDE_ORG_TYPES = {
    OrgType.SHIPPING_LINE,
    OrgType.SHIP_MANAGER,
    OrgType.FUEL_BUYER,
    OrgType.CHARTERER,
}
SIGNUP_ORG_TYPES = BUY_SIDE_ORG_TYPES | {
    OrgType.FUEL_SUPPLIER,
}


def organization_type_matches_role(role: UserRole, org_type: OrgType) -> bool:
    if role == UserRole.SUPPLIER:
        return org_type == OrgType.FUEL_SUPPLIER
    return role == UserRole.BUYER and org_type in BUY_SIDE_ORG_TYPES


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

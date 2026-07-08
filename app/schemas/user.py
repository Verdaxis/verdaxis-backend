from pydantic import BaseModel, EmailStr, Field
from uuid import UUID
from typing import Optional
from enum import Enum

class UserStatus(str, Enum):
    PENDING = "PENDING"
    APPROVED = "APPROVED"
    REJECTED = "REJECTED"

class UserRole(str, Enum):
    BUYER = "BUYER"
    SUPPLIER = "SUPPLIER"
    ADMIN = "ADMIN"

class UserBase(BaseModel):
    email: EmailStr
    first_name: Optional[str] = None
    last_name: Optional[str] = None
    role: UserRole

class UserCreate(UserBase):
    password: str = Field(..., min_length=8)
    organization_id: Optional[UUID] = None
    referral_code: Optional[str] = None

class UserResponse(UserBase):
    id: UUID
    status: UserStatus
    organization_id: Optional[UUID] = None
    referral_code: Optional[str] = None
    must_change_password: bool = False
    
    class Config:
        from_attributes = True

class UserUpdate(BaseModel):
    first_name: Optional[str] = None
    last_name: Optional[str] = None
    role: Optional[UserRole] = None

class Token(BaseModel):
    access_token: str
    token_type: str

class LoginRequest(BaseModel):
    email: EmailStr
    password: str

class RegistrationResponse(BaseModel):
    status: str # "created", "requires_org"
    user: Optional[UserResponse] = None
    registration_token: Optional[str] = None

# Forward reference since OrganizationCreate is in another file, 
# but circular imports are tricky in Pydantic. 
# Better to define a local mixin or import if possible.
# Actually, I'll put RegisterWithOrgRequest in auth_simple.py or organization.py to avoid circularity if OrganizationCreate needs User.
# Or just put it here and import OrganizationCreate inside the file or use a simple dict for now if needed. 
# Wait, organization.py imports user.py? No, organization.py imports user.py for OrgType (enum).
# So I can import OrganizationCreate here if I'm careful or just define a nested model.
# Simplest: Define it in auth_simple.py or a new schema file.
# I'll stick to defining RegistrationResponse here.

class PasswordChangeRequest(BaseModel):
    current_password: str
    new_password: str

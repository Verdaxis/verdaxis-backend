from pydantic import BaseModel
from typing import Optional
from datetime import datetime
from uuid import UUID

class ComplianceLedgerBase(BaseModel):
    transaction_type: str
    amount: float
    currency: str = "EUR"
    units: Optional[float] = None
    description: Optional[str] = None
    reference_id: Optional[str] = None

class ComplianceLedgerCreate(ComplianceLedgerBase):
    pass

class ComplianceLedgerResponse(ComplianceLedgerBase):
    id: UUID
    organization_id: Optional[UUID] = None
    created_at: datetime
    
    class Config:
        from_attributes = True

class TraceabilityEventBase(BaseModel):
    quote_id: Optional[UUID] = None
    stage: str
    location_name: Optional[str] = None
    timestamp: Optional[datetime] = None
    verification_type: Optional[str] = None
    verification_doc_url: Optional[str] = None

class TraceabilityEventCreate(TraceabilityEventBase):
    pass

class TraceabilityEventResponse(TraceabilityEventBase):
    id: UUID
    is_verified: bool
    verification_hash: Optional[str] = None
    
    class Config:
        from_attributes = True

from sqlalchemy import String, ForeignKey, Enum, Numeric, DateTime, Boolean
from sqlalchemy.orm import Mapped, mapped_column
from sqlalchemy.dialects.postgresql import UUID
import uuid
import enum
from datetime import datetime
from app.database import Base

class TraceabilityEvent(Base):
    __tablename__ = "traceability_events"

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    # Legacy identifier retained for historical traceability records. The
    # retired direct_orders table is not part of the current ORM metadata.
    direct_order_id: Mapped[uuid.UUID | None] = mapped_column(UUID(as_uuid=True))
    
    stage: Mapped[str] = mapped_column(String, nullable=False) # 'Origin', 'Production', 'Bunkering'
    location_name: Mapped[str | None] = mapped_column(String)
    timestamp: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    
    verification_type: Mapped[str | None] = mapped_column(String) # 'DigitalTwin', 'Document', 'PhysicalTracer'
    verification_doc_url: Mapped[str | None] = mapped_column(String)
    verification_hash: Mapped[str | None] = mapped_column(String)
    
    is_verified: Mapped[bool] = mapped_column(Boolean, default=False)

class ComplianceLedger(Base):
    __tablename__ = "compliance_ledger"

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    organization_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("organizations.id"))
    
    transaction_type: Mapped[str | None] = mapped_column(String) # 'EUA_Purchase', 'FuelEU_Penalty', 'Pooling_Transfer'
    amount: Mapped[float | None] = mapped_column(Numeric(12, 2))
    currency: Mapped[str] = mapped_column(String, default='EUR')
    units: Mapped[float | None] = mapped_column(Numeric(10, 2))
    
    description: Mapped[str | None] = mapped_column(String)
    reference_id: Mapped[str | None] = mapped_column(String)
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)

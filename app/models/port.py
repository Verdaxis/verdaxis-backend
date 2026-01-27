from sqlalchemy import String, Boolean, Numeric, ForeignKey, DateTime, Enum
from sqlalchemy.orm import Mapped, mapped_column, relationship
from sqlalchemy.dialects.postgresql import UUID
from geoalchemy2 import Geography
import uuid
import enum
from datetime import datetime
from app.database import Base

class CongestionLevel(str, enum.Enum):
    Low = "Low"
    Moderate = "Moderate"
    High = "High"

class Port(Base):
    __tablename__ = "ports"

    id: Mapped[str] = mapped_column(String, primary_key=True) # e.g. 'sg-sin'
    name: Mapped[str] = mapped_column(String, nullable=False)
    country: Mapped[str] = mapped_column(String, nullable=False)
    location: Mapped[Geography] = mapped_column(Geography(geometry_type='POINT', srid=4326))
    timezone: Mapped[str | None] = mapped_column(String)
    is_active: Mapped[bool] = mapped_column(Boolean, default=True)

    intelligence = relationship("PortIntelligence", back_populates="port", uselist=False)
    inventory_items = relationship("InventoryItem", back_populates="port")

class PortIntelligence(Base):
    __tablename__ = "port_intelligence"

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    port_id: Mapped[str] = mapped_column(ForeignKey("ports.id"))
    congestion_level: Mapped[CongestionLevel | None] = mapped_column(Enum(CongestionLevel))
    methanol_price_avg: Mapped[float | None] = mapped_column(Numeric(10, 2))
    biofuel_price_avg: Mapped[float | None] = mapped_column(Numeric(10, 2))
    captured_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)

    port: Mapped["Port"] = relationship(back_populates="intelligence")

class Vessel(Base):
    __tablename__ = "vessels"

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    organization_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("organizations.id"))
    name: Mapped[str] = mapped_column(String, nullable=False)
    imo_number: Mapped[str] = mapped_column(String, unique=True, nullable=False)
    vessel_type: Mapped[str | None] = mapped_column(String)
    flag_state: Mapped[str | None] = mapped_column(String)
    dwt: Mapped[float | None] = mapped_column(Numeric)

    cii_rating: Mapped[str | None] = mapped_column(String(1))
    eu_ets_status: Mapped[str | None] = mapped_column(String)
    fueleu_status: Mapped[str | None] = mapped_column(String)

    current_location: Mapped[Geography | None] = mapped_column(Geography(geometry_type='POINT', srid=4326))
    updated_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)

    organization = relationship("Organization", back_populates="vessels")


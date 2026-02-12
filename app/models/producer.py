from sqlalchemy import String, ForeignKey, Enum, Numeric, Date, DateTime, Text, Integer
from sqlalchemy.orm import Mapped, mapped_column, relationship
from sqlalchemy.dialects.postgresql import UUID
from geoalchemy2 import Geography
import uuid
import enum
from datetime import datetime, date
from decimal import Decimal
from app.database import Base


class ProjectStatus(str, enum.Enum):
    ANNOUNCED = "ANNOUNCED"
    UNDER_CONSTRUCTION = "UNDER_CONSTRUCTION"
    OPERATIONAL = "OPERATIONAL"
    CANCELLED = "CANCELLED"


class ProducerProject(Base):
    """
    A fuel production project, imported from GENA data or manually added.
    Displayed on the producer map and linked to marketplace listings.
    """
    __tablename__ = "producer_projects"

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    name: Mapped[str] = mapped_column(String, nullable=False)
    fuel_type: Mapped[str] = mapped_column(String(50), nullable=False)
    capacity_kt_per_year: Mapped[Decimal | None] = mapped_column(Numeric(10, 2))

    # Location
    country: Mapped[str] = mapped_column(String(100), nullable=False)
    region: Mapped[str | None] = mapped_column(String(100))
    location: Mapped[Geography | None] = mapped_column(
        Geography(geometry_type='POINT', srid=4326), nullable=True
    )

    # Timeline
    cod_date: Mapped[date | None] = mapped_column(Date, nullable=True, comment="Commercial Operation Date")
    cod_year: Mapped[int | None] = mapped_column(Integer, nullable=True, comment="COD year for filtering")

    # Status
    status: Mapped[ProjectStatus] = mapped_column(
        Enum(ProjectStatus, native_enum=False),
        default=ProjectStatus.ANNOUNCED,
    )

    # Data provenance
    data_source: Mapped[str | None] = mapped_column(String(50), comment="GENA, manual, etc.")
    gena_project_id: Mapped[str | None] = mapped_column(String(100), unique=True, nullable=True)

    # Optional link to a supplier org
    organization_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("organizations.id"), nullable=True)

    # Metadata
    feedstock: Mapped[str | None] = mapped_column(String(200))
    technology: Mapped[str | None] = mapped_column(String(200))
    carbon_intensity_gco2_mj: Mapped[Decimal | None] = mapped_column(Numeric(8, 2))
    notes: Mapped[str | None] = mapped_column(Text)

    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)
    updated_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow, onupdate=datetime.utcnow)

    # Relationships
    organization = relationship("Organization", foreign_keys=[organization_id])

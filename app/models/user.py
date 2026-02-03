from sqlalchemy import String, ForeignKey, Enum, DateTime
from sqlalchemy.orm import Mapped, mapped_column, relationship
from sqlalchemy.dialects.postgresql import UUID
import uuid
import enum
from datetime import datetime
from app.database import Base

class UserRole(str, enum.Enum):
    BUYER = "BUYER"
    SUPPLIER = "SUPPLIER"
    ADMIN = "ADMIN"

class UserStatus(str, enum.Enum):
    PENDING = "PENDING"
    APPROVED = "APPROVED"
    REJECTED = "REJECTED"

class OrgType(str, enum.Enum):
    SHIPPING_LINE = "SHIPPING_LINE"
    FUEL_SUPPLIER = "FUEL_SUPPLIER"
    PORT_AUTHORITY = "PORT_AUTHORITY"


class TierLabel(str, enum.Enum):
    TIER_1_PRODUCER = "TIER_1_PRODUCER"
    MAJOR_TRADER = "MAJOR_TRADER"
    REGIONAL_SUPPLIER = "REGIONAL_SUPPLIER"
    INDEPENDENT = "INDEPENDENT"


class Organization(Base):
    __tablename__ = "organizations"

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    name: Mapped[str] = mapped_column(String, nullable=False)
    domain: Mapped[str | None] = mapped_column(String, unique=True, nullable=True)
    type: Mapped[OrgType] = mapped_column(Enum(OrgType, native_enum=False), nullable=False)
    supplier_tier: Mapped[TierLabel] = mapped_column(Enum(TierLabel, native_enum=False), default=TierLabel.INDEPENDENT)
    tax_id: Mapped[str | None] = mapped_column(String)
    country_code: Mapped[str | None] = mapped_column(String(2))
    verification_status: Mapped[str] = mapped_column(String, default="PENDING")
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)

    users: Mapped[list["User"]] = relationship(back_populates="organization")
    vessels: Mapped[list["Vessel"]] = relationship(back_populates="organization")
    listings: Mapped[list["PublicListing"]] = relationship(back_populates="supplier")

class User(Base):
    __tablename__ = "users"

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    email: Mapped[str] = mapped_column(String, unique=True, nullable=False)
    password_hash: Mapped[str] = mapped_column(String, nullable=False)
    first_name: Mapped[str | None] = mapped_column(String)
    last_name: Mapped[str | None] = mapped_column(String)
    role: Mapped[UserRole | None] = mapped_column(Enum(UserRole, native_enum=False), nullable=True)
    status: Mapped[UserStatus] = mapped_column(Enum(UserStatus, native_enum=False), default=UserStatus.PENDING)
    organization_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("organizations.id"))
    last_login: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=datetime.utcnow)

    organization: Mapped["Organization"] = relationship(back_populates="users")

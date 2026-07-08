from sqlalchemy import String, ForeignKey, Enum, DateTime, Boolean, Text
from sqlalchemy.orm import Mapped, mapped_column, relationship
from sqlalchemy.dialects.postgresql import UUID
import uuid
import enum
from datetime import datetime, UTC
from typing import Optional, TYPE_CHECKING
from app.database import Base

if TYPE_CHECKING:
    from app.models.referral import Referral


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
    SHIP_MANAGER = "SHIP_MANAGER"
    FUEL_BUYER = "FUEL_BUYER"
    FUEL_SUPPLIER = "FUEL_SUPPLIER"
    BUNKER_BROKER = "BUNKER_BROKER"
    PORT_AUTHORITY = "PORT_AUTHORITY"
    FUEL_TRADER = "FUEL_TRADER"
    CHARTERER = "CHARTERER"
    FINANCIER = "FINANCIER"
    INSURER = "INSURER"
    INDUSTRY_ASSOC = "INDUSTRY_ASSOC"


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
    supplier_tier: Mapped[TierLabel | None] = mapped_column(Enum(TierLabel, native_enum=False), nullable=True, default=None)
    tax_id: Mapped[str | None] = mapped_column(String)
    country_code: Mapped[str | None] = mapped_column(String(2))
    verification_status: Mapped[str] = mapped_column(String, default="PENDING")
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=lambda: datetime.now(UTC))

    users: Mapped[list["User"]] = relationship(back_populates="organization")
    vessels: Mapped[list["Vessel"]] = relationship(back_populates="organization")
    orderbook_orders: Mapped[list["OrderBookOrder"]] = relationship(back_populates="organization")

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
    password_changed_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)
    must_change_password: Mapped[bool] = mapped_column(Boolean, default=False, server_default='false', nullable=False)
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=lambda: datetime.now(UTC))

    # Email verification (STORY-010a)
    email_verified: Mapped[bool] = mapped_column(Boolean, default=False, server_default='false', nullable=False)
    email_verification_token: Mapped[Optional[str]] = mapped_column(String(64), nullable=True)

    # KYC (STORY-010b)
    kyc_status: Mapped[str] = mapped_column(String(20), default='PENDING', server_default='PENDING', nullable=False)
    kyc_rejection_reason: Mapped[Optional[str]] = mapped_column(Text, nullable=True)

    # Password reset
    password_reset_token_hash: Mapped[Optional[str]] = mapped_column(String(64), nullable=True)
    password_reset_expires: Mapped[Optional[datetime]] = mapped_column(DateTime(timezone=True), nullable=True)

    # Referrals
    referral_code: Mapped[str | None] = mapped_column(String(10), unique=True, nullable=True)
    referred_by_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id"), nullable=True
    )

    # Onboarding survey (migrated via Alembic)
    onboarding_use_case: Mapped[Optional[str]] = mapped_column(String(50), nullable=True)
    onboarding_referral_source: Mapped[Optional[str]] = mapped_column(String(500), nullable=True)

    organization: Mapped["Organization"] = relationship(back_populates="users")

    referrals_made: Mapped[list["Referral"]] = relationship(
        foreign_keys="Referral.referrer_id", back_populates="referrer"
    )
    referral_received: Mapped["Referral | None"] = relationship(
        foreign_keys="Referral.referred_user_id", back_populates="referred_user", uselist=False
    )

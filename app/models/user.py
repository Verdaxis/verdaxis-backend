from sqlalchemy import String, ForeignKey, Enum, DateTime, Boolean, Index, Text, func, text
from sqlalchemy.orm import Mapped, mapped_column, relationship
from sqlalchemy.dialects.postgresql import UUID
import uuid
import enum
from datetime import datetime, UTC
from typing import Optional, TYPE_CHECKING
from app.database import Base
from app.market_constraints import ORGANIZATION_PROVENANCE_DOMAIN, postgresql_check

if TYPE_CHECKING:
    from app.models.orderbook import OrderBookOrder
    from app.models.port import Vessel
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


class OrganizationProvenance(str, enum.Enum):
    """Immutable classification of the organization that owns market data."""

    UNKNOWN = "UNKNOWN"
    REAL = "REAL"
    DEMO = "DEMO"
    TEST = "TEST"
    CANARY = "CANARY"


class Organization(Base):
    __tablename__ = "organizations"
    __table_args__ = (
        postgresql_check(
            ORGANIZATION_PROVENANCE_DOMAIN,
            name="ck_organizations_provenance",
        ),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    name: Mapped[str] = mapped_column(String, nullable=False)
    domain: Mapped[str | None] = mapped_column(String, unique=True, nullable=True)
    type: Mapped[OrgType] = mapped_column(Enum(OrgType, native_enum=False, length=14), nullable=False)
    supplier_tier: Mapped[TierLabel | None] = mapped_column(Enum(TierLabel, native_enum=False, length=50), nullable=True, default=None)
    tax_id: Mapped[str | None] = mapped_column(String)
    country_code: Mapped[str | None] = mapped_column(String(2))
    verification_status: Mapped[str] = mapped_column(
        String, nullable=False, server_default="PENDING"
    )
    provenance: Mapped[OrganizationProvenance] = mapped_column(
        Enum(OrganizationProvenance, native_enum=False),
        nullable=False,
        server_default=OrganizationProvenance.UNKNOWN.value,
    )
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=lambda: datetime.now(UTC))

    users: Mapped[list["User"]] = relationship(
        back_populates="organization", foreign_keys="User.organization_id"
    )
    vessels: Mapped[list["Vessel"]] = relationship(back_populates="organization")
    orderbook_orders: Mapped[list["OrderBookOrder"]] = relationship(back_populates="organization")

class User(Base):
    __tablename__ = "users"
    __table_args__ = (
        Index("ix_users_referral_code", "referral_code"),
        Index("uq_users_email_lower", text("lower(email)"), unique=True),
        Index("ix_users_email_verification_token_expires_at", "email_verification_token_expires_at"),
        Index("ix_users_password_reset_expires", "password_reset_expires"),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    email: Mapped[str] = mapped_column(String, unique=True, nullable=False)
    password_hash: Mapped[str] = mapped_column(String, nullable=False)
    first_name: Mapped[str | None] = mapped_column(String)
    last_name: Mapped[str | None] = mapped_column(String)
    role: Mapped[UserRole | None] = mapped_column(Enum(UserRole, native_enum=False, length=8), nullable=True)
    status: Mapped[UserStatus] = mapped_column(
        Enum(UserStatus, native_enum=False, length=8), default=UserStatus.PENDING,
        server_default="PENDING", nullable=False,
    )
    organization_id: Mapped[uuid.UUID | None] = mapped_column(ForeignKey("organizations.id"))
    last_login: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    password_changed_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)
    must_change_password: Mapped[bool] = mapped_column(Boolean, default=False, server_default='false', nullable=False)
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=lambda: datetime.now(UTC), server_default=func.now(), nullable=False)

    # Email verification (STORY-010a)
    email_verified: Mapped[bool] = mapped_column(Boolean, default=False, server_default='false', nullable=False)
    email_verification_token_hash: Mapped[Optional[str]] = mapped_column(String(64), nullable=True, index=True)
    email_verification_token_expires_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)

    # KYC (STORY-010b)
    kyc_status: Mapped[str] = mapped_column(String(20), default='PENDING', server_default='PENDING', nullable=False)
    # Organization for which the current submission/review/evidence applies.
    # NULL means unknown/legacy, never implicitly current or approved.
    kyc_organization_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("organizations.id", ondelete="SET NULL"), nullable=True, index=True
    )
    kyc_rejection_reason: Mapped[Optional[str]] = mapped_column(Text, nullable=True)
    kyc_external_evidence_reference: Mapped[Optional[str]] = mapped_column(String(200), nullable=True)
    kyc_review_note: Mapped[Optional[str]] = mapped_column(Text, nullable=True)
    kyc_reviewed_by: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id"), nullable=True
    )
    kyc_reviewed_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)

    # Password reset
    password_reset_token_hash: Mapped[Optional[str]] = mapped_column(String(64), nullable=True)
    password_reset_expires: Mapped[Optional[datetime]] = mapped_column(DateTime(timezone=True), nullable=True)

    # Referrals
    referral_code: Mapped[str | None] = mapped_column(String(10), unique=True, nullable=True)
    referred_by_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id"), nullable=True
    )

    # Onboarding survey (migrated via Alembic)
    onboarding_use_case: Mapped[Optional[str]] = mapped_column(
        String(50), nullable=True,
        comment="Self-reported role from post-verification survey: buyer | supplier | financier_other",
    )
    onboarding_referral_source: Mapped[Optional[str]] = mapped_column(
        String(500), nullable=True,
        comment="Free-text attribution from post-verification survey",
    )

    organization: Mapped["Organization"] = relationship(
        back_populates="users", foreign_keys=[organization_id]
    )

    referrals_made: Mapped[list["Referral"]] = relationship(
        foreign_keys="Referral.referrer_id", back_populates="referrer"
    )
    referral_received: Mapped["Referral | None"] = relationship(
        foreign_keys="Referral.referred_user_id", back_populates="referred_user", uselist=False
    )

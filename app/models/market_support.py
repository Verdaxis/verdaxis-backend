"""Durable capability and exact customer authorization records."""
from __future__ import annotations

import enum
import uuid
from datetime import UTC, datetime
from decimal import Decimal

from sqlalchemy import (
    Boolean,
    CheckConstraint,
    DateTime,
    Enum,
    ForeignKey,
    Index,
    JSON,
    Numeric,
    String,
    Text,
    UniqueConstraint,
    func,
    text,
)
from sqlalchemy.dialects.postgresql import UUID
from sqlalchemy.orm import Mapped, mapped_column

from app.model_base import Base


class MarketSupportCapability(str, enum.Enum):
    MARKET_SUPPORT_LISTINGS = "MARKET_SUPPORT_LISTINGS"
    MARKET_SUPPORT_AUTHORIZATIONS = "MARKET_SUPPORT_AUTHORIZATIONS"


class MarketSupportAuthorizationStatus(str, enum.Enum):
    ACTIVE = "ACTIVE"
    CONSUMED = "CONSUMED"
    REVOKED = "REVOKED"


class StaffCapabilityAssignment(Base):
    __tablename__ = "staff_capability_assignments"
    __table_args__ = (
        UniqueConstraint(
            "user_id", "capability", name="uq_staff_capability_user_capability"
        ),
        Index(
            "ix_staff_capability_active",
            "user_id",
            "capability",
            "revoked_at",
            "expires_at",
        ),
        CheckConstraint(
            "capability IN ('MARKET_SUPPORT_LISTINGS', 'MARKET_SUPPORT_AUTHORIZATIONS')",
            name="ck_staff_capability_name",
        ),
    )

    id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), primary_key=True, default=uuid.uuid4
    )
    user_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id", ondelete="CASCADE"), nullable=False
    )
    capability: Mapped[MarketSupportCapability] = mapped_column(
        Enum(MarketSupportCapability, native_enum=False, length=40), nullable=False
    )
    reason: Mapped[str] = mapped_column(String(500), nullable=False)
    granted_by_user_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id", ondelete="RESTRICT"), nullable=False
    )
    granted_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True),
        nullable=False,
        default=lambda: datetime.now(UTC),
        server_default=func.now(),
    )
    expires_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    revoked_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    revoked_by_user_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id", ondelete="SET NULL")
    )
    revocation_reason: Mapped[str | None] = mapped_column(String(500))


class MarketSupportAuthorization(Base):
    __tablename__ = "market_support_authorizations"
    __table_args__ = (
        CheckConstraint("quantity_mt > 0", name="ck_market_support_auth_quantity"),
        CheckConstraint("price_per_mt_usd > 0", name="ck_market_support_auth_price"),
        CheckConstraint(
            "status IN ('ACTIVE', 'CONSUMED', 'REVOKED')",
            name="ck_market_support_auth_status",
        ),
        CheckConstraint(
            "authorization_expires_at <= order_expires_at",
            name="ck_market_support_auth_expiry_order",
        ),
        UniqueConstraint(
            "organization_id",
            "idempotency_key",
            name="uq_market_support_auth_org_idempotency",
        ),
        Index(
            "ix_market_support_auth_org_status",
            "organization_id",
            "status",
            "created_at",
        ),
    )

    id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), primary_key=True, default=uuid.uuid4
    )
    organization_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True),
        ForeignKey("organizations.id", ondelete="RESTRICT"),
        nullable=False,
    )
    accountable_user_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id", ondelete="RESTRICT"), nullable=False
    )
    status: Mapped[MarketSupportAuthorizationStatus] = mapped_column(
        Enum(MarketSupportAuthorizationStatus, native_enum=False, length=16),
        nullable=False,
        default=MarketSupportAuthorizationStatus.ACTIVE,
        server_default=MarketSupportAuthorizationStatus.ACTIVE.value,
    )

    product_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("products.id", ondelete="RESTRICT"), nullable=False
    )
    delivery_point_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True),
        ForeignKey("delivery_points.id", ondelete="RESTRICT"),
        nullable=False,
    )
    availability_window: Mapped[str] = mapped_column(String(16), nullable=False)
    quantity_mt: Mapped[Decimal] = mapped_column(Numeric(12, 2), nullable=False)
    price_per_mt_usd: Mapped[Decimal] = mapped_column(Numeric(10, 2), nullable=False)
    authorization_expires_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), nullable=False
    )
    order_expires_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), nullable=False)
    is_anonymous: Mapped[bool] = mapped_column(
        Boolean, nullable=False, default=True, server_default="true"
    )

    certifications: Mapped[list[str]] = mapped_column(
        JSON, nullable=False, default=list, server_default=text("'[]'::json")
    )
    certification_declared: Mapped[bool] = mapped_column(Boolean, nullable=False)
    certification_scheme: Mapped[str] = mapped_column(String(120), nullable=False)
    specification_standard: Mapped[str] = mapped_column(String(120), nullable=False)
    msds_available: Mapped[bool] = mapped_column(Boolean, nullable=False)
    carbon_intensity_gco2_mj: Mapped[Decimal] = mapped_column(Numeric(), nullable=False)
    carbon_intensity_method: Mapped[str | None] = mapped_column(String(120))
    feedstock: Mapped[str] = mapped_column(String(255), nullable=False)
    origin: Mapped[str] = mapped_column(String(255), nullable=False)
    off_spec: Mapped[bool] = mapped_column(Boolean, nullable=False, server_default="false")
    off_spec_notes: Mapped[str | None] = mapped_column(Text)

    terms_digest: Mapped[str] = mapped_column(String(64), nullable=False)
    evidence_reference: Mapped[str] = mapped_column(String(500), nullable=False)
    evidence_sha256: Mapped[str] = mapped_column(String(64), nullable=False)
    commercial_consent_version: Mapped[str] = mapped_column(String(64), nullable=False)
    commercial_consent_reference: Mapped[str] = mapped_column(String(500), nullable=False)
    support_case_reference: Mapped[str | None] = mapped_column(String(200))

    idempotency_key: Mapped[str] = mapped_column(String(255), nullable=False)
    idempotency_request_hash: Mapped[str] = mapped_column(String(64), nullable=False)
    created_by_actor_user_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id", ondelete="RESTRICT"), nullable=False
    )
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True),
        nullable=False,
        default=lambda: datetime.now(UTC),
        server_default=func.now(),
    )
    consumed_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    revoked_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    revoked_by_actor_user_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id", ondelete="SET NULL")
    )
    revocation_reason: Mapped[str | None] = mapped_column(String(500))

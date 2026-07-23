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
    Integer,
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


class MarketSupportContextScope(str, enum.Enum):
    ASSISTED_ORDER_ENTRY = "ASSISTED_ORDER_ENTRY"


class MarketSupportContextStatus(str, enum.Enum):
    ACTIVE = "ACTIVE"
    EXITED = "EXITED"
    EXPIRED = "EXPIRED"
    REVOKED = "REVOKED"


class MarketSupportContext(Base):
    """Opaque, short-lived binding between an admin and an organization."""

    __tablename__ = "market_support_contexts"
    __table_args__ = (
        CheckConstraint(
            "scope IN ('ASSISTED_ORDER_ENTRY')", name="ck_market_support_context_scope"
        ),
        CheckConstraint(
            "status IN ('ACTIVE', 'EXITED', 'EXPIRED', 'REVOKED')",
            name="ck_market_support_context_status",
        ),
        CheckConstraint(
            "expires_at > started_at", name="ck_market_support_context_expiry"
        ),
        CheckConstraint("version >= 1", name="ck_market_support_context_version"),
        Index(
            "ix_market_support_context_actor_status",
            "actor_user_id",
            "status",
            "expires_at",
        ),
        Index(
            "ix_market_support_context_organization_status",
            "organization_id",
            "status",
            "expires_at",
        ),
        Index(
            "uq_market_support_context_actor_active",
            "actor_user_id",
            unique=True,
            postgresql_where=text("status = 'ACTIVE'"),
        ),
    )

    id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), primary_key=True, default=uuid.uuid4
    )
    actor_user_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id", ondelete="RESTRICT"), nullable=False
    )
    organization_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True),
        ForeignKey("organizations.id", ondelete="RESTRICT"),
        nullable=False,
    )
    accountable_user_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id", ondelete="RESTRICT"), nullable=False
    )
    support_reference: Mapped[str] = mapped_column(String(200), nullable=False)
    scope: Mapped[MarketSupportContextScope] = mapped_column(
        Enum(MarketSupportContextScope, native_enum=False, length=32),
        nullable=False,
        default=MarketSupportContextScope.ASSISTED_ORDER_ENTRY,
        server_default=MarketSupportContextScope.ASSISTED_ORDER_ENTRY.value,
    )
    started_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), nullable=False, default=lambda: datetime.now(UTC), server_default=func.now()
    )
    expires_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), nullable=False)
    ended_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    status: Mapped[MarketSupportContextStatus] = mapped_column(
        Enum(MarketSupportContextStatus, native_enum=False, length=16),
        nullable=False,
        default=MarketSupportContextStatus.ACTIVE,
        server_default=MarketSupportContextStatus.ACTIVE.value,
    )
    version: Mapped[int] = mapped_column(
        Integer, nullable=False, default=1, server_default="1"
    )


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
            "order_expires_at IS NULL OR authorization_expires_at <= order_expires_at",
            name="ck_market_support_auth_expiry_order",
        ),
        CheckConstraint(
            "order_side IN ('BID', 'ASK')",
            name="ck_market_support_auth_order_side",
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
        Index(
            "ix_market_support_authorizations_context", "market_support_context_id"
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
    market_support_context_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True),
        ForeignKey("market_support_contexts.id", ondelete="RESTRICT"),
        nullable=True,
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
    order_side: Mapped[str] = mapped_column(
        String(8), nullable=False, default="ASK", server_default="ASK"
    )
    authorization_expires_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), nullable=False
    )
    order_expires_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    is_anonymous: Mapped[bool] = mapped_column(
        Boolean, nullable=False, default=True, server_default="true"
    )

    certifications: Mapped[list[str]] = mapped_column(
        JSON, nullable=False, default=list, server_default=text("'[]'::json")
    )
    certification_declared: Mapped[bool] = mapped_column(Boolean, nullable=False)
    certification_scheme: Mapped[str | None] = mapped_column(String(120))
    specification_standard: Mapped[str | None] = mapped_column(String(120))
    msds_available: Mapped[bool] = mapped_column(Boolean, nullable=False)
    carbon_intensity_gco2_mj: Mapped[Decimal | None] = mapped_column(Numeric())
    carbon_intensity_method: Mapped[str | None] = mapped_column(String(120))
    feedstock: Mapped[str | None] = mapped_column(String(255))
    origin: Mapped[str | None] = mapped_column(String(255))
    off_spec: Mapped[bool] = mapped_column(Boolean, nullable=False, server_default="false")
    off_spec_notes: Mapped[str | None] = mapped_column(Text)

    terms_digest: Mapped[str] = mapped_column(String(64), nullable=False)
    evidence_reference: Mapped[str] = mapped_column(String(500), nullable=False)
    evidence_sha256: Mapped[str | None] = mapped_column(String(64))
    commercial_consent_version: Mapped[str] = mapped_column(String(64), nullable=False)
    commercial_consent_reference: Mapped[str] = mapped_column(String(500), nullable=False)
    support_case_reference: Mapped[str | None] = mapped_column(String(200))
    # Forensic facts for context-mode confirmations.  Nullable preserves
    # compatibility with authorizations created before context mode existed.
    instruction_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    acknowledge_exact_terms: Mapped[bool | None] = mapped_column(Boolean)
    acknowledge_executable_standing_order: Mapped[bool | None] = mapped_column(Boolean)

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

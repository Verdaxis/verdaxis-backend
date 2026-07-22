"""Durable least-privilege and attribution records for market support."""
from __future__ import annotations

import enum
import uuid
from datetime import UTC, datetime
from decimal import Decimal

from sqlalchemy import (
    CheckConstraint,
    DateTime,
    Enum,
    ForeignKey,
    Index,
    Integer,
    JSON,
    Numeric,
    String,
    UniqueConstraint,
    event,
    func,
    select,
)
from sqlalchemy.dialects.postgresql import UUID
from sqlalchemy.orm import Mapped, mapped_column

from app.model_base import Base
from app.models.notification import Notification, NotificationType
from app.models.orderbook import OrderBookOrder
from app.models.user import User


class AdminCapability(str, enum.Enum):
    MARKET_SUPPORT_LISTINGS = "MARKET_SUPPORT_LISTINGS"
    MARKET_SUPPORT_AUTHORIZATIONS = "MARKET_SUPPORT_AUTHORIZATIONS"


class SupportAuthorizationStatus(str, enum.Enum):
    ACTIVE = "ACTIVE"
    REVOKED = "REVOKED"


class SupportAuthorizationScope(str, enum.Enum):
    PUBLISH_POST_ONLY_EXECUTABLE_ORDER = "PUBLISH_POST_ONLY_EXECUTABLE_ORDER"
    EDIT_SUPPORT_MANAGED_ORDER = "EDIT_SUPPORT_MANAGED_ORDER"
    CANCEL_SUPPORT_MANAGED_ORDER = "CANCEL_SUPPORT_MANAGED_ORDER"


class SupportActionOperation(str, enum.Enum):
    CREATE = "CREATE"
    UPDATE = "UPDATE"
    CANCEL = "CANCEL"


class SupportManagementAuthority(str, enum.Enum):
    SUPPORT_MANDATE = "SUPPORT_MANDATE"
    CUSTOMER_DIRECT = "CUSTOMER_DIRECT"


class AdminCapabilityGrant(Base):
    __tablename__ = "admin_capability_grants"
    __table_args__ = (
        UniqueConstraint("user_id", "capability", name="uq_admin_capability_grants_user_capability"),
        Index("ix_admin_capability_grants_active", "user_id", "capability", "revoked_at", "expires_at"),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    user_id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), ForeignKey("users.id", ondelete="CASCADE"), nullable=False)
    capability: Mapped[AdminCapability] = mapped_column(
        Enum(AdminCapability, native_enum=False, length=40), nullable=False
    )
    granted_by_user_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id", ondelete="SET NULL"), nullable=True
    )
    reason: Mapped[str] = mapped_column(String(500), nullable=False)
    granted_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=lambda: datetime.now(UTC), server_default=func.now(), nullable=False
    )
    expires_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)
    revoked_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)
    revoked_by_user_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id", ondelete="SET NULL"), nullable=True
    )


class OrganizationSupportAuthorization(Base):
    __tablename__ = "organization_support_authorizations"
    __table_args__ = (
        CheckConstraint("valid_until > valid_from", name="ck_support_authorization_valid_range"),
        CheckConstraint(
            "min_price_per_mt_usd IS NULL OR max_price_per_mt_usd IS NULL "
            "OR min_price_per_mt_usd <= max_price_per_mt_usd",
            name="ck_support_authorization_price_range",
        ),
        CheckConstraint("max_quantity_mt_per_order > 0", name="ck_support_authorization_quantity"),
        CheckConstraint("max_total_open_quantity_mt > 0", name="ck_support_authorization_open_quantity"),
        CheckConstraint(
            "max_order_ttl_hours >= 1 AND max_order_ttl_hours <= 720",
            name="ck_support_authorization_ttl",
        ),
        CheckConstraint(
            "max_uses IS NULL OR (max_uses > 0 AND uses_count >= 0 AND uses_count <= max_uses)",
            name="ck_support_authorization_uses",
        ),
        Index(
            "ix_support_authorizations_org_state_validity",
            "organization_id",
            "status",
            "valid_from",
            "valid_until",
        ),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    organization_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("organizations.id", ondelete="RESTRICT"), nullable=False
    )
    authorized_contact_name: Mapped[str] = mapped_column(String(200), nullable=False)
    authorized_contact_email: Mapped[str] = mapped_column(String(320), nullable=False)
    evidence_reference: Mapped[str] = mapped_column(String(500), nullable=False)
    support_case_reference: Mapped[str | None] = mapped_column(String(200), nullable=True)
    scopes: Mapped[list[str]] = mapped_column(JSON, nullable=False)
    valid_from: Mapped[datetime] = mapped_column(DateTime(timezone=True), nullable=False)
    valid_until: Mapped[datetime] = mapped_column(DateTime(timezone=True), nullable=False)
    status: Mapped[SupportAuthorizationStatus] = mapped_column(
        Enum(SupportAuthorizationStatus, native_enum=False, length=16),
        default=SupportAuthorizationStatus.ACTIVE,
        server_default=SupportAuthorizationStatus.ACTIVE.value,
        nullable=False,
    )
    allowed_side: Mapped[str] = mapped_column(String(3), default="ASK", server_default="ASK", nullable=False)
    product_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("products.id", ondelete="RESTRICT"), nullable=True
    )
    delivery_point_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("delivery_points.id", ondelete="RESTRICT"), nullable=True
    )
    min_price_per_mt_usd: Mapped[Decimal | None] = mapped_column(Numeric(12, 2), nullable=True)
    max_price_per_mt_usd: Mapped[Decimal | None] = mapped_column(Numeric(12, 2), nullable=True)
    max_quantity_mt_per_order: Mapped[Decimal] = mapped_column(Numeric(14, 2), nullable=False)
    max_total_open_quantity_mt: Mapped[Decimal] = mapped_column(Numeric(14, 2), nullable=False)
    max_order_ttl_hours: Mapped[int] = mapped_column(Integer, nullable=False, default=168, server_default="168")
    max_uses: Mapped[int | None] = mapped_column(Integer, nullable=True)
    uses_count: Mapped[int] = mapped_column(Integer, nullable=False, default=0, server_default="0")
    created_by_admin_user_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id", ondelete="RESTRICT"), nullable=False
    )
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=lambda: datetime.now(UTC), server_default=func.now(), nullable=False
    )
    revoked_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)
    revoked_by_admin_user_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id", ondelete="SET NULL"), nullable=True
    )
    revocation_reason: Mapped[str | None] = mapped_column(String(500), nullable=True)


class OrderSupportAttribution(Base):
    __tablename__ = "order_support_attributions"
    __table_args__ = (
        Index("ix_order_support_attributions_org", "organization_id", "created_at"),
        Index("ix_order_support_attributions_authorization", "support_authorization_id", "created_at"),
    )

    order_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("orderbook_orders.id", ondelete="RESTRICT"), primary_key=True
    )
    organization_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("organizations.id", ondelete="RESTRICT"), nullable=False
    )
    accountable_user_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id", ondelete="RESTRICT"), nullable=False
    )
    created_by_admin_user_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id", ondelete="RESTRICT"), nullable=False
    )
    support_authorization_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True),
        ForeignKey("organization_support_authorizations.id", ondelete="RESTRICT"),
        nullable=False,
    )
    submission_method: Mapped[str] = mapped_column(
        String(32), default="VERDAXIS_ASSISTED", server_default="VERDAXIS_ASSISTED", nullable=False
    )
    support_version: Mapped[int] = mapped_column(Integer, default=1, server_default="1", nullable=False)
    management_authority: Mapped[SupportManagementAuthority] = mapped_column(
        Enum(SupportManagementAuthority, native_enum=False, length=24),
        default=SupportManagementAuthority.SUPPORT_MANDATE,
        server_default=SupportManagementAuthority.SUPPORT_MANDATE.value,
        nullable=False,
    )
    customer_adopted_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)
    customer_adopted_by_user_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id", ondelete="SET NULL"), nullable=True
    )
    last_action_actor_user_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id", ondelete="RESTRICT"), nullable=False
    )
    last_action_reason_code: Mapped[str] = mapped_column(String(64), nullable=False)
    support_case_reference: Mapped[str | None] = mapped_column(String(200), nullable=True)
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=lambda: datetime.now(UTC), server_default=func.now(), nullable=False
    )
    updated_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True),
        default=lambda: datetime.now(UTC),
        onupdate=lambda: datetime.now(UTC),
        server_default=func.now(),
        nullable=False,
    )


class MarketSupportActionReceipt(Base):
    __tablename__ = "market_support_action_receipts"
    __table_args__ = (
        UniqueConstraint(
            "organization_id", "operation", "idempotency_key",
            name="uq_market_support_receipts_org_operation_key",
        ),
        Index("ix_market_support_receipts_order", "order_id", "created_at"),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    organization_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("organizations.id", ondelete="RESTRICT"), nullable=False
    )
    operation: Mapped[SupportActionOperation] = mapped_column(
        Enum(SupportActionOperation, native_enum=False, length=16), nullable=False
    )
    idempotency_key: Mapped[str] = mapped_column(String(255), nullable=False)
    request_hash: Mapped[str] = mapped_column(String(64), nullable=False)
    order_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("orderbook_orders.id", ondelete="RESTRICT"), nullable=False
    )
    support_version: Mapped[int] = mapped_column(Integer, nullable=False)
    created_by_admin_user_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id", ondelete="RESTRICT"), nullable=False
    )
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=lambda: datetime.now(UTC), server_default=func.now(), nullable=False
    )


@event.listens_for(OrderBookOrder, "after_insert")
def _persist_assisted_order_attribution(_mapper, connection, target: OrderBookOrder) -> None:
    """Persist assisted attribution, idempotency receipt, and customer notice.

    The orderbook create application function owns the commit.  Using the same
    SQLAlchemy connection here keeps these support records in that exact
    commercial transaction without changing the ordinary order route.
    """
    from app.services.market_support_context import current_market_support_action

    context = current_market_support_action()
    if context is None or context.operation != SupportActionOperation.CREATE.value:
        return
    if (
        target.organization_id != context.target_organization_id
        or target.owner_user_id != context.accountable_user_id
    ):
        raise RuntimeError("delegated order attribution does not match the persisted order")

    now = datetime.now(UTC)
    connection.execute(
        OrderSupportAttribution.__table__.insert().values(
            order_id=target.id,
            organization_id=context.target_organization_id,
            accountable_user_id=context.accountable_user_id,
            created_by_admin_user_id=context.actor_user_id,
            support_authorization_id=context.support_authorization_id,
            submission_method="VERDAXIS_ASSISTED",
            support_version=context.support_version,
            management_authority=SupportManagementAuthority.SUPPORT_MANDATE,
            last_action_actor_user_id=context.actor_user_id,
            last_action_reason_code=context.reason_code,
            support_case_reference=context.support_case_reference,
            created_at=now,
            updated_at=now,
        )
    )
    connection.execute(
        MarketSupportActionReceipt.__table__.insert().values(
            organization_id=context.target_organization_id,
            operation=SupportActionOperation.CREATE,
            idempotency_key=context.idempotency_key,
            request_hash=context.request_hash,
            order_id=target.id,
            support_version=context.support_version,
            created_by_admin_user_id=context.actor_user_id,
            created_at=now,
        )
    )

    recipient_ids = connection.execute(
        select(User.id).where(User.organization_id == context.target_organization_id)
    ).scalars().all()
    if recipient_ids:
        connection.execute(
            Notification.__table__.insert(),
            [
                {
                    "recipient_id": recipient_id,
                    "type": NotificationType.ORDER_UPDATE,
                    "title": "Listing set up by Verdaxis Support",
                    "message": "Verdaxis Support published a post-only listing for your organization.",
                    "data": {
                        "order_id": str(target.id),
                        "submission_method": "VERDAXIS_ASSISTED",
                    },
                    "is_read": False,
                    "created_at": now,
                }
                for recipient_id in recipient_ids
            ],
        )

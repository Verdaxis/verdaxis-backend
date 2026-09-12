"""Subscription model — org-level tier gating."""
import enum
import uuid
from datetime import datetime
from typing import Optional

from decimal import Decimal

from sqlalchemy import Boolean, DateTime, ForeignKey, Index, Numeric, String
from sqlalchemy.dialects.postgresql import UUID
from sqlalchemy.orm import Mapped, mapped_column, relationship

from app.model_base import Base
from app.market_constraints import postgresql_check


class SubscriptionTier(str, enum.Enum):
    FREE = "free"
    STANDARD = "standard"
    ENTERPRISE = "enterprise"


class Subscription(Base):
    __tablename__ = "subscriptions"
    __table_args__ = (
        Index("ix_subscriptions_org_id", "org_id"),
        postgresql_check(
            "seller_fee_per_mt_usd IS NULL OR "
            "(seller_fee_per_mt_usd >= 0 AND seller_fee_per_mt_usd <= 100000 "
            "AND seller_fee_per_mt_usd * 100 = trunc(seller_fee_per_mt_usd * 100))",
            name="ck_subscriptions_seller_fee_per_mt",
        ),
    )

    id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), primary_key=True, default=uuid.uuid4
    )
    org_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True),
        ForeignKey("organizations.id"),
        unique=True,
        nullable=False,
    )
    # Use a Python-level default (callable) so the value is set at object
    # construction time — not only at INSERT (server_default) time.
    tier: Mapped[SubscriptionTier] = mapped_column(
        String,
        nullable=False,
        default=SubscriptionTier.FREE,
    )
    started_at: Mapped[Optional[datetime]] = mapped_column(
        DateTime(timezone=True),
        server_default="now()",
        nullable=True,
        default=None,
    )
    expires_at: Mapped[Optional[datetime]] = mapped_column(
        DateTime(timezone=True), nullable=True, default=None
    )
    is_active: Mapped[bool] = mapped_column(
        Boolean, nullable=False, default=True
    )
    seller_fee_per_mt_usd: Mapped[Decimal | None] = mapped_column(
        Numeric(), nullable=True, default=None
    )

    # Relationship
    organization = relationship("Organization", backref="subscription", uselist=False)

    def __init__(self, **kwargs):
        kwargs.setdefault("tier", SubscriptionTier.FREE)
        kwargs.setdefault("is_active", True)
        super().__init__(**kwargs)

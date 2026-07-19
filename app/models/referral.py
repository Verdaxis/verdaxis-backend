"""Referral tracking model."""
import enum
import uuid
import string
import secrets
from datetime import datetime, UTC

from sqlalchemy import ForeignKey, Enum, Index, String, DateTime, UniqueConstraint
from sqlalchemy.dialects.postgresql import UUID
from sqlalchemy.orm import Mapped, mapped_column, relationship

from app.database import Base


class ReferralStatus(str, enum.Enum):
    SIGNED_UP = "SIGNED_UP"
    VERIFIED = "VERIFIED"
    ACTIVE = "ACTIVE"


def generate_referral_code() -> str:
    """Generate a unique referral code like VDX-7K3MX9."""
    alphabet = string.ascii_uppercase + string.digits
    suffix = "".join(secrets.choice(alphabet) for _ in range(6))
    return f"VDX-{suffix}"


class Referral(Base):
    __tablename__ = "referrals"
    __table_args__ = (Index("ix_referrals_referrer_id", "referrer_id"),)

    id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), primary_key=True, default=uuid.uuid4
    )
    referrer_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id"), nullable=False
    )
    referred_user_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id"), nullable=False, unique=True
    )
    referral_code_used: Mapped[str] = mapped_column(String, nullable=False)
    status: Mapped[ReferralStatus] = mapped_column(
        Enum(ReferralStatus, native_enum=False),
        default=ReferralStatus.SIGNED_UP,
        nullable=False,
    )
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=lambda: datetime.now(UTC)
    )
    verified_at: Mapped[datetime | None] = mapped_column(
        DateTime(timezone=True), nullable=True
    )
    activated_at: Mapped[datetime | None] = mapped_column(
        DateTime(timezone=True), nullable=True
    )

    referrer: Mapped["User"] = relationship(
        foreign_keys=[referrer_id], back_populates="referrals_made"
    )
    referred_user: Mapped["User"] = relationship(
        foreign_keys=[referred_user_id], back_populates="referral_received"
    )

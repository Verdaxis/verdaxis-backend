"""Migration-owned, organization-bound KYC execution grace snapshot."""
from datetime import datetime
from uuid import UUID, uuid4

from sqlalchemy import DateTime, ForeignKey, String, UniqueConstraint
from sqlalchemy.orm import Mapped, mapped_column

from app.database import Base


class KYCExecutionGrace(Base):
    __tablename__ = "kyc_execution_grace"
    __table_args__ = (
        UniqueConstraint("user_id", "organization_id", name="uq_kyc_execution_grace_user_org"),
    )

    id: Mapped[UUID] = mapped_column(primary_key=True, default=uuid4)
    user_id: Mapped[UUID] = mapped_column(ForeignKey("users.id", ondelete="RESTRICT"))
    organization_id: Mapped[UUID] = mapped_column(ForeignKey("organizations.id", ondelete="RESTRICT"))
    kyc_status_at_enrollment: Mapped[str] = mapped_column(String(20))
    enrolled_at: Mapped[datetime] = mapped_column(DateTime(timezone=True))
    expires_at: Mapped[datetime] = mapped_column(DateTime(timezone=True))
    created_by: Mapped[str] = mapped_column(String(64))


class KYCGraceNotice(Base):
    """A durable send claim prevents retries after an ambiguous provider result."""
    __tablename__ = "kyc_grace_notices"

    grace_id: Mapped[UUID] = mapped_column(ForeignKey("kyc_execution_grace.id", ondelete="RESTRICT"), primary_key=True)
    claimed_at: Mapped[datetime] = mapped_column(DateTime(timezone=True))
    sent_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)

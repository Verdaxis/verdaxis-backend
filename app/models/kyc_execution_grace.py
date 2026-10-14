"""One-time KYC grace snapshot for users who could trade before the KYC gate.

Rows are written only by the ``kycg_20261014_prod`` migration. The runtime
role has SELECT authority only, so a later KYC upload or review can never
create, renew, or extend a grace row.
"""
from datetime import datetime
import uuid

from sqlalchemy import CheckConstraint, DateTime, ForeignKey, String, UniqueConstraint
from sqlalchemy.dialects.postgresql import UUID
from sqlalchemy.orm import Mapped, mapped_column

from app.model_base import Base


class KycExecutionGrace(Base):
    __tablename__ = "kyc_execution_grace"
    __table_args__ = (
        UniqueConstraint(
            "user_id",
            "organization_id",
            name="uq_kyc_execution_grace_user_organization",
        ),
        CheckConstraint(
            "kyc_status_at_enrollment IN ('PENDING', 'SUBMITTED')",
            name="ck_kyc_execution_grace_enrollment_status",
        ),
        # The grace period is fixed at enrollment and never renewed.
        CheckConstraint(
            "expires_at = enrolled_at + interval '14 days'",
            name="ck_kyc_execution_grace_period",
        ).ddl_if(dialect="postgresql"),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    user_id: Mapped[uuid.UUID] = mapped_column(
        ForeignKey("users.id", ondelete="CASCADE"), nullable=False
    )
    organization_id: Mapped[uuid.UUID] = mapped_column(
        ForeignKey("organizations.id", ondelete="CASCADE"), nullable=False
    )
    kyc_status_at_enrollment: Mapped[str] = mapped_column(String(20), nullable=False)
    enrolled_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), nullable=False)
    expires_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), nullable=False)
    # The migration revision that wrote the row.
    created_by: Mapped[str] = mapped_column(String(64), nullable=False)

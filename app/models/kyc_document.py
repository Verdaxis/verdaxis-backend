"""Encrypted KYC evidence retained independently of account cleanup."""
from datetime import datetime
from uuid import UUID

from sqlalchemy import (
    CheckConstraint,
    DateTime,
    ForeignKey,
    Index,
    LargeBinary,
    Text,
    func,
)
from sqlalchemy.orm import Mapped, mapped_column

from app.database import Base


class KYCDocument(Base):
    __tablename__ = "kyc_documents"
    __table_args__ = (
        CheckConstraint("kind IN ('IDENTITY', 'COMPANY_REGISTRATION')", name="ck_kyc_documents_kind"),
        CheckConstraint("content_type IN ('image/jpeg', 'image/png', 'application/pdf')", name="ck_kyc_documents_content_type"),
        CheckConstraint("size_bytes > 0", name="ck_kyc_documents_size"),
        Index("ix_kyc_documents_user_created", "user_id", "created_at"),
    )

    id: Mapped[UUID] = mapped_column(primary_key=True)
    submission_id: Mapped[UUID] = mapped_column()
    user_id: Mapped[UUID] = mapped_column(ForeignKey("users.id", ondelete="RESTRICT"))
    organization_id: Mapped[UUID] = mapped_column(ForeignKey("organizations.id", ondelete="RESTRICT"))
    kind: Mapped[str] = mapped_column(Text)
    content_type: Mapped[str] = mapped_column(Text)
    size_bytes: Mapped[int] = mapped_column()
    key_id: Mapped[str] = mapped_column(Text)
    wrapped_key: Mapped[bytes] = mapped_column(LargeBinary)
    key_nonce: Mapped[bytes] = mapped_column(LargeBinary)
    data_nonce: Mapped[bytes] = mapped_column(LargeBinary)
    ciphertext: Mapped[bytes] = mapped_column(LargeBinary)
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), server_default=func.now())

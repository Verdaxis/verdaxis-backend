"""Product Analytics fact tables (plan §2.4).

``UserLoginDay`` is a bounded daily login fact: one row per user per UTC
day, upserted inside the same transaction as ``User.last_login``. It stores
no IP, user agent, token, or credential data, and is pruned to 800 UTC
calendar dates by scripts/prune_product_analytics.py.

``UserStatusTransition`` is an append-only account-status history: workflow
transitions are written atomically with the ``User.status`` change; the
deployment migration inserts one non-backdated ``migration_snapshot`` row per
existing Buyer/Supplier user. Transitions are durable business history and
are never pruned.
"""

from __future__ import annotations

import uuid
from datetime import UTC, date, datetime

from sqlalchemy import Date, DateTime, Enum, ForeignKey, Index, Integer, String, UniqueConstraint
from sqlalchemy.dialects.postgresql import UUID
from sqlalchemy.orm import Mapped, mapped_column

from app.model_base import Base
from app.models.user import UserRole, UserStatus

STATUS_TRANSITION_PROVENANCE_WORKFLOW = "workflow"
STATUS_TRANSITION_PROVENANCE_MIGRATION = "migration_snapshot"


class UserLoginDay(Base):
    __tablename__ = "user_login_days"
    __table_args__ = (
        UniqueConstraint("activity_date", "user_id", name="uq_user_login_days_date_user"),
        Index("ix_user_login_days_date_role", "activity_date", "role"),
        Index("ix_user_login_days_org_date", "organization_id", "activity_date"),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    activity_date: Mapped[date] = mapped_column(Date, nullable=False)  # UTC calendar date
    user_id: Mapped[uuid.UUID] = mapped_column(
        ForeignKey("users.id", ondelete="CASCADE"), nullable=False
    )
    # Snapshots at login time; deliberately not foreign keys so later
    # organization changes never rewrite history.
    organization_id: Mapped[uuid.UUID | None] = mapped_column(UUID(as_uuid=True), nullable=True)
    role: Mapped[UserRole | None] = mapped_column(Enum(UserRole, native_enum=False), nullable=True)
    login_count: Mapped[int] = mapped_column(Integer, nullable=False, default=1)
    first_login_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), nullable=False)
    last_login_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), nullable=False)


class UserStatusTransition(Base):
    __tablename__ = "user_status_transitions"
    __table_args__ = (
        Index("ix_user_status_transitions_user_time", "user_id", "effective_at"),
        Index("ix_user_status_transitions_status_time", "to_status", "effective_at"),
    )

    id: Mapped[uuid.UUID] = mapped_column(UUID(as_uuid=True), primary_key=True, default=uuid.uuid4)
    user_id: Mapped[uuid.UUID] = mapped_column(
        ForeignKey("users.id", ondelete="CASCADE"), nullable=False
    )
    organization_id: Mapped[uuid.UUID | None] = mapped_column(UUID(as_uuid=True), nullable=True)
    role: Mapped[UserRole | None] = mapped_column(Enum(UserRole, native_enum=False), nullable=True)
    from_status: Mapped[UserStatus | None] = mapped_column(
        Enum(UserStatus, native_enum=False), nullable=True
    )
    to_status: Mapped[UserStatus] = mapped_column(Enum(UserStatus, native_enum=False), nullable=False)
    effective_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), nullable=False, default=lambda: datetime.now(UTC)
    )
    provenance: Mapped[str] = mapped_column(
        String(32), nullable=False, default=STATUS_TRANSITION_PROVENANCE_WORKFLOW
    )

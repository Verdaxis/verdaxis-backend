"""Add Product Analytics fact tables (plan §2.4).

Creates ``user_login_days`` (bounded daily login facts) and
``user_status_transitions`` (append-only status history), then inserts one
``migration_snapshot`` transition per existing Buyer/Supplier user at
deployment time. Snapshot rows are deliberately NOT backdated: historical
approval intervals before this deployment are unknowable, and analytics
returns null with insufficient_coverage for periods before transition
coverage begins.

Revision ID: pa_20260715_analytics_facts
Revises: pref_20260709_user_preferences
"""

from __future__ import annotations

import sqlalchemy as sa
from alembic import op
from sqlalchemy.dialects.postgresql import UUID

revision = "pa_20260715_analytics_facts"
down_revision = "pref_20260709_user_preferences"
branch_labels = None
depends_on = None

_ROLE_ENUM = sa.Enum("BUYER", "SUPPLIER", "ADMIN", name="userrole", native_enum=False)
_STATUS_ENUM = sa.Enum("PENDING", "APPROVED", "REJECTED", name="userstatus", native_enum=False)


def upgrade() -> None:
    op.create_table(
        "user_login_days",
        sa.Column("id", UUID(as_uuid=True), primary_key=True),
        sa.Column("activity_date", sa.Date(), nullable=False),
        sa.Column(
            "user_id",
            UUID(as_uuid=True),
            sa.ForeignKey("users.id", ondelete="CASCADE"),
            nullable=False,
        ),
        sa.Column("organization_id", UUID(as_uuid=True), nullable=True),
        sa.Column("role", _ROLE_ENUM, nullable=True),
        sa.Column("login_count", sa.Integer(), nullable=False),
        sa.Column("first_login_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("last_login_at", sa.DateTime(timezone=True), nullable=False),
        sa.UniqueConstraint("activity_date", "user_id", name="uq_user_login_days_date_user"),
    )
    op.create_index(
        "ix_user_login_days_date_role", "user_login_days", ["activity_date", "role"]
    )
    op.create_index(
        "ix_user_login_days_org_date", "user_login_days", ["organization_id", "activity_date"]
    )

    op.create_table(
        "user_status_transitions",
        sa.Column("id", UUID(as_uuid=True), primary_key=True),
        sa.Column(
            "user_id",
            UUID(as_uuid=True),
            sa.ForeignKey("users.id", ondelete="CASCADE"),
            nullable=False,
        ),
        sa.Column("organization_id", UUID(as_uuid=True), nullable=True),
        sa.Column("role", _ROLE_ENUM, nullable=True),
        sa.Column("from_status", _STATUS_ENUM, nullable=True),
        sa.Column("to_status", _STATUS_ENUM, nullable=False),
        sa.Column("effective_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("provenance", sa.String(length=32), nullable=False),
    )
    op.create_index(
        "ix_user_status_transitions_user_time",
        "user_status_transitions",
        ["user_id", "effective_at"],
    )
    op.create_index(
        "ix_user_status_transitions_status_time",
        "user_status_transitions",
        ["to_status", "effective_at"],
    )

    # Non-backdated snapshot: one row per existing Buyer/Supplier user with
    # the CURRENT status at deployment time. gen_random_uuid() is built into
    # PostgreSQL 13+.
    op.execute(
        sa.text(
            "INSERT INTO user_status_transitions "
            "(id, user_id, organization_id, role, from_status, to_status, "
            "effective_at, provenance) "
            "SELECT gen_random_uuid(), id, organization_id, role, NULL, status, "
            "now(), 'migration_snapshot' "
            "FROM users WHERE role IN ('BUYER', 'SUPPLIER')"
        )
    )


def downgrade() -> None:
    op.drop_index("ix_user_status_transitions_status_time", table_name="user_status_transitions")
    op.drop_index("ix_user_status_transitions_user_time", table_name="user_status_transitions")
    op.drop_table("user_status_transitions")
    op.drop_index("ix_user_login_days_org_date", table_name="user_login_days")
    op.drop_index("ix_user_login_days_date_role", table_name="user_login_days")
    op.drop_table("user_login_days")

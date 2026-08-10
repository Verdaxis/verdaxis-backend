"""add durable account-approval email retry marker

Revision ID: ae_20260811_approval_email
Revises: fb_20260804_feedback_entries
Create Date: 2026-08-11
"""

from __future__ import annotations

import sqlalchemy as sa
from alembic import op
from sqlalchemy.dialects.postgresql import JSONB, UUID


revision = "ae_20260811_approval_email"
down_revision = "fb_20260804_feedback_entries"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column(
        "users",
        sa.Column(
            "pending_approval_email_transition_id",
            UUID(as_uuid=True),
            nullable=True,
        ),
    )
    op.add_column(
        "users",
        sa.Column("pending_approval_email_payload", JSONB(), nullable=True),
    )
    op.add_column(
        "users",
        sa.Column(
            "pending_approval_email_retry_at",
            sa.DateTime(timezone=True),
            nullable=True,
        ),
    )
    op.create_index(
        "ix_users_pending_approval_email_retry",
        "users",
        ["pending_approval_email_retry_at"],
        unique=False,
        postgresql_where=sa.text(
            "pending_approval_email_transition_id IS NOT NULL"
        ),
    )


def downgrade() -> None:
    op.drop_index("ix_users_pending_approval_email_retry", table_name="users")
    op.drop_column("users", "pending_approval_email_retry_at")
    op.drop_column("users", "pending_approval_email_payload")
    op.drop_column("users", "pending_approval_email_transition_id")

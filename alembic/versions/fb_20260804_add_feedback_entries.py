"""add feedback_entries

Revision ID: fb_20260804_feedback_entries
Revises: ms_20260723_assisted_order_v2
Create Date: 2026-08-04

Voluntary, identified in-app feedback (docs/feedback.md). Append-only.
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op
from sqlalchemy.dialects.postgresql import UUID


revision = "fb_20260804_feedback_entries"
down_revision = "ms_20260723_assisted_order_v2"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "feedback_entries",
        sa.Column("id", UUID(as_uuid=True), nullable=False),
        sa.Column("user_id", UUID(as_uuid=True), nullable=False),
        sa.Column("message", sa.String(length=2000), nullable=False),
        sa.Column("page", sa.String(length=200), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index(
        op.f("ix_feedback_entries_user_id"),
        "feedback_entries",
        ["user_id"],
        unique=False,
    )


def downgrade() -> None:
    op.drop_index(op.f("ix_feedback_entries_user_id"), table_name="feedback_entries")
    op.drop_table("feedback_entries")

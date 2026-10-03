"""add browser-reported identified activity delivery loss

Revision ID: uadl_20261003_delivery_reports
Revises: rcp_20261003_command_results
Create Date: 2026-10-03
"""

from __future__ import annotations

import sqlalchemy as sa
from alembic import op
from sqlalchemy.dialects import postgresql


revision = "uadl_20261003_delivery_reports"
down_revision = "rcp_20261003_command_results"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "user_activity_delivery_reports",
        sa.Column("user_id", postgresql.UUID(as_uuid=True), nullable=False),
        sa.Column("report_id", postgresql.UUID(as_uuid=True), nullable=False),
        sa.Column("dropped_events", sa.Integer(), nullable=False),
        sa.Column("rejected_events", sa.Integer(), nullable=False),
        sa.Column(
            "received_at",
            sa.DateTime(timezone=True),
            server_default=sa.text("now()"),
            nullable=False,
        ),
        sa.CheckConstraint(
            "dropped_events BETWEEN 0 AND 10000",
            name="ck_user_activity_delivery_reports_dropped",
        ),
        sa.CheckConstraint(
            "rejected_events BETWEEN 0 AND 10000",
            name="ck_user_activity_delivery_reports_rejected",
        ),
        sa.CheckConstraint(
            "dropped_events + rejected_events > 0",
            name="ck_user_activity_delivery_reports_positive_loss",
        ),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint(
            "user_id",
            "report_id",
            name="pk_user_activity_delivery_reports",
        ),
    )
    op.create_index(
        "ix_user_activity_delivery_reports_user_received",
        "user_activity_delivery_reports",
        ["user_id", "received_at"],
        unique=False,
    )
    op.create_index(
        "ix_user_activity_delivery_reports_received",
        "user_activity_delivery_reports",
        ["received_at"],
        unique=False,
    )


def downgrade() -> None:
    op.drop_index(
        "ix_user_activity_delivery_reports_received",
        table_name="user_activity_delivery_reports",
    )
    op.drop_index(
        "ix_user_activity_delivery_reports_user_received",
        table_name="user_activity_delivery_reports",
    )
    op.drop_table("user_activity_delivery_reports")

"""add consent-linked user browsing activity

Revision ID: ua_20260925_user_activity
Revises: fee_20260912_seller_per_mt
Create Date: 2026-09-25
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op
from sqlalchemy.dialects import postgresql


revision = "ua_20260925_user_activity"
down_revision = "fee_20260912_seller_per_mt"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "user_browsing_events",
        sa.Column("user_id", postgresql.UUID(as_uuid=True), nullable=False),
        sa.Column("event_id", postgresql.UUID(as_uuid=True), nullable=False),
        sa.Column("consent_version", sa.Integer(), nullable=False),
        sa.Column("action", sa.String(length=32), nullable=False),
        sa.Column("page", sa.String(length=64), nullable=False),
        sa.Column("market_product", sa.String(length=32), nullable=True),
        sa.Column("delivery_point_id", postgresql.UUID(as_uuid=True), nullable=True),
        sa.Column("availability_window", sa.String(length=16), nullable=True),
        sa.Column(
            "received_at",
            sa.DateTime(timezone=True),
            server_default=sa.text("now()"),
            nullable=False,
        ),
        sa.CheckConstraint(
            "action IN ('page_view', 'market_view', 'market_filter')",
            name="ck_user_browsing_events_action",
        ),
        sa.CheckConstraint(
            "consent_version = 2",
            name="ck_user_browsing_events_consent_version",
        ),
        sa.CheckConstraint(
            "length(page) BETWEEN 1 AND 64",
            name="ck_user_browsing_events_page_length",
        ),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint(
            "user_id",
            "event_id",
            name="pk_user_browsing_events",
        ),
    )
    op.create_index(
        "ix_user_browsing_events_user_received",
        "user_browsing_events",
        ["user_id", "received_at"],
        unique=False,
    )
    op.create_index(
        "ix_user_browsing_events_received",
        "user_browsing_events",
        ["received_at"],
        unique=False,
    )
    op.create_index(
        "ix_audit_logs_user_timestamp",
        "audit_logs",
        ["user_id", "timestamp"],
        unique=False,
    )
    op.create_index(
        "ix_user_login_days_user_last_login",
        "user_login_days",
        ["user_id", "last_login_at"],
        unique=False,
    )


def downgrade() -> None:
    op.drop_index(
        "ix_user_login_days_user_last_login",
        table_name="user_login_days",
    )
    op.drop_index("ix_audit_logs_user_timestamp", table_name="audit_logs")
    op.drop_index(
        "ix_user_browsing_events_received",
        table_name="user_browsing_events",
    )
    op.drop_index(
        "ix_user_browsing_events_user_received",
        table_name="user_browsing_events",
    )
    op.drop_table("user_browsing_events")

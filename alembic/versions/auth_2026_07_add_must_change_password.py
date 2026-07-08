"""add must_change_password to users

Revision ID: auth_20260708_pw_change
Revises: fc_2026_06_monitor_signals
Create Date: 2026-07-08
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op


revision = "auth_20260708_pw_change"
down_revision = "fc_2026_06_monitor_signals"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column(
        "users",
        sa.Column(
            "must_change_password",
            sa.Boolean(),
            nullable=False,
            server_default=sa.text("false"),
        ),
    )


def downgrade() -> None:
    op.drop_column("users", "must_change_password")

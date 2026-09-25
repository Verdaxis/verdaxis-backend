"""make the legacy activity consent marker optional

Revision ID: ua_20260926_activity_policy
Revises: ua_20260925_user_activity
Create Date: 2026-09-26
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op


revision = "ua_20260926_activity_policy"
down_revision = "ua_20260925_user_activity"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.alter_column(
        "user_browsing_events",
        "consent_version",
        existing_type=sa.Integer(),
        nullable=True,
    )


def downgrade() -> None:
    # Deliberately let PostgreSQL reject this if new NULL rows exist. A
    # downgrade must not fabricate a historical consent marker.
    op.alter_column(
        "user_browsing_events",
        "consent_version",
        existing_type=sa.Integer(),
        nullable=False,
    )

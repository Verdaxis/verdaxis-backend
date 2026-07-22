"""Bind refresh families to opaque browser device sessions.

Revision ID: sec_20260720_device
Revises: sec_20260720_fresh
"""

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


revision: str = "sec_20260720_device"
down_revision: Union[str, None] = "sec_20260720_fresh"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.add_column(
        "refresh_sessions",
        sa.Column("device_id_hash", sa.String(length=64), nullable=True),
    )
    # There is no trustworthy browser identifier to backfill. Force one
    # explicit sign-in rather than let a delayed legacy cookie attach itself
    # to a newly-created device after a cross-account login.
    op.execute(
        sa.text(
            "UPDATE refresh_sessions SET revoked = true "
            "WHERE device_id_hash IS NULL"
        )
    )
    op.create_index(
        "ix_refresh_sessions_device_id_hash",
        "refresh_sessions",
        ["device_id_hash"],
    )


def downgrade() -> None:
    op.drop_index("ix_refresh_sessions_device_id_hash", table_name="refresh_sessions")
    op.drop_column("refresh_sessions", "device_id_hash")

"""Add a distinct administrative authentication-revocation cutoff.

Revision ID: auth_20261002_session_cutoff
Revises: catalog_20260926_biofuels
Create Date: 2026-10-02
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op


revision = "auth_20261002_session_cutoff"
down_revision = "catalog_20260926_biofuels"
branch_labels = None
depends_on = None


def upgrade() -> None:
    # Keep bounded DDL fail-fast behind active user writes.
    bind = op.get_bind()
    bind.execute(
        sa.text("LOCK TABLE users IN ACCESS EXCLUSIVE MODE NOWAIT")
    )
    # NULL preserves all existing sessions. Rejection writes the first cutoff
    # under the locked User row; there is no historical inference or backfill.
    op.add_column(
        "users",
        sa.Column("authentication_revoked_at", sa.DateTime(timezone=True), nullable=True),
    )


def downgrade() -> None:
    # Dropping an active cutoff would revive pre-rejection access tokens after
    # rollback. Refuse once this security state has ever been persisted.
    bind = op.get_bind()
    # Block concurrent writers before reading the guard. NOWAIT fails safely
    # instead of waiting behind runtime authentication traffic.
    bind.execute(
        sa.text("LOCK TABLE users IN ACCESS EXCLUSIVE MODE NOWAIT")
    )
    used = bind.execute(
        sa.text(
            "SELECT EXISTS ("
            "SELECT 1 FROM users WHERE authentication_revoked_at IS NOT NULL"
            ")"
        )
    ).scalar_one()
    if used:
        raise RuntimeError(
            "cannot downgrade authentication cutoff after a session revocation"
        )
    op.drop_column("users", "authentication_revoked_at")

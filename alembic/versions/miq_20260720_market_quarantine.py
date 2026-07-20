"""Add explicit market seed and quarantine records.

Revision ID: miq_20260720_market_quarantine
Revises: sec_20260720_device

Integration note: reparented from pa_20260715_analytics_facts onto the linear
runtime + security chain (pa -> rh -> sec_identity -> sec_boundaries ->
sec_fresh -> sec_device -> miq -> mi).
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op

revision = "miq_20260720_market_quarantine"
down_revision = "sec_20260720_device"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "seed_runs",
        sa.Column("id", sa.UUID(), nullable=False),
        sa.Column("seed_name", sa.String(length=80), nullable=False),
        sa.Column("environment", sa.String(length=32), nullable=False),
        sa.Column("metadata", sa.JSON(), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()),
        sa.Column("completed_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint("seed_name", "environment", name="uq_seed_runs_name_environment"),
    )
    op.create_table(
        "market_row_quarantines",
        sa.Column("id", sa.UUID(), nullable=False),
        sa.Column("source_table", sa.String(length=64), nullable=False),
        sa.Column("source_id", sa.UUID(), nullable=False),
        sa.Column("original_row", sa.JSON(), nullable=False),
        sa.Column("dependencies", sa.JSON(), nullable=False),
        sa.Column("environment", sa.String(length=32), nullable=False),
        sa.Column("database_name", sa.String(length=128), nullable=False),
        sa.Column("reason", sa.Text(), nullable=False),
        sa.Column("operator", sa.String(length=255), nullable=False),
        sa.Column("reference", sa.String(length=255), nullable=False),
        sa.Column("quarantined_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint("source_table", "source_id", name="uq_market_row_quarantines_source"),
    )
    # Deployment grants a distinct migrator/operator role explicitly. PUBLIC
    # (and therefore an ordinary least-privilege app role) cannot write seed
    # or quarantine control history.
    op.execute(sa.text("REVOKE ALL ON seed_runs FROM PUBLIC"))
    op.execute(sa.text("REVOKE ALL ON market_row_quarantines FROM PUBLIC"))


def downgrade() -> None:
    raise RuntimeError(
        "miq_20260720_market_quarantine downgrade is unsupported: "
        "seed and quarantine audit history cannot be reconstructed safely"
    )

"""Add lookup indexes for the vessel and producer reads.

Revision ID: p5_20261020_read_indexes
Revises: prc_20261018_cache
Create Date: 2026-10-20

Three ordinary B-tree indexes, built inside the migration transaction. A failure
rolls every index back; no partial or concurrent build is left behind.
"""
from __future__ import annotations

from alembic import op

revision = "p5_20261020_read_indexes"
down_revision = "prc_20261018_cache"
branch_labels = None
depends_on = None

# Index name -> (table, key columns in order). None of them is unique.
INDEXES = {
    "ix_p5_vessels_org_id": ("vessels", ("organization_id", "id")),
    "ix_p5_producers_cod_name_id": ("producer_projects", ("cod_year", "name", "id")),
    "ix_p5_producers_status_cod_id": ("producer_projects", ("status", "cod_year", "id")),
}
TABLES = ("vessels", "producer_projects")


def upgrade() -> None:
    # CREATE INDEX holds SHARE mode until commit. Taking it on both tables up
    # front without waiting fails fast behind an active writer, rather than
    # queueing behind it and blocking every later writer. Readers are not blocked.
    op.execute(
        "LOCK TABLE "
        + ", ".join(f'public."{table}"' for table in TABLES)
        + " IN SHARE MODE NOWAIT"
    )
    for name, (table, columns) in INDEXES.items():
        op.create_index(name, table, list(columns), unique=False)


def downgrade() -> None:
    for name, (table, _columns) in reversed(INDEXES.items()):
        op.drop_index(name, table_name=table)

"""add benchmarks table

Revision ID: bm_2026_04_benchmarks
Revises: ob_2026_04_supplier_meta
Create Date: 2026-04-08
"""

from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects import postgresql


revision = "bm_2026_04_benchmarks"
down_revision = "ob_2026_04_supplier_meta"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "benchmarks",
        sa.Column("id", postgresql.UUID(as_uuid=True), nullable=False),
        sa.Column("market_product", sa.String(length=64), nullable=False),
        sa.Column("delivery_point_id", postgresql.UUID(as_uuid=True), nullable=False),
        sa.Column("availability_window", sa.String(length=16), nullable=False),
        sa.Column("price_per_mt_usd", sa.Numeric(10, 2), nullable=False),
        sa.Column("source", sa.String(length=32), nullable=False, server_default="manual_override"),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.text("now()")),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.text("now()")),
        sa.ForeignKeyConstraint(["delivery_point_id"], ["delivery_points.id"]),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint(
            "market_product",
            "delivery_point_id",
            "availability_window",
            name="uq_benchmarks_market_key",
        ),
    )


def downgrade() -> None:
    op.drop_table("benchmarks")

"""add persisted live slice benchmark table

Revision ID: sb_2026_04_live_slice_benchmarks
Revises: ix_2026_04_exec_perf
Create Date: 2026-04-15
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op
from sqlalchemy.dialects.postgresql import UUID


revision = "sb_2026_04_live_slice_benchmarks"
down_revision = "ix_2026_04_exec_perf"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "live_slice_benchmarks",
        sa.Column("id", UUID(as_uuid=True), nullable=False),
        sa.Column("side", sa.Enum("BID", "ASK", name="orderside", native_enum=False), nullable=False),
        sa.Column("market_product", sa.String(length=64), nullable=False),
        sa.Column("delivery_point_id", UUID(as_uuid=True), sa.ForeignKey("delivery_points.id"), nullable=False),
        sa.Column("availability_window", sa.String(length=16), nullable=False),
        sa.Column("benchmark_price_per_mt_usd", sa.Numeric(10, 2), nullable=False),
        sa.Column("total_remaining_quantity_mt", sa.Numeric(12, 2), nullable=False),
        sa.Column("order_count", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("source", sa.String(length=32), nullable=False, server_default="live_slice_vwap"),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint(
            "side",
            "market_product",
            "delivery_point_id",
            "availability_window",
            name="uq_live_slice_benchmarks_slice_key",
        ),
    )
    op.create_index(
        "ix_live_slice_benchmarks_lookup",
        "live_slice_benchmarks",
        ["side", "market_product", "delivery_point_id", "availability_window"],
    )


def downgrade() -> None:
    op.drop_index("ix_live_slice_benchmarks_lookup", table_name="live_slice_benchmarks")
    op.drop_table("live_slice_benchmarks")

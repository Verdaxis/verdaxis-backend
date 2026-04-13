"""add executable orderbook and watchlist performance indexes

Revision ID: ix_2026_04_exec_perf
Revises: bm_2026_04_benchmarks, wl_2026_04_radar
Create Date: 2026-04-14
"""
from __future__ import annotations

from alembic import op


revision = "ix_2026_04_exec_perf"
down_revision = ("bm_2026_04_benchmarks", "wl_2026_04_radar")
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_index(
        "ix_orderbook_orders_active_slice_lookup",
        "orderbook_orders",
        ["side", "status", "product_id", "delivery_point_id", "availability_window", "created_at"],
    )
    op.create_index(
        "ix_watchlist_events_target_is_read_created",
        "watchlist_events",
        ["watchlist_target_id", "is_read", "created_at"],
    )


def downgrade() -> None:
    op.drop_index("ix_watchlist_events_target_is_read_created", table_name="watchlist_events")
    op.drop_index("ix_orderbook_orders_active_slice_lookup", table_name="orderbook_orders")

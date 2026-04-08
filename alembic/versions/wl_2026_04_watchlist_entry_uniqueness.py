"""enforce unique watchlist entries per product and delivery point

Revision ID: wl_2026_04_watchlist_entry_uniqueness
Revises: news_2026_03
Create Date: 2026-04-06
"""
from alembic import op
import sqlalchemy as sa


revision = "wl_2026_04_watchlist_entry_uniqueness"
down_revision = "news_2026_03"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_index(
        "uq_watchlist_entries_watchlist_product_delivery_point",
        "watchlist_entries",
        ["watchlist_id", "product_id", "delivery_point_id"],
        unique=True,
        sqlite_where=sa.text("delivery_point_id IS NOT NULL"),
        postgresql_where=sa.text("delivery_point_id IS NOT NULL"),
    )
    op.create_index(
        "uq_watchlist_entries_watchlist_product_no_delivery_point",
        "watchlist_entries",
        ["watchlist_id", "product_id"],
        unique=True,
        sqlite_where=sa.text("delivery_point_id IS NULL"),
        postgresql_where=sa.text("delivery_point_id IS NULL"),
    )


def downgrade() -> None:
    op.drop_index(
        "uq_watchlist_entries_watchlist_product_no_delivery_point",
        table_name="watchlist_entries",
    )
    op.drop_index(
        "uq_watchlist_entries_watchlist_product_delivery_point",
        table_name="watchlist_entries",
    )

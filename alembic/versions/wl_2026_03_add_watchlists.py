"""add watchlists and watchlist_entries tables

Revision ID: wl_2026_03
Revises: rfq_2026_03
Create Date: 2026-03-15
"""
from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects.postgresql import UUID

revision = "wl_2026_03"
down_revision = "rfq_2026_03"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "watchlists",
        sa.Column("id", UUID(as_uuid=True), nullable=False),
        sa.Column("user_id", UUID(as_uuid=True), nullable=False),
        sa.Column("name", sa.String(100), nullable=False),
        sa.Column(
            "created_at", sa.DateTime(timezone=True),
            server_default=sa.text("now()"), nullable=False,
        ),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"]),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index("ix_watchlists_user_id", "watchlists", ["user_id"])

    op.create_table(
        "watchlist_entries",
        sa.Column("id", UUID(as_uuid=True), nullable=False),
        sa.Column("watchlist_id", UUID(as_uuid=True), nullable=False),
        sa.Column("product_id", UUID(as_uuid=True), nullable=False),
        sa.Column("delivery_point_id", UUID(as_uuid=True), nullable=True),
        sa.Column(
            "created_at", sa.DateTime(timezone=True),
            server_default=sa.text("now()"), nullable=False,
        ),
        sa.ForeignKeyConstraint(["watchlist_id"], ["watchlists.id"]),
        sa.ForeignKeyConstraint(["product_id"], ["products.id"]),
        sa.ForeignKeyConstraint(["delivery_point_id"], ["delivery_points.id"]),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index("ix_watchlist_entries_watchlist_id", "watchlist_entries", ["watchlist_id"])
    op.create_index("ix_watchlist_entries_product_id", "watchlist_entries", ["product_id"])


def downgrade() -> None:
    op.drop_index("ix_watchlist_entries_product_id", table_name="watchlist_entries")
    op.drop_index("ix_watchlist_entries_watchlist_id", table_name="watchlist_entries")
    op.drop_table("watchlist_entries")
    op.drop_index("ix_watchlists_user_id", table_name="watchlists")
    op.drop_table("watchlists")

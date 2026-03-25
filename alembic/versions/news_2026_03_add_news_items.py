"""add news_items table

Revision ID: news_2026_03
Revises: wl_2026_03
Create Date: 2026-03-25
"""
from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects.postgresql import UUID

# revision identifiers, used by Alembic.
revision = "news_2026_03"
down_revision = "contracts_2026_03"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "news_items",
        sa.Column("id", UUID(as_uuid=True), primary_key=True),
        sa.Column("title", sa.String(500), nullable=False),
        sa.Column("summary", sa.Text(), nullable=True),
        sa.Column("source", sa.String(100), nullable=False),
        sa.Column("source_url", sa.String(500), nullable=False),
        sa.Column("url", sa.String(1000), nullable=False, unique=True),
        sa.Column("category", sa.String(50), nullable=False, server_default="markets"),
        sa.Column("relevance", sa.Integer(), nullable=False, server_default="3"),
        sa.Column("published_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("fetched_at", sa.DateTime(timezone=True), nullable=False),
    )
    op.create_index("ix_news_items_published_at", "news_items", ["published_at"])
    op.create_index("ix_news_items_category", "news_items", ["category"])


def downgrade() -> None:
    op.drop_index("ix_news_items_category", table_name="news_items")
    op.drop_index("ix_news_items_published_at", table_name="news_items")
    op.drop_table("news_items")

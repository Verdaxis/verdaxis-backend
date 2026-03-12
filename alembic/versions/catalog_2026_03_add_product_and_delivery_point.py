"""add product and delivery_point tables

Revision ID: catalog_2026_03
Revises: anon_trade_2026_03
Create Date: 2026-03-12
"""
from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects.postgresql import UUID


# revision identifiers, used by Alembic.
revision = "catalog_2026_03"
down_revision = "anon_trade_2026_03"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "products",
        sa.Column("id", UUID(as_uuid=True), primary_key=True),
        sa.Column("name", sa.String(), nullable=False, unique=True),
        sa.Column("fuel_type", sa.String(), nullable=False),
        sa.Column("fuel_grade", sa.String(), nullable=False),
        sa.Column("unit", sa.String(), server_default="MT"),
        sa.Column("min_lot_size", sa.Numeric(12, 2), server_default="100"),
        sa.Column("spec_description", sa.String(), nullable=True),
        sa.Column("is_active", sa.Boolean(), server_default="true"),
        sa.Column("created_at", sa.DateTime(timezone=True), server_default=sa.func.now()),
    )

    op.create_table(
        "delivery_points",
        sa.Column("id", UUID(as_uuid=True), primary_key=True),
        sa.Column("name", sa.String(), nullable=False, unique=True),
        sa.Column("region", sa.String(), nullable=False),
        sa.Column("timezone", sa.String(), nullable=True),
        sa.Column("is_active", sa.Boolean(), server_default="true"),
        sa.Column("created_at", sa.DateTime(timezone=True), server_default=sa.func.now()),
    )


def downgrade() -> None:
    op.drop_table("delivery_points")
    op.drop_table("products")

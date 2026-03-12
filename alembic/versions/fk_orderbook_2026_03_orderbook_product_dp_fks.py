"""migrate orderbook_orders from string enums to product/delivery_point FKs

Replaces fuel_type, fuel_grade, region string columns on orderbook_orders
with product_id (FK -> products.id, NOT NULL) and delivery_point_id
(FK -> delivery_points.id, nullable).

Deletes existing demo/test trades and orders since they use the old
string schema and no real trade data exists yet.

Revision ID: fk_orderbook_2026_03
Revises: catalog_2026_03
Create Date: 2026-03-12
"""
from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects.postgresql import UUID


# revision identifiers, used by Alembic.
revision = "fk_orderbook_2026_03"
down_revision = "catalog_2026_03"
branch_labels = None
depends_on = None


def upgrade() -> None:
    # 0. Delete existing demo/test data — trades reference orders via FK,
    #    so trades must be deleted first.
    op.execute("DELETE FROM trades")
    op.execute("DELETE FROM orderbook_orders")

    # 1. Add new FK columns (nullable initially for the ALTER sequence)
    op.add_column(
        "orderbook_orders",
        sa.Column("product_id", UUID(as_uuid=True), nullable=True),
    )
    op.add_column(
        "orderbook_orders",
        sa.Column("delivery_point_id", UUID(as_uuid=True), nullable=True),
    )

    # 2. Create foreign key constraints
    op.create_foreign_key(
        "fk_orderbook_orders_product_id",
        "orderbook_orders",
        "products",
        ["product_id"],
        ["id"],
    )
    op.create_foreign_key(
        "fk_orderbook_orders_delivery_point_id",
        "orderbook_orders",
        "delivery_points",
        ["delivery_point_id"],
        ["id"],
    )

    # 3. Drop old string columns
    op.drop_column("orderbook_orders", "fuel_type")
    op.drop_column("orderbook_orders", "fuel_grade")
    op.drop_column("orderbook_orders", "region")

    # 4. Make product_id NOT NULL (delivery_point_id stays nullable)
    op.alter_column(
        "orderbook_orders",
        "product_id",
        nullable=False,
    )


def downgrade() -> None:
    # 1. Make product_id nullable again so we can drop it
    op.alter_column(
        "orderbook_orders",
        "product_id",
        nullable=True,
    )

    # 2. Re-add old string columns
    op.add_column(
        "orderbook_orders",
        sa.Column("fuel_type", sa.String(), nullable=True),
    )
    op.add_column(
        "orderbook_orders",
        sa.Column("fuel_grade", sa.String(), nullable=True),
    )
    op.add_column(
        "orderbook_orders",
        sa.Column("region", sa.String(), nullable=True),
    )

    # 3. Drop FK constraints
    op.drop_constraint("fk_orderbook_orders_delivery_point_id", "orderbook_orders", type_="foreignkey")
    op.drop_constraint("fk_orderbook_orders_product_id", "orderbook_orders", type_="foreignkey")

    # 4. Drop the FK columns
    op.drop_column("orderbook_orders", "delivery_point_id")
    op.drop_column("orderbook_orders", "product_id")

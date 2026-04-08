"""add supplier listing metadata to orderbook and inventory

Revision ID: ob_2026_04_supplier_meta
Revises: catalog_2026_04_green_fuels
Create Date: 2026-04-08
"""

from alembic import op
import sqlalchemy as sa


revision = "ob_2026_04_supplier_meta"
down_revision = "catalog_2026_04_green_fuels"
branch_labels = None
depends_on = None


SUPPLIER_METADATA_COLUMNS = (
    sa.Column("certification_declared", sa.Boolean(), nullable=False, server_default=sa.text("false")),
    sa.Column("certification_scheme", sa.String(length=120), nullable=True),
    sa.Column("specification_standard", sa.String(length=120), nullable=True),
    sa.Column("msds_available", sa.Boolean(), nullable=False, server_default=sa.text("false")),
    sa.Column("carbon_intensity_method", sa.String(length=120), nullable=True),
    sa.Column("feedstock", sa.String(length=255), nullable=True),
    sa.Column("origin", sa.String(length=255), nullable=True),
    sa.Column("off_spec", sa.Boolean(), nullable=False, server_default=sa.text("false")),
    sa.Column("off_spec_notes", sa.Text(), nullable=True),
)


def upgrade() -> None:
    for column in SUPPLIER_METADATA_COLUMNS:
        op.add_column("orderbook_orders", column.copy())
    op.add_column(
        "inventory_items",
        sa.Column("certification_declared", sa.Boolean(), nullable=False, server_default=sa.text("false")),
    )
    op.add_column("inventory_items", sa.Column("certification_scheme", sa.String(length=120), nullable=True))
    op.add_column("inventory_items", sa.Column("specification_standard", sa.String(length=120), nullable=True))
    op.add_column(
        "inventory_items",
        sa.Column("msds_available", sa.Boolean(), nullable=False, server_default=sa.text("false")),
    )
    op.add_column("inventory_items", sa.Column("carbon_intensity_gco2_mj", sa.Numeric(8, 2), nullable=True))
    op.add_column("inventory_items", sa.Column("carbon_intensity_method", sa.String(length=120), nullable=True))
    op.add_column("inventory_items", sa.Column("feedstock", sa.String(length=255), nullable=True))
    op.add_column("inventory_items", sa.Column("origin", sa.String(length=255), nullable=True))
    op.add_column(
        "inventory_items",
        sa.Column("off_spec", sa.Boolean(), nullable=False, server_default=sa.text("false")),
    )
    op.add_column("inventory_items", sa.Column("off_spec_notes", sa.Text(), nullable=True))


def downgrade() -> None:
    op.drop_column("inventory_items", "off_spec_notes")
    op.drop_column("inventory_items", "off_spec")
    op.drop_column("inventory_items", "origin")
    op.drop_column("inventory_items", "feedstock")
    op.drop_column("inventory_items", "carbon_intensity_method")
    op.drop_column("inventory_items", "carbon_intensity_gco2_mj")
    op.drop_column("inventory_items", "msds_available")
    op.drop_column("inventory_items", "specification_standard")
    op.drop_column("inventory_items", "certification_scheme")
    op.drop_column("inventory_items", "certification_declared")

    op.drop_column("orderbook_orders", "off_spec_notes")
    op.drop_column("orderbook_orders", "off_spec")
    op.drop_column("orderbook_orders", "origin")
    op.drop_column("orderbook_orders", "feedstock")
    op.drop_column("orderbook_orders", "carbon_intensity_method")
    op.drop_column("orderbook_orders", "msds_available")
    op.drop_column("orderbook_orders", "specification_standard")
    op.drop_column("orderbook_orders", "certification_scheme")
    op.drop_column("orderbook_orders", "certification_declared")

"""Bind KYC/inventory provenance and preserve safe rollback width.

Revision ID: sec_20260720_fresh
Revises: sec_20260720_boundaries
"""

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects import postgresql


revision: str = "sec_20260720_fresh"
down_revision: Union[str, None] = "sec_20260720_boundaries"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.add_column(
        "users",
        sa.Column("kyc_organization_id", postgresql.UUID(as_uuid=True), nullable=True),
    )
    op.create_foreign_key(
        "fk_users_kyc_organization_id",
        "users",
        "organizations",
        ["kyc_organization_id"],
        ["id"],
        ondelete="SET NULL",
    )
    op.create_index("ix_users_kyc_organization_id", "users", ["kyc_organization_id"])

    op.add_column(
        "orderbook_orders",
        sa.Column("inventory_item_id", postgresql.UUID(as_uuid=True), nullable=True),
    )
    op.create_foreign_key(
        "fk_orderbook_orders_inventory_item_id",
        "orderbook_orders",
        "inventory_items",
        ["inventory_item_id"],
        ["id"],
        ondelete="SET NULL",
    )
    op.create_index(
        "ix_orderbook_orders_inventory_item_id", "orderbook_orders", ["inventory_item_id"]
    )


def downgrade() -> None:
    op.drop_index("ix_orderbook_orders_inventory_item_id", table_name="orderbook_orders")
    op.drop_constraint(
        "fk_orderbook_orders_inventory_item_id", "orderbook_orders", type_="foreignkey"
    )
    op.drop_column("orderbook_orders", "inventory_item_id")
    op.drop_index("ix_users_kyc_organization_id", table_name="users")
    op.drop_constraint("fk_users_kyc_organization_id", "users", type_="foreignkey")
    op.drop_column("users", "kyc_organization_id")
    # Deliberately retain inventory_items.fuel_type VARCHAR(20). Downgrading
    # application features does not require narrowing storage, and narrowing
    # would fail for valid post-upgrade values such as "Biomethane".

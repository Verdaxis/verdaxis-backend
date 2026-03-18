"""add advanced order types: OrderType enum, stop_price, linked_order_id, TRIGGERED status

Revision ID: s5_001_advanced_order_types
Revises: pw_reset_2026_03
Create Date: 2026-03-18

All enum columns use native_enum=False (VARCHAR storage) per project convention,
so no ALTER TYPE is needed — the status column is already VARCHAR and TRIGGERED is
a valid new string value with no schema change required.
"""
from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects import postgresql

# revision identifiers, used by Alembic.
revision = "s5_001_advanced_order_types"
down_revision = "pw_reset_2026_03"
branch_labels = None
depends_on = None


def upgrade() -> None:
    # Add order_type column (VARCHAR, native_enum=False convention, default LIMIT)
    op.add_column(
        "orderbook_orders",
        sa.Column(
            "order_type",
            sa.String(length=20),
            nullable=False,
            server_default="LIMIT",
        ),
    )

    # Add stop_price (required only for STOP / STOP_LIMIT order types)
    op.add_column(
        "orderbook_orders",
        sa.Column("stop_price", sa.Numeric(10, 2), nullable=True),
    )

    # Add linked_order_id self-referencing FK (for OCO pairs)
    op.add_column(
        "orderbook_orders",
        sa.Column(
            "linked_order_id",
            postgresql.UUID(as_uuid=True),
            nullable=True,
        ),
    )
    op.create_foreign_key(
        "fk_orderbook_orders_linked_order_id",
        "orderbook_orders",
        "orderbook_orders",
        ["linked_order_id"],
        ["id"],
        ondelete="SET NULL",
    )

    # No DDL needed for TRIGGERED status — status is VARCHAR (native_enum=False),
    # so inserting 'TRIGGERED' is valid without any schema change.


def downgrade() -> None:
    op.drop_constraint(
        "fk_orderbook_orders_linked_order_id",
        "orderbook_orders",
        type_="foreignkey",
    )
    op.drop_column("orderbook_orders", "linked_order_id")
    op.drop_column("orderbook_orders", "stop_price")
    op.drop_column("orderbook_orders", "order_type")

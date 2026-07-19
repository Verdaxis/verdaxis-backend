"""align inventory fuel storage with the supported catalog values"""

from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op


revision: str = "rh_20260720_runtime_metadata"
down_revision: Union[str, None] = "pa_20260715_analytics_facts"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.alter_column(
        "inventory_items",
        "fuel_type",
        existing_type=sa.String(length=8),
        type_=sa.String(length=16),
    )
    op.alter_column(
        "orderbook_orders",
        "availability_window",
        existing_type=sa.String(length=50),
        type_=sa.String(length=16),
        existing_nullable=True,
        nullable=False,
        existing_server_default=sa.text("'Spot'::character varying"),
        server_default=None,
    )
    op.alter_column(
        "orderbook_orders", "is_verdaxis_verified",
        existing_type=sa.Boolean(), existing_nullable=True, nullable=False,
        existing_server_default=sa.text("false"), server_default=None,
    )
    op.alter_column(
        "orderbook_orders", "status",
        existing_type=sa.String(length=20), existing_nullable=True, nullable=False,
        existing_server_default=sa.text("'OPEN'::character varying"), server_default=None,
    )
    op.alter_column(
        "orderbook_orders", "created_at",
        existing_type=sa.DateTime(timezone=True), existing_nullable=True, nullable=False,
        existing_server_default=sa.text("now()"), server_default=None,
    )
    op.alter_column(
        "orderbook_orders", "updated_at",
        existing_type=sa.DateTime(timezone=True), existing_nullable=True, nullable=False,
        existing_server_default=sa.text("now()"), server_default=None,
    )
    op.drop_column("orderbook_orders", "delivery_window_start")
    op.drop_column("orderbook_orders", "delivery_window_end")


def downgrade() -> None:
    op.add_column("orderbook_orders", sa.Column("delivery_window_start", sa.Date(), nullable=True))
    op.add_column("orderbook_orders", sa.Column("delivery_window_end", sa.Date(), nullable=True))
    op.alter_column("orderbook_orders", "updated_at", existing_type=sa.DateTime(timezone=True), nullable=True, server_default=sa.text("now()"))
    op.alter_column("orderbook_orders", "created_at", existing_type=sa.DateTime(timezone=True), nullable=True, server_default=sa.text("now()"))
    op.alter_column("orderbook_orders", "status", existing_type=sa.String(length=20), nullable=True, server_default=sa.text("'OPEN'"))
    op.alter_column("orderbook_orders", "is_verdaxis_verified", existing_type=sa.Boolean(), nullable=True, server_default=sa.text("false"))
    op.alter_column("orderbook_orders", "availability_window", existing_type=sa.String(length=16), type_=sa.String(length=50), nullable=True, server_default=sa.text("'Spot'"))
    op.alter_column(
        "inventory_items",
        "fuel_type",
        existing_type=sa.String(length=16),
        type_=sa.String(length=8),
    )

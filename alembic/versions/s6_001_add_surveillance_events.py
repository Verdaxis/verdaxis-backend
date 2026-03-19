"""add surveillance_events table

Revision ID: s6_001_surveillance_events
Revises: s5_002_add_order_audit_logs
Create Date: 2026-03-19

Creates the surveillance_events table for market abuse detection.
All enum columns use native_enum=False (VARCHAR storage) per project convention.
"""
from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects import postgresql

# revision identifiers, used by Alembic.
revision = "s6_001_surveillance_events"
down_revision = "s5_002_add_order_audit_logs"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "surveillance_events",
        sa.Column(
            "id",
            postgresql.UUID(as_uuid=True),
            primary_key=True,
            nullable=False,
        ),
        sa.Column("type", sa.String(length=30), nullable=False),
        sa.Column("severity", sa.String(length=10), nullable=False),
        sa.Column("status", sa.String(length=10), nullable=False, server_default="OPEN"),
        sa.Column("participants", postgresql.JSON(astext_type=sa.Text()), nullable=True),
        sa.Column("related_orders", postgresql.JSON(astext_type=sa.Text()), nullable=True),
        sa.Column("related_trades", postgresql.JSON(astext_type=sa.Text()), nullable=True),
        sa.Column("description", sa.String(length=1000), nullable=True),
        sa.Column("notes", sa.String(length=2000), nullable=True),
        sa.Column("window_start", sa.DateTime(timezone=True), nullable=True),
        sa.Column("window_end", sa.DateTime(timezone=True), nullable=True),
        sa.Column("auto_detected", sa.Boolean(), nullable=False, server_default=sa.true()),
        sa.Column(
            "created_at",
            sa.DateTime(timezone=True),
            nullable=False,
            server_default=sa.func.now(),
        ),
        sa.Column(
            "updated_at",
            sa.DateTime(timezone=True),
            nullable=False,
            server_default=sa.func.now(),
        ),
    )


def downgrade() -> None:
    op.drop_table("surveillance_events")

"""add order_audit_logs table for per-field amendment history

Revision ID: s5_002_add_order_audit_logs
Revises: s5_001_advanced_order_types
Create Date: 2026-03-19

"""
from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects import postgresql

# revision identifiers, used by Alembic.
revision = "s5_002_add_order_audit_logs"
down_revision = "s5_001_advanced_order_types"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "order_audit_logs",
        sa.Column(
            "id",
            postgresql.UUID(as_uuid=True),
            primary_key=True,
            nullable=False,
        ),
        sa.Column(
            "order_id",
            postgresql.UUID(as_uuid=True),
            sa.ForeignKey("orderbook_orders.id"),
            nullable=False,
            index=True,
        ),
        sa.Column("field_changed", sa.String(length=50), nullable=False),
        sa.Column("old_value", sa.String(length=500), nullable=False),
        sa.Column("new_value", sa.String(length=500), nullable=False),
        sa.Column(
            "changed_by",
            postgresql.UUID(as_uuid=True),
            sa.ForeignKey("users.id"),
            nullable=False,
        ),
        sa.Column(
            "changed_at",
            sa.DateTime(timezone=True),
            nullable=False,
            index=True,
        ),
    )


def downgrade() -> None:
    op.drop_table("order_audit_logs")

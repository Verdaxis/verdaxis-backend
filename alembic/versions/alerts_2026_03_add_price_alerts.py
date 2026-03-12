"""add price_alerts table

Revision ID: alerts_2026_03
Revises: fk_orderbook_2026_03
Create Date: 2026-03-12
"""
from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects.postgresql import UUID


# revision identifiers, used by Alembic.
revision = "alerts_2026_03"
down_revision = "sub_2026_03"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "price_alerts",
        sa.Column("id", UUID(as_uuid=True), primary_key=True),
        sa.Column(
            "org_id",
            UUID(as_uuid=True),
            sa.ForeignKey("organizations.id", ondelete="CASCADE"),
            nullable=False,
        ),
        sa.Column(
            "product_id",
            UUID(as_uuid=True),
            sa.ForeignKey("products.id", ondelete="CASCADE"),
            nullable=False,
        ),
        sa.Column(
            "delivery_point_id",
            UUID(as_uuid=True),
            sa.ForeignKey("delivery_points.id", ondelete="SET NULL"),
            nullable=True,
        ),
        sa.Column("direction", sa.String(), nullable=False),
        sa.Column("threshold_usd", sa.Numeric(10, 2), nullable=False),
        sa.Column("is_active", sa.Boolean(), server_default="true", nullable=False),
        sa.Column("triggered_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), server_default=sa.func.now()),
    )
    # Index for the hot query: active alerts by product
    op.create_index(
        "ix_price_alerts_product_active",
        "price_alerts",
        ["product_id", "is_active"],
    )
    # Index for listing alerts by org
    op.create_index(
        "ix_price_alerts_org_id",
        "price_alerts",
        ["org_id"],
    )


def downgrade() -> None:
    op.drop_index("ix_price_alerts_org_id", table_name="price_alerts")
    op.drop_index("ix_price_alerts_product_active", table_name="price_alerts")
    op.drop_table("price_alerts")

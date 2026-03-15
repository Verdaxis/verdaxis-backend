"""add rfq and rfq_quotes tables

Revision ID: rfq_2026_03
Revises: ref_2026_03
Create Date: 2026-03-15
"""
from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects.postgresql import UUID

revision = "rfq_2026_03"
down_revision = "ref_2026_03"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "rfqs",
        sa.Column("id", UUID(as_uuid=True), nullable=False),
        sa.Column("buyer_org_id", UUID(as_uuid=True), nullable=False),
        sa.Column("product_id", UUID(as_uuid=True), nullable=False),
        sa.Column("delivery_point_id", UUID(as_uuid=True), nullable=True),
        sa.Column("quantity_mt", sa.Numeric(12, 2), nullable=False),
        sa.Column("target_price_per_mt", sa.Numeric(10, 2), nullable=True),
        sa.Column("availability_window", sa.String(), nullable=False, server_default="Spot"),
        sa.Column("notes", sa.Text(), nullable=True),
        sa.Column("is_anonymous", sa.Boolean(), nullable=False, server_default="false"),
        sa.Column("status", sa.String(), nullable=False, server_default="OPEN"),
        sa.Column("expires_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("created_at", sa.DateTime(timezone=True), server_default=sa.text("now()"), nullable=False),
        sa.ForeignKeyConstraint(["buyer_org_id"], ["organizations.id"]),
        sa.ForeignKeyConstraint(["product_id"], ["products.id"]),
        sa.ForeignKeyConstraint(["delivery_point_id"], ["delivery_points.id"]),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index("ix_rfqs_buyer_org_id", "rfqs", ["buyer_org_id"])
    op.create_index("ix_rfqs_status", "rfqs", ["status"])

    op.create_table(
        "rfq_quotes",
        sa.Column("id", UUID(as_uuid=True), nullable=False),
        sa.Column("rfq_id", UUID(as_uuid=True), nullable=False),
        sa.Column("seller_org_id", UUID(as_uuid=True), nullable=False),
        sa.Column("price_per_mt_usd", sa.Numeric(10, 2), nullable=False),
        sa.Column("notes", sa.Text(), nullable=True),
        sa.Column("status", sa.String(), nullable=False, server_default="PENDING"),
        sa.Column("created_at", sa.DateTime(timezone=True), server_default=sa.text("now()"), nullable=False),
        sa.ForeignKeyConstraint(["rfq_id"], ["rfqs.id"]),
        sa.ForeignKeyConstraint(["seller_org_id"], ["organizations.id"]),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index("ix_rfq_quotes_rfq_id", "rfq_quotes", ["rfq_id"])


def downgrade() -> None:
    op.drop_index("ix_rfq_quotes_rfq_id", table_name="rfq_quotes")
    op.drop_table("rfq_quotes")
    op.drop_index("ix_rfqs_status", table_name="rfqs")
    op.drop_index("ix_rfqs_buyer_org_id", table_name="rfqs")
    op.drop_table("rfqs")

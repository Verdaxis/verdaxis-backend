"""add negotiations and negotiation_rounds tables

Revision ID: neg_2026_04
Revises: news_2026_03
Create Date: 2026-04-06
"""
from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects.postgresql import UUID

revision = "neg_2026_04"
down_revision = "news_2026_03"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "negotiations",
        sa.Column("id", UUID(as_uuid=True), primary_key=True),
        sa.Column("bid_order_id", UUID(as_uuid=True),
                  sa.ForeignKey("orderbook_orders.id", ondelete="SET NULL"), nullable=True),
        sa.Column("ask_order_id", UUID(as_uuid=True),
                  sa.ForeignKey("orderbook_orders.id", ondelete="SET NULL"), nullable=True),
        sa.Column("initiator_org_id", UUID(as_uuid=True),
                  sa.ForeignKey("organizations.id"), nullable=False),
        sa.Column("counterparty_org_id", UUID(as_uuid=True),
                  sa.ForeignKey("organizations.id"), nullable=False),
        sa.Column("product_id", UUID(as_uuid=True),
                  sa.ForeignKey("products.id"), nullable=False),
        sa.Column("quantity_mt", sa.Numeric(12, 2), nullable=False),
        sa.Column("current_price", sa.Numeric(10, 2), nullable=False),
        sa.Column("status", sa.String, nullable=False, server_default="OPEN"),
        sa.Column("last_actor_org_id", UUID(as_uuid=True),
                  sa.ForeignKey("organizations.id"), nullable=False),
        sa.Column("trade_id", UUID(as_uuid=True),
                  sa.ForeignKey("trades.id", ondelete="SET NULL"), nullable=True),
        sa.Column("expires_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("created_at", sa.DateTime(timezone=True),
                  server_default=sa.func.now(), nullable=False),
    )

    op.create_table(
        "negotiation_rounds",
        sa.Column("id", UUID(as_uuid=True), primary_key=True),
        sa.Column("negotiation_id", UUID(as_uuid=True),
                  sa.ForeignKey("negotiations.id"), nullable=False),
        sa.Column("round_number", sa.Integer, nullable=False),
        sa.Column("proposer_org_id", UUID(as_uuid=True),
                  sa.ForeignKey("organizations.id"), nullable=False),
        sa.Column("proposed_price", sa.Numeric(10, 2), nullable=False),
        sa.Column("notes", sa.Text, nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True),
                  server_default=sa.func.now(), nullable=False),
    )

    # Index for fast lookup of negotiations by party
    op.create_index("ix_negotiations_initiator_org", "negotiations", ["initiator_org_id"])
    op.create_index("ix_negotiations_counterparty_org", "negotiations", ["counterparty_org_id"])
    op.create_index("ix_negotiations_status", "negotiations", ["status"])
    op.create_index("ix_negotiation_rounds_negotiation", "negotiation_rounds", ["negotiation_id"])


def downgrade() -> None:
    op.drop_index("ix_negotiation_rounds_negotiation")
    op.drop_index("ix_negotiations_status")
    op.drop_index("ix_negotiations_counterparty_org")
    op.drop_index("ix_negotiations_initiator_org")
    op.drop_table("negotiation_rounds")
    op.drop_table("negotiations")

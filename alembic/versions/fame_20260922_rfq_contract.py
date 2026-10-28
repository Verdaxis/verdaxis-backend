"""Add declared UCOME contract terms and versioned, expiring RFQ quotes.

Revision ID: fame_20260922_rfq_contract
Revises: fame_20260922_catalog
"""
import sqlalchemy as sa
from alembic import op


revision = "fame_20260922_rfq_contract"
down_revision = "fame_20260922_catalog"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("rfqs", sa.Column("contract_terms", sa.JSON(), nullable=True))
    op.add_column("rfq_quotes", sa.Column("offer_terms", sa.JSON(), nullable=True))
    op.add_column("rfq_quotes", sa.Column("expires_at", sa.DateTime(timezone=True), nullable=True))
    op.add_column("rfq_quotes", sa.Column("revision", sa.Integer(), nullable=False, server_default="1"))
    op.create_check_constraint("ck_rfq_quotes_revision", "rfq_quotes", "revision >= 1")


def downgrade() -> None:
    connection = op.get_bind()
    connection.execute(sa.text("LOCK TABLE rfqs, rfq_quotes IN ACCESS EXCLUSIVE MODE NOWAIT"))
    has_terms = connection.execute(sa.text(
        "SELECT EXISTS (SELECT 1 FROM rfqs WHERE contract_terms IS NOT NULL) "
        "OR EXISTS (SELECT 1 FROM rfq_quotes WHERE offer_terms IS NOT NULL "
        "OR expires_at IS NOT NULL OR revision <> 1)"
    )).scalar_one()
    if has_terms:
        raise RuntimeError("Cannot remove RFQ contract fields while contract or quote history exists")
    op.drop_constraint("ck_rfq_quotes_revision", "rfq_quotes", type_="check")
    op.drop_column("rfq_quotes", "revision")
    op.drop_column("rfq_quotes", "expires_at")
    op.drop_column("rfq_quotes", "offer_terms")
    op.drop_column("rfqs", "contract_terms")

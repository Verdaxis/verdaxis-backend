"""Add declared UCOME supplier offers and immutable RFQ source references.

Revision ID: fame_20260922_supplier_offers
Revises: fame_20260922_rfq_contract

Offers are RFQ discovery records. This revision does not relax executable
order, assisted-order, negotiation, or trade snapshot constraints.
"""
from alembic import op
import sqlalchemy as sa

revision = "fame_20260922_supplier_offers"
down_revision = "fame_20260922_rfq_contract"
branch_labels = None
depends_on = None

_CATALOG = (
    "product_id = 'e561e43f-d9b2-598e-981c-f1d28d515ddc' "
    "AND delivery_point_id = '73835e92-820e-584b-8280-bb61c63aa28e'"
)
_NUMERIC_VALUES = (
    "quantity_mt >= 1 AND quantity_mt <= 100000 "
    "AND quantity_mt * 100 = trunc(quantity_mt * 100) "
    "AND min_fill_mt >= 1 AND min_fill_mt <= quantity_mt "
    "AND min_fill_mt * 100 = trunc(min_fill_mt * 100) "
    "AND price_per_mt_usd >= 0.01 AND price_per_mt_usd <= 1000000 "
    "AND price_per_mt_usd * 100 = trunc(price_per_mt_usd * 100)"
)
_IDEMPOTENCY = (
    "(idempotency_key IS NULL AND idempotency_request_hash IS NULL) OR "
    "(idempotency_key IS NOT NULL AND idempotency_request_hash IS NOT NULL)"
)
_RFQ_SOURCE = (
    "(source_offer_id IS NULL AND target_supplier_org_id IS NULL AND source_offer_snapshot IS NULL) OR "
    "(source_offer_id IS NOT NULL AND target_supplier_org_id IS NOT NULL AND source_offer_snapshot IS NOT NULL)"
)


def upgrade() -> None:
    op.create_table(
        "supplier_offers",
        sa.Column("id", sa.UUID(), primary_key=True, nullable=False),
        sa.Column("supplier_org_id", sa.UUID(), sa.ForeignKey("organizations.id"), nullable=False),
        sa.Column("supplier_user_id", sa.UUID(), sa.ForeignKey("users.id"), nullable=False),
        sa.Column("product_id", sa.UUID(), sa.ForeignKey("products.id"), nullable=False),
        sa.Column("delivery_point_id", sa.UUID(), sa.ForeignKey("delivery_points.id"), nullable=False),
        # Unbounded NUMERIC preserves invalid scale for CHECK rejection; a
        # precision/scale typmod would silently round before validation.
        sa.Column("quantity_mt", sa.Numeric(), nullable=False),
        sa.Column("min_fill_mt", sa.Numeric(), nullable=False),
        sa.Column("price_per_mt_usd", sa.Numeric(), nullable=False),
        sa.Column("availability_window", sa.String(16), nullable=False),
        sa.Column("listing_terms", sa.JSON(), nullable=False),
        sa.Column("status", sa.String(16), nullable=False, server_default="OPEN"),
        sa.Column("revision", sa.Integer(), nullable=False, server_default="1"),
        sa.Column("expires_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("idempotency_key", sa.String(255), nullable=True),
        sa.Column("idempotency_request_hash", sa.String(64), nullable=True),
        sa.UniqueConstraint("supplier_org_id", "idempotency_key", name="uq_supplier_offers_org_idempotency"),
        sa.CheckConstraint(_CATALOG, name="ck_supplier_offers_catalog"),
        sa.CheckConstraint(_NUMERIC_VALUES, name="ck_supplier_offers_numeric_values"),
        sa.CheckConstraint("status IN ('OPEN', 'WITHDRAWN')", name="ck_supplier_offers_status"),
        sa.CheckConstraint("revision >= 1", name="ck_supplier_offers_revision"),
        sa.CheckConstraint(_IDEMPOTENCY, name="ck_supplier_offers_idempotency"),
    )
    op.create_index("ix_supplier_offers_supplier_org_id", "supplier_offers", ["supplier_org_id"])
    op.create_index(
        "ix_supplier_offers_public", "supplier_offers",
        ["status", "product_id", "delivery_point_id", "expires_at"],
    )
    op.add_column("rfqs", sa.Column("source_offer_id", sa.UUID(), nullable=True))
    op.add_column("rfqs", sa.Column("target_supplier_org_id", sa.UUID(), nullable=True))
    op.add_column("rfqs", sa.Column("source_offer_snapshot", sa.JSON(), nullable=True))
    op.create_foreign_key(
        "fk_rfqs_source_offer_id", "rfqs", "supplier_offers", ["source_offer_id"], ["id"],
    )
    op.create_foreign_key(
        "fk_rfqs_target_supplier_org_id", "rfqs", "organizations", ["target_supplier_org_id"], ["id"],
    )
    op.create_check_constraint("ck_rfqs_source_offer", "rfqs", _RFQ_SOURCE)


def downgrade() -> None:
    connection = op.get_bind()
    connection.execute(sa.text("LOCK TABLE supplier_offers, rfqs IN ACCESS EXCLUSIVE MODE NOWAIT"))
    has_history = connection.execute(sa.text(
        "SELECT EXISTS (SELECT 1 FROM supplier_offers) "
        "OR EXISTS (SELECT 1 FROM rfqs WHERE source_offer_id IS NOT NULL "
        "OR target_supplier_org_id IS NOT NULL OR source_offer_snapshot IS NOT NULL)"
    )).scalar_one()
    if has_history:
        raise RuntimeError("Cannot remove supplier offers while offer or targeted RFQ history exists")
    op.drop_constraint("ck_rfqs_source_offer", "rfqs", type_="check")
    op.drop_constraint("fk_rfqs_source_offer_id", "rfqs", type_="foreignkey")
    op.drop_constraint("fk_rfqs_target_supplier_org_id", "rfqs", type_="foreignkey")
    op.drop_column("rfqs", "source_offer_snapshot")
    op.drop_column("rfqs", "target_supplier_org_id")
    op.drop_column("rfqs", "source_offer_id")
    op.drop_index("ix_supplier_offers_public", table_name="supplier_offers")
    op.drop_index("ix_supplier_offers_supplier_org_id", table_name="supplier_offers")
    op.drop_table("supplier_offers")

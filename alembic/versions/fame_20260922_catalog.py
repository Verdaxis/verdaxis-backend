"""Add the Singapore UCOME B100 RFQ catalog lane without executable orders.

Revision ID: fame_20260922_catalog
Revises: fee_20260912_seller_per_mt

Catalog identity and policy are frozen here. No application imports, demo
liquidity, price evidence, or historical alcohol records are changed.
"""
from alembic import op
import sqlalchemy as sa

revision = "fame_20260922_catalog"
down_revision = "fee_20260912_seller_per_mt"
branch_labels = None
depends_on = None

_PRODUCT_ID = "e561e43f-d9b2-598e-981c-f1d28d515ddc"
_SINGAPORE_ID = "73835e92-820e-584b-8280-bb61c63aa28e"
_EXECUTION_CHECKS = (
    ("orderbook_orders", "ck_orderbook_orders_execution_product"),
    ("negotiations", "ck_negotiations_execution_product"),
    ("market_support_authorizations", "ck_market_support_auth_execution_product"),
)


def upgrade() -> None:
    op.execute(sa.text(f"""
        DO $$ BEGIN
            IF EXISTS (SELECT 1 FROM products WHERE id = '{_PRODUCT_ID}'
                AND (name <> 'UCOME B100' OR fuel_type <> 'FAME' OR fuel_grade <> 'UCOME')) THEN
                RAISE EXCEPTION 'UCOME B100 catalog identity conflicts with an existing product';
            END IF;
        END $$;
    """))
    op.execute(sa.text(f"""
        INSERT INTO products
            (id, name, fuel_type, fuel_grade, unit, min_lot_size, spec_description, is_active)
        VALUES ('{_PRODUCT_ID}', 'UCOME B100', 'FAME', 'UCOME', 'MT', 1,
            'Neat B100 used cooking oil methyl ester for wholesale Singapore RFQs. '
            '1 MT is the platform input minimum; contract minimum fill and quality terms are negotiated.', true)
        ON CONFLICT (id) DO UPDATE SET is_active = true,
            unit = EXCLUDED.unit, min_lot_size = EXCLUDED.min_lot_size,
            spec_description = EXCLUDED.spec_description
    """))
    for table, name in _EXECUTION_CHECKS:
        op.create_check_constraint(name, table, f"product_id <> '{_PRODUCT_ID}'")
    op.create_check_constraint(
        "ck_rfqs_fame_delivery_lane", "rfqs",
        f"product_id <> '{_PRODUCT_ID}' OR "
        f"(delivery_point_id IS NOT NULL AND delivery_point_id = '{_SINGAPORE_ID}')",
    )
    # The pre-existing four-product trade snapshot constraint and trigger stay
    # unchanged: an RFQ-only contract must not create an executable trade.


def downgrade() -> None:
    op.drop_constraint("ck_rfqs_fame_delivery_lane", "rfqs", type_="check")
    for table, name in reversed(_EXECUTION_CHECKS):
        op.drop_constraint(name, table, type_="check")
    # Keep foreign keys and any recorded RFQs intact on a reviewed rollback.
    op.execute(sa.text(f"UPDATE products SET is_active = false WHERE id = '{_PRODUCT_ID}'"))

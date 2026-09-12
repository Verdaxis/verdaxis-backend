"""add immutable seller per-tonne fee snapshots

Revision ID: fee_20260912_seller_per_mt
Revises: oa_20260910_auto_real_orgs
Create Date: 2026-09-12
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op


revision = "fee_20260912_seller_per_mt"
down_revision = "oa_20260910_auto_real_orgs"
branch_labels = None
depends_on = None


_LEGACY_TRADE_NUMERIC_PARTS = (
    "quantity_mt IS NOT NULL AND quantity_mt >= 0.01 AND quantity_mt <= 100000.00 AND quantity_mt * 100 = trunc(quantity_mt * 100)",
    "price_per_mt_usd IS NOT NULL AND price_per_mt_usd >= 0.01 AND price_per_mt_usd <= 1000000.00 AND price_per_mt_usd * 100 = trunc(price_per_mt_usd * 100)",
    "(final_quantity_mt IS NULL OR (final_quantity_mt IS NOT NULL AND final_quantity_mt >= 0.01 AND final_quantity_mt <= 100000.00 AND final_quantity_mt * 100 = trunc(final_quantity_mt * 100)))",
    "(final_price_per_mt IS NULL OR (final_price_per_mt IS NOT NULL AND final_price_per_mt >= 0.01 AND final_price_per_mt <= 1000000.00 AND final_price_per_mt * 100 = trunc(final_price_per_mt * 100)))",
    "(final_total_usd IS NULL OR (final_total_usd IS NOT NULL AND final_total_usd >= 0 AND final_total_usd <= 100000000000.00 AND final_total_usd * 100 = trunc(final_total_usd * 100)))",
    "commission_rate_pct IS NOT NULL AND commission_rate_pct >= 0 AND commission_rate_pct <= 100 AND commission_rate_pct * 1000 = trunc(commission_rate_pct * 1000)",
    "(commission_amount_usd IS NULL OR (commission_amount_usd IS NOT NULL AND commission_amount_usd >= 0 AND commission_amount_usd <= 100000000000.00 AND commission_amount_usd * 100 = trunc(commission_amount_usd * 100)))",
)
_TRADE_NUMERIC_VALUES = " AND ".join(
    (
        *_LEGACY_TRADE_NUMERIC_PARTS,
        "(commission_fee_per_mt_usd IS NULL OR (commission_fee_per_mt_usd IS NOT NULL AND commission_fee_per_mt_usd >= 0 AND commission_fee_per_mt_usd <= 100000.00 AND commission_fee_per_mt_usd * 100 = trunc(commission_fee_per_mt_usd * 100)))",
    )
)

_TRADE_COMMISSION_SNAPSHOT = (
    "(commission_fee_per_mt_usd IS NULL AND commission_plan IS NULL) OR "
    "(commission_fee_per_mt_usd IS NOT NULL AND commission_rate_pct = 0 "
    "AND commission_plan IS NOT NULL "
    "AND commission_plan IN ('free','standard','enterprise'))"
)

_SUBSCRIPTION_RATE = (
    "seller_fee_per_mt_usd IS NULL OR "
    "(seller_fee_per_mt_usd >= 0 AND seller_fee_per_mt_usd <= 100000 "
    "AND seller_fee_per_mt_usd * 100 = trunc(seller_fee_per_mt_usd * 100))"
)


def upgrade() -> None:
    op.add_column(
        "subscriptions",
        sa.Column("seller_fee_per_mt_usd", sa.Numeric(), nullable=True),
    )
    op.create_check_constraint(
        "ck_subscriptions_seller_fee_per_mt",
        "subscriptions",
        _SUBSCRIPTION_RATE,
    )

    op.add_column(
        "trades",
        sa.Column("commission_fee_per_mt_usd", sa.Numeric(), nullable=True),
    )
    op.add_column(
        "trades",
        sa.Column("commission_plan", sa.String(length=20), nullable=True),
    )
    op.drop_constraint("ck_trades_numeric_values", "trades", type_="check")
    op.create_check_constraint(
        "ck_trades_numeric_values", "trades", _TRADE_NUMERIC_VALUES
    )
    op.create_check_constraint(
        "ck_trades_commission_snapshot", "trades", _TRADE_COMMISSION_SNAPSHOT
    )
    op.execute(
        sa.text(
            """
            CREATE FUNCTION verdaxis_immutable_trade_fee_snapshot()
            RETURNS trigger LANGUAGE plpgsql AS $$
            BEGIN
                IF OLD.commission_fee_per_mt_usd IS DISTINCT FROM NEW.commission_fee_per_mt_usd
                    OR OLD.commission_plan IS DISTINCT FROM NEW.commission_plan
                    OR (
                        OLD.commission_fee_per_mt_usd IS NOT NULL
                        AND OLD.commission_rate_pct IS DISTINCT FROM NEW.commission_rate_pct
                    )
                THEN
                    RAISE EXCEPTION 'trade fee snapshot is immutable';
                END IF;
                RETURN NEW;
            END;
            $$
            """
        )
    )
    op.execute(
        sa.text(
            """
            CREATE TRIGGER trades_fee_snapshot_guard
            BEFORE UPDATE OF commission_fee_per_mt_usd, commission_plan, commission_rate_pct
            ON trades FOR EACH ROW
            EXECUTE FUNCTION verdaxis_immutable_trade_fee_snapshot()
            """
        )
    )


def downgrade() -> None:
    connection = op.get_bind()
    # Runtime paths do not share one lock order: new matching reads a
    # subscription before writing a trade, while idempotent manual creation
    # reads trades before resolving the subscription. Never wait between these
    # locks. If either table is active, abort the downgrade and let runtime work
    # finish without making this migration a deadlock participant.
    connection.execute(
        sa.text(
            "LOCK TABLE subscriptions, trades IN ACCESS EXCLUSIVE MODE NOWAIT"
        )
    )
    fee_snapshot_count = connection.execute(
        sa.text(
            "SELECT count(*) FROM trades "
            "WHERE commission_fee_per_mt_usd IS NOT NULL OR commission_plan IS NOT NULL"
        )
    ).scalar_one()
    negotiated_rate_count = connection.execute(
        sa.text(
            "SELECT count(*) FROM subscriptions "
            "WHERE seller_fee_per_mt_usd IS NOT NULL"
        )
    ).scalar_one()
    if fee_snapshot_count or negotiated_rate_count:
        raise RuntimeError(
            "cannot downgrade seller per-tonne fees while trade fee snapshots "
            "or negotiated subscription rates exist"
        )

    op.execute(sa.text("DROP TRIGGER trades_fee_snapshot_guard ON trades"))
    op.execute(sa.text("DROP FUNCTION verdaxis_immutable_trade_fee_snapshot()"))
    op.drop_constraint("ck_trades_commission_snapshot", "trades", type_="check")
    op.drop_constraint("ck_trades_numeric_values", "trades", type_="check")
    op.create_check_constraint(
        "ck_trades_numeric_values",
        "trades",
        " AND ".join(_LEGACY_TRADE_NUMERIC_PARTS),
    )
    op.drop_column("trades", "commission_plan")
    op.drop_column("trades", "commission_fee_per_mt_usd")
    op.drop_constraint(
        "ck_subscriptions_seller_fee_per_mt", "subscriptions", type_="check"
    )
    op.drop_column("subscriptions", "seller_fee_per_mt_usd")

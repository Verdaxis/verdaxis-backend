"""Align runtime-owned metadata with the live prod/staging schemas.

The migration is intentionally non-destructive: delivery-window columns are
still consumed by ``scripts/seed_realistic_orderbook.py`` and remain part of
the orderbook model. Existing nullable producer/audit/match fields are
backfilled before their ORM-required NOT NULL constraints are applied.
"""

from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op


revision: str = "rh_20260720_runtime_metadata"
down_revision: Union[str, None] = "pa_20260715_analytics_facts"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def _backfill_required_values() -> None:
    op.execute(
        sa.text(
            "UPDATE orderbook_orders SET availability_window = 'SPOT' "
            "WHERE availability_window IS NULL"
        )
    )
    op.execute(
        sa.text(
            "UPDATE orderbook_orders SET is_verdaxis_verified = false "
            "WHERE is_verdaxis_verified IS NULL"
        )
    )
    op.execute(
        sa.text("UPDATE orderbook_orders SET status = 'OPEN' WHERE status IS NULL")
    )
    op.execute(
        sa.text("UPDATE orderbook_orders SET created_at = now() WHERE created_at IS NULL")
    )
    op.execute(
        sa.text("UPDATE orderbook_orders SET updated_at = now() WHERE updated_at IS NULL")
    )
    op.execute(sa.text("UPDATE trades SET status = 'PENDING_CONFIRMATION' WHERE status IS NULL"))
    op.execute(sa.text("UPDATE trades SET commission_rate_pct = 0.5 WHERE commission_rate_pct IS NULL"))
    op.execute(sa.text("UPDATE trades SET created_at = now() WHERE created_at IS NULL"))
    op.execute(sa.text("UPDATE audit_logs SET timestamp = now() WHERE timestamp IS NULL"))
    op.execute(sa.text("UPDATE producer_projects SET status = 'ANNOUNCED' WHERE status IS NULL"))
    op.execute(sa.text("UPDATE producer_projects SET created_at = now() WHERE created_at IS NULL"))
    op.execute(sa.text("UPDATE producer_projects SET updated_at = now() WHERE updated_at IS NULL"))
    op.execute(sa.text("UPDATE match_suggestions SET match_reasons = '[]'::json WHERE match_reasons IS NULL"))
    op.execute(sa.text("UPDATE match_suggestions SET status = 'SUGGESTED' WHERE status IS NULL"))
    op.execute(sa.text("UPDATE match_suggestions SET created_at = now() WHERE created_at IS NULL"))


def _assert_no_null_commission_legacy_keys() -> None:
    null_count = op.get_bind().scalar(
        sa.text("SELECT count(*) FROM commissions WHERE match_id IS NULL")
    )
    if null_count:
        raise RuntimeError(
            "cannot tighten commissions.match_id: existing rows lack the required legacy match"
        )


def upgrade() -> None:
    # Preflight/backfill is deliberately before every NOT NULL alteration.
    _backfill_required_values()
    _assert_no_null_commission_legacy_keys()

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
        nullable=False,
        existing_nullable=True,
        server_default=sa.text("'SPOT'"),
        existing_server_default=sa.text("'SPOT'::character varying"),
    )
    op.alter_column(
        "orderbook_orders",
        "is_verdaxis_verified",
        existing_type=sa.Boolean(),
        nullable=False,
        existing_nullable=True,
        server_default=sa.text("false"),
        existing_server_default=sa.text("false"),
    )
    op.alter_column(
        "orderbook_orders",
        "status",
        existing_type=sa.String(length=20),
        nullable=False,
        existing_nullable=True,
        server_default=sa.text("'OPEN'"),
        existing_server_default=sa.text("'OPEN'::character varying"),
    )
    for column in ("created_at", "updated_at"):
        op.alter_column(
            "orderbook_orders",
            column,
            existing_type=sa.DateTime(timezone=True),
            nullable=False,
            existing_nullable=True,
            server_default=sa.text("now()"),
            existing_server_default=sa.text("now()"),
        )

    op.alter_column(
        "trades",
        "status",
        existing_type=sa.String(length=30),
        nullable=False,
        existing_nullable=True,
        server_default=sa.text("'PENDING_CONFIRMATION'"),
        existing_server_default=sa.text("'PENDING_CONFIRMATION'::character varying"),
    )
    op.alter_column(
        "trades",
        "commission_rate_pct",
        existing_type=sa.Numeric(precision=5, scale=3),
        nullable=False,
        existing_nullable=True,
        server_default=sa.text("0.5"),
        existing_server_default=sa.text("0.5"),
    )
    op.alter_column(
        "trades",
        "created_at",
        existing_type=sa.DateTime(timezone=True),
        nullable=False,
        existing_nullable=True,
        server_default=sa.text("now()"),
        existing_server_default=sa.text("now()"),
    )
    op.alter_column(
        "audit_logs",
        "timestamp",
        existing_type=sa.DateTime(timezone=True),
        nullable=False,
        existing_nullable=True,
        server_default=sa.text("now()"),
        existing_server_default=sa.text("now()"),
    )
    op.alter_column(
        "producer_projects",
        "status",
        existing_type=sa.String(length=30),
        nullable=False,
        existing_nullable=True,
        server_default=sa.text("'ANNOUNCED'"),
        existing_server_default=sa.text("'ANNOUNCED'::character varying"),
    )
    for column in ("created_at", "updated_at"):
        op.alter_column(
            "producer_projects",
            column,
            existing_type=sa.DateTime(timezone=True),
            nullable=False,
            existing_nullable=True,
            server_default=sa.text("now()"),
            existing_server_default=sa.text("now()"),
        )
    op.alter_column(
        "match_suggestions",
        "match_reasons",
        existing_type=sa.JSON(),
        nullable=False,
        existing_nullable=True,
        server_default=sa.text("'[]'::json"),
        existing_server_default=sa.text("'[]'::json"),
    )
    op.alter_column(
        "match_suggestions",
        "status",
        existing_type=sa.String(length=20),
        nullable=False,
        existing_nullable=True,
        server_default=sa.text("'SUGGESTED'"),
        existing_server_default=sa.text("'SUGGESTED'::character varying"),
    )
    op.alter_column(
        "match_suggestions",
        "created_at",
        existing_type=sa.DateTime(timezone=True),
        nullable=False,
        existing_nullable=True,
        server_default=sa.text("now()"),
        existing_server_default=sa.text("now()"),
    )
    op.alter_column(
        "commissions",
        "match_id",
        existing_type=sa.UUID(),
        nullable=False,
        existing_nullable=True,
    )
    op.alter_column(
        "users",
        "status",
        existing_type=sa.String(length=8),
        server_default=sa.text("'PENDING'"),
        existing_server_default=sa.text("'APPROVED'::character varying"),
    )
    for table_name, column_name, length, current_default, expected_default in (
        ("negotiations", "status", 10, "'OPEN'::character varying", "'OPEN'"),
        ("referrals", "status", 10, "'SIGNED_UP'::character varying", "'SIGNED_UP'"),
        ("rfq_quotes", "status", 10, "'PENDING'::character varying", "'PENDING'"),
        ("rfqs", "status", 10, "'OPEN'::character varying", "'OPEN'"),
    ):
        op.alter_column(
            table_name,
            column_name,
            existing_type=sa.String(),
            type_=sa.String(length=length),
            existing_nullable=False,
            server_default=sa.text(expected_default),
            existing_server_default=sa.text(current_default),
        )


def downgrade() -> None:
    # Reverse metadata only; all delivery-window data remains intact.
    op.alter_column(
        "match_suggestions", "created_at", existing_type=sa.DateTime(timezone=True),
        nullable=True, existing_nullable=False, server_default=sa.text("now()"),
    )
    op.alter_column(
        "match_suggestions", "status", existing_type=sa.String(length=20),
        nullable=True, existing_nullable=False, server_default=sa.text("'SUGGESTED'"),
    )
    op.alter_column(
        "match_suggestions", "match_reasons", existing_type=sa.JSON(),
        nullable=True, existing_nullable=False, server_default=sa.text("'[]'::json"),
    )
    for column in ("created_at", "updated_at"):
        op.alter_column(
            "producer_projects", column, existing_type=sa.DateTime(timezone=True),
            nullable=True, existing_nullable=False, server_default=sa.text("now()"),
        )
    op.alter_column(
        "producer_projects", "status", existing_type=sa.String(length=30),
        nullable=True, existing_nullable=False, server_default=sa.text("'ANNOUNCED'"),
    )
    op.alter_column(
        "audit_logs", "timestamp", existing_type=sa.DateTime(timezone=True),
        nullable=True, existing_nullable=False, server_default=sa.text("now()"),
    )
    op.alter_column(
        "trades", "created_at", existing_type=sa.DateTime(timezone=True),
        nullable=True, existing_nullable=False, server_default=sa.text("now()"),
    )
    op.alter_column(
        "trades", "commission_rate_pct", existing_type=sa.Numeric(precision=5, scale=3),
        nullable=True, existing_nullable=False, server_default=sa.text("0.5"),
    )
    op.alter_column(
        "trades", "status", existing_type=sa.String(length=30),
        nullable=True, existing_nullable=False, server_default=sa.text("'PENDING_CONFIRMATION'"),
    )
    for column in ("created_at", "updated_at"):
        op.alter_column(
            "orderbook_orders", column, existing_type=sa.DateTime(timezone=True),
            nullable=True, existing_nullable=False, server_default=sa.text("now()"),
        )
    op.alter_column(
        "orderbook_orders", "status", existing_type=sa.String(length=20),
        nullable=True, existing_nullable=False, server_default=sa.text("'OPEN'"),
    )
    op.alter_column(
        "orderbook_orders", "is_verdaxis_verified", existing_type=sa.Boolean(),
        nullable=True, existing_nullable=False, server_default=sa.text("false"),
    )
    op.alter_column(
        "orderbook_orders", "availability_window", existing_type=sa.String(length=50),
        nullable=True, existing_nullable=False, server_default=sa.text("'SPOT'"),
    )
    op.alter_column(
        "inventory_items", "fuel_type", existing_type=sa.String(length=16),
        type_=sa.String(length=8),
    )
    for table_name, column_name, current_default in (
        ("rfqs", "status", "'OPEN'"),
        ("rfq_quotes", "status", "'PENDING'"),
        ("referrals", "status", "'SIGNED_UP'"),
        ("negotiations", "status", "'OPEN'"),
    ):
        op.alter_column(
            table_name,
            column_name,
            existing_type=sa.String(length=10),
            type_=sa.String(),
            nullable=False,
            existing_nullable=False,
            server_default=sa.text(current_default),
        )
    op.alter_column(
        "users", "status", existing_type=sa.String(length=8),
        server_default=sa.text("'APPROVED'"),
        existing_server_default=sa.text("'PENDING'"),
    )
    op.alter_column(
        "commissions", "match_id", existing_type=sa.UUID(),
        nullable=True, existing_nullable=False,
    )

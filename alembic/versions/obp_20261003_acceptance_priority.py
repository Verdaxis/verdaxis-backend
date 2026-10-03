"""Persist deterministic order acceptance priority.

Revision ID: obp_20261003_acceptance_priority
Revises: auth_20261002_session_cutoff
Create Date: 2026-10-03
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op


revision = "obp_20261003_acceptance_priority"
down_revision = "auth_20261002_session_cutoff"
branch_labels = None
depends_on = None

_SEQUENCE = "orderbook_acceptance_ordinal_seq"


def upgrade() -> None:
    bind = op.get_bind()
    # A concurrent admission must never observe a partially backfilled queue.
    # The operator retries the checkpoint later if the small table is busy.
    bind.execute(sa.text("LOCK TABLE orderbook_orders IN ACCESS EXCLUSIVE MODE NOWAIT"))
    op.execute(sa.text(f"CREATE SEQUENCE {_SEQUENCE}"))
    op.add_column(
        "orderbook_orders",
        sa.Column("acceptance_ordinal", sa.BigInteger(), nullable=True),
    )
    bind.execute(
        sa.text(
            "WITH ranked AS ("
            "SELECT id, row_number() OVER (ORDER BY created_at, id) AS ordinal "
            "FROM orderbook_orders"
            ") "
            "UPDATE orderbook_orders AS orders "
            "SET acceptance_ordinal = ranked.ordinal "
            "FROM ranked WHERE orders.id = ranked.id"
        )
    )
    maximum = bind.execute(
        sa.text("SELECT max(acceptance_ordinal) FROM orderbook_orders")
    ).scalar_one()
    if maximum is None:
        bind.execute(
            sa.text(f"SELECT setval('{_SEQUENCE}', 1, false)")
        )
    else:
        bind.execute(
            sa.text(f"SELECT setval('{_SEQUENCE}', :maximum, true)"),
            {"maximum": int(maximum)},
        )
    op.alter_column(
        "orderbook_orders",
        "acceptance_ordinal",
        existing_type=sa.BigInteger(),
        nullable=False,
        server_default=sa.text(f"nextval('{_SEQUENCE}'::regclass)"),
    )
    op.create_unique_constraint(
        "uq_orderbook_orders_acceptance_ordinal",
        "orderbook_orders",
        ["acceptance_ordinal"],
    )


def downgrade() -> None:
    bind = op.get_bind()
    bind.execute(sa.text("LOCK TABLE orderbook_orders IN ACCESS EXCLUSIVE MODE NOWAIT"))
    op.drop_constraint(
        "uq_orderbook_orders_acceptance_ordinal",
        "orderbook_orders",
        type_="unique",
    )
    op.drop_column("orderbook_orders", "acceptance_ordinal")
    op.execute(sa.text(f"DROP SEQUENCE {_SEQUENCE}"))

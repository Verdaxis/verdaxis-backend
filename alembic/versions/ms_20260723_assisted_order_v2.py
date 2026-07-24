"""Broaden assisted order entry to organization-scoped BID and ASK listings.

Revision ID: ms_20260723_assisted_order_v2
Revises: ms_20260723_organization_context
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op


revision = "ms_20260723_assisted_order_v2"
down_revision = "ms_20260723_organization_context"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.drop_constraint(
        "ck_market_support_context_scope",
        "market_support_contexts",
        type_="check",
    )
    op.execute(
        "UPDATE market_support_contexts "
        "SET scope = 'ASSISTED_ORDER_ENTRY' "
        "WHERE scope = 'ASK_LISTINGS'"
    )
    op.alter_column(
        "market_support_contexts",
        "scope",
        existing_type=sa.String(length=32),
        server_default="ASSISTED_ORDER_ENTRY",
        nullable=False,
    )
    op.create_check_constraint(
        "ck_market_support_context_scope",
        "market_support_contexts",
        "scope IN ('ASSISTED_ORDER_ENTRY')",
    )

    op.add_column(
        "market_support_authorizations",
        sa.Column("order_side", sa.String(length=8), server_default="ASK", nullable=False),
    )
    op.alter_column(
        "market_support_authorizations",
        "order_expires_at",
        existing_type=sa.DateTime(timezone=True),
        nullable=True,
    )
    op.alter_column(
        "market_support_authorizations",
        "evidence_sha256",
        existing_type=sa.String(length=64),
        nullable=True,
    )
    for column_name, column_type in (
        ("certification_scheme", sa.String(length=120)),
        ("specification_standard", sa.String(length=120)),
        ("carbon_intensity_gco2_mj", sa.Numeric()),
        ("feedstock", sa.String(length=255)),
        ("origin", sa.String(length=255)),
    ):
        op.alter_column(
            "market_support_authorizations",
            column_name,
            existing_type=column_type,
            nullable=True,
        )
    op.drop_constraint(
        "ck_market_support_auth_expiry_order",
        "market_support_authorizations",
        type_="check",
    )
    op.create_check_constraint(
        "ck_market_support_auth_expiry_order",
        "market_support_authorizations",
        "order_expires_at IS NULL OR authorization_expires_at <= order_expires_at",
    )
    op.create_check_constraint(
        "ck_market_support_auth_order_side",
        "market_support_authorizations",
        "order_side IN ('BID', 'ASK')",
    )


def downgrade() -> None:
    raise RuntimeError(
        "downgrade is unsupported: organization-scoped assisted orders retain audit history"
    )

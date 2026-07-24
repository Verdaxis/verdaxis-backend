"""Opaque short-lived Market Support organization contexts.

Revision ID: ms_20260723_organization_context
Revises: ms_20260723_assisted_listings
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op
from sqlalchemy.dialects import postgresql


revision = "ms_20260723_organization_context"
down_revision = "ms_20260723_assisted_listings"
branch_labels = None
depends_on = None

UUID = postgresql.UUID(as_uuid=True)


def upgrade() -> None:
    op.create_table(
        "market_support_contexts",
        sa.Column("id", UUID, nullable=False),
        sa.Column("actor_user_id", UUID, nullable=False),
        sa.Column("organization_id", UUID, nullable=False),
        sa.Column("accountable_user_id", UUID, nullable=False),
        sa.Column("support_reference", sa.String(length=200), nullable=False),
        sa.Column("scope", sa.String(length=32), server_default="ASK_LISTINGS", nullable=False),
        sa.Column("started_at", sa.DateTime(timezone=True), server_default=sa.func.now(), nullable=False),
        sa.Column("expires_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("ended_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("status", sa.String(length=16), server_default="ACTIVE", nullable=False),
        sa.Column("version", sa.Integer(), server_default="1", nullable=False),
        sa.CheckConstraint("scope IN ('ASK_LISTINGS')", name="ck_market_support_context_scope"),
        sa.CheckConstraint(
            "status IN ('ACTIVE', 'EXITED', 'EXPIRED', 'REVOKED')",
            name="ck_market_support_context_status",
        ),
        sa.CheckConstraint("expires_at > started_at", name="ck_market_support_context_expiry"),
        sa.CheckConstraint("version >= 1", name="ck_market_support_context_version"),
        sa.ForeignKeyConstraint(["actor_user_id"], ["users.id"], ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(["organization_id"], ["organizations.id"], ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(["accountable_user_id"], ["users.id"], ondelete="RESTRICT"),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index(
        "ix_market_support_context_actor_status",
        "market_support_contexts",
        ["actor_user_id", "status", "expires_at"],
    )
    op.create_index(
        "ix_market_support_context_organization_status",
        "market_support_contexts",
        ["organization_id", "status", "expires_at"],
    )
    op.create_index(
        "uq_market_support_context_actor_active",
        "market_support_contexts",
        ["actor_user_id"],
        unique=True,
        postgresql_where=sa.text("status = 'ACTIVE'"),
    )
    op.add_column(
        "market_support_authorizations",
        sa.Column("market_support_context_id", UUID, nullable=True),
    )
    op.add_column(
        "market_support_authorizations",
        sa.Column("instruction_at", sa.DateTime(timezone=True), nullable=True),
    )
    op.add_column(
        "market_support_authorizations",
        sa.Column("acknowledge_exact_terms", sa.Boolean(), nullable=True),
    )
    op.add_column(
        "market_support_authorizations",
        sa.Column("acknowledge_executable_standing_order", sa.Boolean(), nullable=True),
    )
    op.create_index(
        "ix_market_support_authorizations_context",
        "market_support_authorizations",
        ["market_support_context_id"],
    )
    op.create_foreign_key(
        "fk_market_support_authorizations_context",
        "market_support_authorizations",
        "market_support_contexts",
        ["market_support_context_id"],
        ["id"],
        ondelete="RESTRICT",
    )


def downgrade() -> None:
    raise RuntimeError(
        "downgrade is unsupported: market-support contexts retain audit identity"
    )

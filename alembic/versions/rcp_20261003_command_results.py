"""Persist immutable successful market command results.

Revision ID: rcp_20261003_command_results
Revises: obp_20261003_acceptance_priority
Create Date: 2026-10-03
"""
from __future__ import annotations

import sqlalchemy as sa
from sqlalchemy.dialects import postgresql
from alembic import op


revision = "rcp_20261003_command_results"
down_revision = "obp_20261003_acceptance_priority"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "market_command_results",
        sa.Column("id", postgresql.UUID(as_uuid=True), nullable=False),
        sa.Column("actor_user_id", postgresql.UUID(as_uuid=True), nullable=False),
        sa.Column("effective_organization_id", postgresql.UUID(as_uuid=True), nullable=False),
        sa.Column("support_context_id", postgresql.UUID(as_uuid=True), nullable=True),
        sa.Column("operation", sa.String(length=32), nullable=False),
        sa.Column("idempotency_key", sa.String(length=255), nullable=False),
        sa.Column("request_hash", sa.String(length=64), nullable=False),
        sa.Column("resource_type", sa.String(length=16), nullable=False),
        sa.Column("resource_id", postgresql.UUID(as_uuid=True), nullable=False),
        sa.Column("response_status", sa.SmallInteger(), nullable=False),
        sa.Column("response_body", postgresql.JSONB(astext_type=sa.Text()), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), server_default=sa.func.now(), nullable=False),
        sa.CheckConstraint("operation IN ('trade.confirm', 'trade.decline', 'trade.deliver', 'trade.pay', 'order.amend', 'order.cancel')", name="ck_market_command_results_operation"),
        sa.CheckConstraint("resource_type IN ('trade', 'order')", name="ck_market_command_results_resource_type"),
        sa.CheckConstraint("length(btrim(idempotency_key)) BETWEEN 1 AND 255", name="ck_market_command_results_key"),
        sa.CheckConstraint("request_hash ~ '^[0-9a-f]{64}$'", name="ck_market_command_results_request_hash"),
        sa.CheckConstraint("response_status BETWEEN 200 AND 299", name="ck_market_command_results_response_status"),
        sa.ForeignKeyConstraint(["actor_user_id"], ["users.id"], ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(["effective_organization_id"], ["organizations.id"], ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(["support_context_id"], ["market_support_contexts.id"], ondelete="RESTRICT"),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint("actor_user_id", "operation", "idempotency_key", name="uq_market_command_results_actor_operation_key"),
    )
    op.create_index("ix_market_command_results_resource_created", "market_command_results", ["resource_type", "resource_id", "created_at"])


def downgrade() -> None:
    op.drop_index("ix_market_command_results_resource_created", table_name="market_command_results")
    op.drop_table("market_command_results")

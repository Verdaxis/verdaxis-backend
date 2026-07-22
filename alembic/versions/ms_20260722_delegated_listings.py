"""Delegated market-support authorization and attribution records.

Revision ID: ms_20260722_delegated_listings
Revises: sse_20260720_market_event_stream
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op
from sqlalchemy.dialects import postgresql


revision = "ms_20260722_delegated_listings"
down_revision = "sse_20260720_market_event_stream"
branch_labels = None
depends_on = None


_UUID = postgresql.UUID(as_uuid=True)


def upgrade() -> None:
    op.create_table(
        "admin_capability_grants",
        sa.Column("id", _UUID, nullable=False),
        sa.Column("user_id", _UUID, nullable=False),
        sa.Column("capability", sa.String(length=40), nullable=False),
        sa.Column("granted_by_user_id", _UUID, nullable=True),
        sa.Column("reason", sa.String(length=500), nullable=False),
        sa.Column("granted_at", sa.DateTime(timezone=True), server_default=sa.func.now(), nullable=False),
        sa.Column("expires_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("revoked_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("revoked_by_user_id", _UUID, nullable=True),
        sa.CheckConstraint(
            "capability IN ('MARKET_SUPPORT_LISTINGS', 'MARKET_SUPPORT_AUTHORIZATIONS')",
            name="ck_admin_capability_grants_capability",
        ),
        sa.ForeignKeyConstraint(["granted_by_user_id"], ["users.id"], ondelete="SET NULL"),
        sa.ForeignKeyConstraint(["revoked_by_user_id"], ["users.id"], ondelete="SET NULL"),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint(
            "user_id", "capability", name="uq_admin_capability_grants_user_capability"
        ),
    )
    op.create_index(
        "ix_admin_capability_grants_active",
        "admin_capability_grants",
        ["user_id", "capability", "revoked_at", "expires_at"],
    )

    op.create_table(
        "organization_support_authorizations",
        sa.Column("id", _UUID, nullable=False),
        sa.Column("organization_id", _UUID, nullable=False),
        sa.Column("authorized_contact_name", sa.String(length=200), nullable=False),
        sa.Column("authorized_contact_email", sa.String(length=320), nullable=False),
        sa.Column("evidence_reference", sa.String(length=500), nullable=False),
        sa.Column("support_case_reference", sa.String(length=200), nullable=True),
        sa.Column("scopes", sa.JSON(), nullable=False),
        sa.Column("valid_from", sa.DateTime(timezone=True), nullable=False),
        sa.Column("valid_until", sa.DateTime(timezone=True), nullable=False),
        sa.Column("status", sa.String(length=16), server_default="ACTIVE", nullable=False),
        sa.Column("allowed_side", sa.String(length=3), server_default="ASK", nullable=False),
        sa.Column("product_id", _UUID, nullable=True),
        sa.Column("delivery_point_id", _UUID, nullable=True),
        sa.Column("min_price_per_mt_usd", sa.Numeric(precision=12, scale=2), nullable=True),
        sa.Column("max_price_per_mt_usd", sa.Numeric(precision=12, scale=2), nullable=True),
        sa.Column("max_quantity_mt_per_order", sa.Numeric(precision=14, scale=2), nullable=False),
        sa.Column("max_total_open_quantity_mt", sa.Numeric(precision=14, scale=2), nullable=False),
        sa.Column("max_order_ttl_hours", sa.Integer(), server_default="168", nullable=False),
        sa.Column("max_uses", sa.Integer(), nullable=True),
        sa.Column("uses_count", sa.Integer(), server_default="0", nullable=False),
        sa.Column("created_by_admin_user_id", _UUID, nullable=False),
        sa.Column("created_at", sa.DateTime(timezone=True), server_default=sa.func.now(), nullable=False),
        sa.Column("revoked_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("revoked_by_admin_user_id", _UUID, nullable=True),
        sa.Column("revocation_reason", sa.String(length=500), nullable=True),
        sa.CheckConstraint("valid_until > valid_from", name="ck_support_authorization_valid_range"),
        sa.CheckConstraint(
            "min_price_per_mt_usd IS NULL OR max_price_per_mt_usd IS NULL "
            "OR min_price_per_mt_usd <= max_price_per_mt_usd",
            name="ck_support_authorization_price_range",
        ),
        sa.CheckConstraint("max_quantity_mt_per_order > 0", name="ck_support_authorization_quantity"),
        sa.CheckConstraint(
            "max_total_open_quantity_mt > 0", name="ck_support_authorization_open_quantity"
        ),
        sa.CheckConstraint(
            "max_order_ttl_hours >= 1 AND max_order_ttl_hours <= 720",
            name="ck_support_authorization_ttl",
        ),
        sa.CheckConstraint("uses_count >= 0", name="ck_support_authorization_uses_count"),
        sa.CheckConstraint(
            "max_uses IS NULL OR (max_uses > 0 AND uses_count <= max_uses)",
            name="ck_support_authorization_uses",
        ),
        sa.CheckConstraint("status IN ('ACTIVE', 'REVOKED')", name="ck_support_authorization_status"),
        sa.CheckConstraint("allowed_side = 'ASK'", name="ck_support_authorization_side"),
        sa.ForeignKeyConstraint(["created_by_admin_user_id"], ["users.id"], ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(["delivery_point_id"], ["delivery_points.id"], ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(["organization_id"], ["organizations.id"], ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(["product_id"], ["products.id"], ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(["revoked_by_admin_user_id"], ["users.id"], ondelete="SET NULL"),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index(
        "ix_support_authorizations_org_state_validity",
        "organization_support_authorizations",
        ["organization_id", "status", "valid_from", "valid_until"],
    )

    op.create_table(
        "order_support_attributions",
        sa.Column("order_id", _UUID, nullable=False),
        sa.Column("organization_id", _UUID, nullable=False),
        sa.Column("accountable_user_id", _UUID, nullable=False),
        sa.Column("created_by_admin_user_id", _UUID, nullable=False),
        sa.Column("support_authorization_id", _UUID, nullable=False),
        sa.Column(
            "submission_method",
            sa.String(length=32),
            server_default="VERDAXIS_ASSISTED",
            nullable=False,
        ),
        sa.Column("support_version", sa.Integer(), server_default="1", nullable=False),
        sa.Column(
            "management_authority",
            sa.String(length=24),
            server_default="SUPPORT_MANDATE",
            nullable=False,
        ),
        sa.Column("customer_adopted_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("customer_adopted_by_user_id", _UUID, nullable=True),
        sa.Column("last_action_actor_user_id", _UUID, nullable=False),
        sa.Column("last_action_reason_code", sa.String(length=64), nullable=False),
        sa.Column("support_case_reference", sa.String(length=200), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), server_default=sa.func.now(), nullable=False),
        sa.Column("updated_at", sa.DateTime(timezone=True), server_default=sa.func.now(), nullable=False),
        sa.CheckConstraint("submission_method = 'VERDAXIS_ASSISTED'", name="ck_order_support_submission_method"),
        sa.CheckConstraint(
            "management_authority IN ('SUPPORT_MANDATE', 'CUSTOMER_DIRECT')",
            name="ck_order_support_management_authority",
        ),
        sa.CheckConstraint("support_version >= 1", name="ck_order_support_version"),
        sa.ForeignKeyConstraint(["accountable_user_id"], ["users.id"], ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(["created_by_admin_user_id"], ["users.id"], ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(["customer_adopted_by_user_id"], ["users.id"], ondelete="SET NULL"),
        sa.ForeignKeyConstraint(["last_action_actor_user_id"], ["users.id"], ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(["order_id"], ["orderbook_orders.id"], ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(["organization_id"], ["organizations.id"], ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(
            ["support_authorization_id"],
            ["organization_support_authorizations.id"],
            ondelete="RESTRICT",
        ),
        sa.PrimaryKeyConstraint("order_id"),
    )
    op.create_index(
        "ix_order_support_attributions_org",
        "order_support_attributions",
        ["organization_id", "created_at"],
    )
    op.create_index(
        "ix_order_support_attributions_authorization",
        "order_support_attributions",
        ["support_authorization_id", "created_at"],
    )

    op.create_table(
        "market_support_action_receipts",
        sa.Column("id", _UUID, nullable=False),
        sa.Column("organization_id", _UUID, nullable=False),
        sa.Column("operation", sa.String(length=16), nullable=False),
        sa.Column("idempotency_key", sa.String(length=255), nullable=False),
        sa.Column("request_hash", sa.String(length=64), nullable=False),
        sa.Column("order_id", _UUID, nullable=False),
        sa.Column("support_version", sa.Integer(), nullable=False),
        sa.Column("created_by_admin_user_id", _UUID, nullable=False),
        sa.Column("created_at", sa.DateTime(timezone=True), server_default=sa.func.now(), nullable=False),
        sa.CheckConstraint("operation IN ('CREATE', 'UPDATE', 'CANCEL')", name="ck_market_support_receipt_operation"),
        sa.CheckConstraint("support_version >= 1", name="ck_market_support_receipt_version"),
        sa.ForeignKeyConstraint(["created_by_admin_user_id"], ["users.id"], ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(["order_id"], ["orderbook_orders.id"], ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(["organization_id"], ["organizations.id"], ondelete="RESTRICT"),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint(
            "organization_id",
            "operation",
            "idempotency_key",
            name="uq_market_support_receipts_org_operation_key",
        ),
    )
    op.create_index(
        "ix_market_support_receipts_order",
        "market_support_action_receipts",
        ["order_id", "created_at"],
    )


def downgrade() -> None:
    # These records are the evidence chain for live commercial actions. A
    # downgrade would orphan assisted orders from their authorization and actor
    # attribution, so restore a parent-schema backup instead.
    raise RuntimeError(
        "ms_20260722_delegated_listings downgrade is unsupported: "
        "restore a parent-schema backup instead"
    )

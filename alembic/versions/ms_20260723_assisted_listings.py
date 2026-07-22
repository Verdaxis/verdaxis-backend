"""Exact one-use authorizations for assisted supplier listings.

Revision ID: ms_20260723_assisted_listings
Revises: sse_20260720_market_event_stream
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op
from sqlalchemy.dialects import postgresql


revision = "ms_20260723_assisted_listings"
down_revision = "sse_20260720_market_event_stream"
branch_labels = None
depends_on = None


UUID = postgresql.UUID(as_uuid=True)


def upgrade() -> None:
    op.create_table(
        "staff_capability_assignments",
        sa.Column("id", UUID, nullable=False),
        sa.Column("user_id", UUID, nullable=False),
        sa.Column("capability", sa.String(length=40), nullable=False),
        sa.Column("reason", sa.String(length=500), nullable=False),
        sa.Column("granted_by_user_id", UUID, nullable=False),
        sa.Column("granted_at", sa.DateTime(timezone=True), server_default=sa.func.now(), nullable=False),
        sa.Column("expires_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("revoked_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("revoked_by_user_id", UUID, nullable=True),
        sa.Column("revocation_reason", sa.String(length=500), nullable=True),
        sa.CheckConstraint(
            "capability IN ('MARKET_SUPPORT_LISTINGS', 'MARKET_SUPPORT_AUTHORIZATIONS')",
            name="ck_staff_capability_name",
        ),
        sa.ForeignKeyConstraint(["granted_by_user_id"], ["users.id"], ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(["revoked_by_user_id"], ["users.id"], ondelete="SET NULL"),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint("user_id", "capability", name="uq_staff_capability_user_capability"),
    )
    op.create_index(
        "ix_staff_capability_active",
        "staff_capability_assignments",
        ["user_id", "capability", "revoked_at", "expires_at"],
    )

    op.create_table(
        "market_support_authorizations",
        sa.Column("id", UUID, nullable=False),
        sa.Column("organization_id", UUID, nullable=False),
        sa.Column("accountable_user_id", UUID, nullable=False),
        sa.Column("status", sa.String(length=16), server_default="ACTIVE", nullable=False),
        sa.Column("product_id", UUID, nullable=False),
        sa.Column("delivery_point_id", UUID, nullable=False),
        sa.Column("availability_window", sa.String(length=16), nullable=False),
        sa.Column("quantity_mt", sa.Numeric(12, 2), nullable=False),
        sa.Column("price_per_mt_usd", sa.Numeric(10, 2), nullable=False),
        sa.Column("authorization_expires_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("order_expires_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("is_anonymous", sa.Boolean(), server_default=sa.true(), nullable=False),
        sa.Column("certifications", sa.JSON(), server_default=sa.text("'[]'::json"), nullable=False),
        sa.Column("certification_declared", sa.Boolean(), nullable=False),
        sa.Column("certification_scheme", sa.String(length=120), nullable=False),
        sa.Column("specification_standard", sa.String(length=120), nullable=False),
        sa.Column("msds_available", sa.Boolean(), nullable=False),
        sa.Column("carbon_intensity_gco2_mj", sa.Numeric(), nullable=False),
        sa.Column("carbon_intensity_method", sa.String(length=120), nullable=True),
        sa.Column("feedstock", sa.String(length=255), nullable=False),
        sa.Column("origin", sa.String(length=255), nullable=False),
        sa.Column("off_spec", sa.Boolean(), server_default=sa.false(), nullable=False),
        sa.Column("off_spec_notes", sa.Text(), nullable=True),
        sa.Column("terms_digest", sa.String(length=64), nullable=False),
        sa.Column("evidence_reference", sa.String(length=500), nullable=False),
        sa.Column("evidence_sha256", sa.String(length=64), nullable=False),
        sa.Column("commercial_consent_version", sa.String(length=64), nullable=False),
        sa.Column("commercial_consent_reference", sa.String(length=500), nullable=False),
        sa.Column("support_case_reference", sa.String(length=200), nullable=True),
        sa.Column("idempotency_key", sa.String(length=255), nullable=False),
        sa.Column("idempotency_request_hash", sa.String(length=64), nullable=False),
        sa.Column("created_by_actor_user_id", UUID, nullable=False),
        sa.Column("created_at", sa.DateTime(timezone=True), server_default=sa.func.now(), nullable=False),
        sa.Column("consumed_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("revoked_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("revoked_by_actor_user_id", UUID, nullable=True),
        sa.Column("revocation_reason", sa.String(length=500), nullable=True),
        sa.CheckConstraint("quantity_mt > 0", name="ck_market_support_auth_quantity"),
        sa.CheckConstraint("price_per_mt_usd > 0", name="ck_market_support_auth_price"),
        sa.CheckConstraint(
            "authorization_expires_at <= order_expires_at",
            name="ck_market_support_auth_expiry_order",
        ),
        sa.CheckConstraint(
            "status IN ('ACTIVE', 'CONSUMED', 'REVOKED')",
            name="ck_market_support_auth_status",
        ),
        sa.ForeignKeyConstraint(["accountable_user_id"], ["users.id"], ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(["created_by_actor_user_id"], ["users.id"], ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(["delivery_point_id"], ["delivery_points.id"], ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(["organization_id"], ["organizations.id"], ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(["product_id"], ["products.id"], ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(["revoked_by_actor_user_id"], ["users.id"], ondelete="SET NULL"),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint("organization_id", "idempotency_key", name="uq_market_support_auth_org_idempotency"),
    )
    op.create_index(
        "ix_market_support_auth_org_status",
        "market_support_authorizations",
        ["organization_id", "status", "created_at"],
    )

    op.add_column("orderbook_orders", sa.Column("created_by_actor_user_id", UUID, nullable=True))
    op.add_column(
        "orderbook_orders",
        sa.Column("creation_method", sa.String(length=24), server_default="LEGACY_UNKNOWN", nullable=False),
    )
    op.add_column("orderbook_orders", sa.Column("support_authorization_id", UUID, nullable=True))
    op.add_column(
        "orderbook_orders",
        sa.Column("version", sa.Integer(), server_default="1", nullable=False),
    )
    op.create_check_constraint(
        "ck_orderbook_creation_method",
        "orderbook_orders",
        "creation_method IN ('SELF_SERVICE', 'MARKET_SUPPORT', 'SYSTEM', 'LEGACY_UNKNOWN')",
    )
    op.create_check_constraint(
        "ck_orderbook_version", "orderbook_orders", "version >= 1"
    )
    op.create_foreign_key(
        "fk_orderbook_created_by_actor",
        "orderbook_orders",
        "users",
        ["created_by_actor_user_id"],
        ["id"],
        ondelete="SET NULL",
    )
    op.create_foreign_key(
        "fk_orderbook_support_authorization",
        "orderbook_orders",
        "market_support_authorizations",
        ["support_authorization_id"],
        ["id"],
        ondelete="RESTRICT",
    )
    op.create_unique_constraint(
        "uq_orderbook_support_authorization",
        "orderbook_orders",
        ["support_authorization_id"],
    )


def downgrade() -> None:
    raise RuntimeError(
        "downgrade is unsupported: ms_20260723_assisted_listings carries "
        "commercial authorization evidence; restore a parent-schema backup"
    )

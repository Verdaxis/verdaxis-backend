"""add contract supply_listing demand_profile tables

Revision ID: contracts_2026_03
Revises: wl_2026_03
Create Date: 2026-03-18
"""
from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects.postgresql import UUID

revision = "contracts_2026_03"
down_revision = "wl_2026_03"
branch_labels = None
depends_on = None


def upgrade() -> None:
    # ── contracts ────────────────────────────────────────────────────
    op.create_table(
        "contracts",
        sa.Column("id", UUID(as_uuid=True), primary_key=True),
        # Instrument & status
        sa.Column("instrument_type", sa.String(), nullable=False),
        sa.Column("status", sa.String(), nullable=False, server_default="DRAFT"),
        # Parties
        sa.Column("buyer_org_id", UUID(as_uuid=True), sa.ForeignKey("organizations.id"), nullable=False),
        sa.Column("supplier_org_id", UUID(as_uuid=True), sa.ForeignKey("organizations.id"), nullable=True),
        # Product & delivery
        sa.Column("product_id", UUID(as_uuid=True), sa.ForeignKey("products.id"), nullable=False),
        sa.Column("delivery_point_id", UUID(as_uuid=True), sa.ForeignKey("delivery_points.id"), nullable=False),
        sa.Column("fuel_type", sa.String(), nullable=False),
        # Compliance
        sa.Column("ci_score", sa.Numeric(8, 2), nullable=False, comment="gCO2eq/MJ well-to-wake"),
        sa.Column("certification_body", sa.String(), nullable=False),
        # Quantity
        sa.Column("lot_count", sa.Integer(), nullable=False, comment="1 lot = 500 MT"),
        sa.Column("quantity_mt", sa.Numeric(12, 2), nullable=False, comment="lot_count * 500"),
        # Price
        sa.Column("price_per_mt_usd", sa.Numeric(10, 2), nullable=False),
        sa.Column("price_structure", sa.String(), nullable=False, server_default="FIXED"),
        sa.Column("index_reference", sa.String(), nullable=True),
        # Delivery windows
        sa.Column("availability_window", sa.String(), nullable=False),
        sa.Column("delivery_window_start", sa.Date(), nullable=False),
        sa.Column("delivery_window_end", sa.Date(), nullable=False),
        # Supply tier
        sa.Column("supply_tier", sa.String(), nullable=False, server_default="FIRM"),
        # FDR-specific
        sa.Column("deposit_pct", sa.Numeric(5, 2), nullable=True, comment="FDR: 10-15%"),
        sa.Column("deposit_amount_usd", sa.Numeric(14, 2), nullable=True),
        # TOP-specific
        sa.Column("shortfall_fee_pct", sa.Numeric(5, 2), nullable=True, comment="Take-or-Pay: 85-95%"),
        # NOV / compliance flags
        sa.Column("novation_eligible", sa.Boolean(), nullable=False, server_default=sa.text("false")),
        sa.Column("fueleu_compliant", sa.Boolean(), nullable=False, server_default=sa.text("false")),
        sa.Column("cii_impact_grade", sa.String(2), nullable=True),
        # Documents
        sa.Column("term_sheet_url", sa.String(), nullable=True),
        # Lifecycle
        sa.Column("expires_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()),
    )

    # ── supply_listings ──────────────────────────────────────────────
    op.create_table(
        "supply_listings",
        sa.Column("id", UUID(as_uuid=True), primary_key=True),
        # Owner
        sa.Column("supplier_org_id", UUID(as_uuid=True), sa.ForeignKey("organizations.id"), nullable=False),
        # Product & delivery
        sa.Column("product_id", UUID(as_uuid=True), sa.ForeignKey("products.id"), nullable=False),
        sa.Column("delivery_point_id", UUID(as_uuid=True), sa.ForeignKey("delivery_points.id"), nullable=False),
        # Supply certainty
        sa.Column("supply_tier", sa.String(), nullable=False, server_default="FIRM"),
        # Lots
        sa.Column("available_lots", sa.Integer(), nullable=False),
        sa.Column("remaining_lots", sa.Integer(), nullable=False),
        # Compliance
        sa.Column("ci_score", sa.Numeric(8, 2), nullable=False, comment="gCO2eq/MJ well-to-wake"),
        sa.Column("certification_body", sa.String(), nullable=False),
        # Price
        sa.Column("price_per_mt_usd", sa.Numeric(10, 2), nullable=False),
        sa.Column("price_structure", sa.String(), nullable=False, server_default="FIXED"),
        # Delivery windows
        sa.Column("availability_window", sa.String(), nullable=False),
        sa.Column("delivery_window_start", sa.Date(), nullable=False),
        sa.Column("delivery_window_end", sa.Date(), nullable=False),
        # Production context
        sa.Column("fid_status", sa.String(), nullable=True, comment="Final Investment Decision status"),
        sa.Column("plant_location", sa.String(), nullable=True),
        # Active flag
        sa.Column("is_active", sa.Boolean(), nullable=False, server_default=sa.text("true")),
        # Lifecycle
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()),
    )

    # ── demand_profiles ──────────────────────────────────────────────
    op.create_table(
        "demand_profiles",
        sa.Column("id", UUID(as_uuid=True), primary_key=True),
        # Owner
        sa.Column("buyer_org_id", UUID(as_uuid=True), sa.ForeignKey("organizations.id"), nullable=False),
        # Demand spec
        sa.Column("fuel_type", sa.String(), nullable=False),
        sa.Column("min_ci_score", sa.Numeric(8, 2), nullable=False, comment="Minimum CI score gCO2eq/MJ"),
        sa.Column("quantity_mt", sa.Numeric(12, 2), nullable=False),
        sa.Column("port_preference", sa.String(), nullable=False),
        sa.Column("delivery_window", sa.String(), nullable=False),
        # Price cap
        sa.Column("max_price_per_mt_usd", sa.Numeric(10, 2), nullable=True),
        # Alert
        sa.Column("alert_enabled", sa.Boolean(), nullable=False, server_default=sa.text("true")),
        # Status
        sa.Column("status", sa.String(), nullable=False, server_default="ACTIVE"),
        # Lifecycle
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()),
    )


def downgrade() -> None:
    op.drop_table("demand_profiles")
    op.drop_table("supply_listings")
    op.drop_table("contracts")

"""add forward monitoring signal tables

Revision ID: fc_2026_06_monitor_signals
Revises: usr_2026_04_onboarding_survey
Create Date: 2026-06-17
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op
from sqlalchemy.dialects.postgresql import UUID


revision = "fc_2026_06_monitor_signals"
down_revision = "usr_2026_04_onboarding_survey"
branch_labels = None
depends_on = None


MARKET_PRODUCT_CHECK = "market_product IN ('BIO_METHANOL', 'E_METHANOL', 'BIO_ETHANOL', 'SYNTHETIC_ETHANOL')"
WINDOW_CHECK = (
    "availability_window = 'SPOT' "
    "OR availability_window LIKE '____-__' "
    "OR availability_window LIKE '____-Q_' "
    "OR availability_window LIKE '____-CAL'"
)
REAL_VERIFICATION_CHECK = "is_verified_real = false OR is_demo = false"


def upgrade() -> None:
    op.create_table(
        "market_signal_ingestion_runs",
        sa.Column("id", UUID(as_uuid=True), nullable=False),
        sa.Column("signal_family", sa.String(length=32), nullable=False),
        sa.Column("source", sa.String(length=64), nullable=False),
        sa.Column("source_kind", sa.String(length=32), nullable=False),
        sa.Column("started_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
        sa.Column("verified_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
        sa.CheckConstraint(
            "signal_family IN ('MARKET_INDICATION', 'FAIR_PRICE_BAND', 'PHYSICAL_STEM')",
            name="ck_market_signal_ingestion_runs_family",
        ),
        sa.CheckConstraint(
            "source_kind IN ('MARKET_INDICATION', 'FAIR_PRICE_MODEL', 'PHYSICAL_STEM')",
            name="ck_market_signal_ingestion_runs_source_kind",
        ),
        sa.PrimaryKeyConstraint("id"),
    )

    op.create_table(
        "market_indications",
        sa.Column("id", UUID(as_uuid=True), nullable=False),
        sa.Column("market_product", sa.String(length=64), nullable=False),
        sa.Column("delivery_point_id", UUID(as_uuid=True), sa.ForeignKey("delivery_points.id"), nullable=False),
        sa.Column("availability_window", sa.String(length=16), nullable=False),
        sa.Column("side", sa.String(length=8), nullable=False),
        sa.Column("price_per_mt_usd", sa.Numeric(10, 2), nullable=False),
        sa.Column("quantity_mt", sa.Numeric(12, 2), nullable=True),
        sa.Column("source", sa.String(length=64), nullable=False),
        sa.Column("source_record_id", sa.String(length=128), nullable=True),
        sa.Column("source_event_id", sa.String(length=128), nullable=True),
        sa.Column(
            "trusted_ingestion_run_id",
            UUID(as_uuid=True),
            sa.ForeignKey("market_signal_ingestion_runs.id"),
            nullable=True,
        ),
        sa.Column("is_demo", sa.Boolean(), nullable=False, server_default=sa.text("true")),
        sa.Column("is_verified_real", sa.Boolean(), nullable=False, server_default=sa.text("false")),
        sa.Column("verified_real_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("observed_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
        sa.CheckConstraint(MARKET_PRODUCT_CHECK, name="ck_market_indications_market_product"),
        sa.CheckConstraint(WINDOW_CHECK, name="ck_market_indications_availability_window"),
        sa.CheckConstraint("side IN ('BID', 'ASK', 'MID')", name="ck_market_indications_side"),
        sa.CheckConstraint("price_per_mt_usd > 0", name="ck_market_indications_price_positive"),
        sa.CheckConstraint("quantity_mt IS NULL OR quantity_mt > 0", name="ck_market_indications_quantity_positive"),
        sa.CheckConstraint(REAL_VERIFICATION_CHECK, name="ck_market_indications_real_verification"),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index(
        "ix_market_indications_board_lookup",
        "market_indications",
        ["market_product", "delivery_point_id", "availability_window", "observed_at", "created_at", "id"],
    )
    op.create_index(
        "ix_market_indications_latest_side",
        "market_indications",
        ["market_product", "delivery_point_id", "availability_window", "side", "observed_at", "created_at", "id"],
    )
    op.create_index(
        "uq_market_indications_source_event",
        "market_indications",
        ["source", "source_event_id"],
        unique=True,
        sqlite_where=sa.text("source_event_id IS NOT NULL"),
        postgresql_where=sa.text("source_event_id IS NOT NULL"),
    )
    op.create_index(
        "ix_market_indications_source_record",
        "market_indications",
        ["source", "source_record_id", "observed_at", "created_at", "id"],
        sqlite_where=sa.text("source_record_id IS NOT NULL"),
        postgresql_where=sa.text("source_record_id IS NOT NULL"),
    )

    op.create_table(
        "fair_price_bands",
        sa.Column("id", UUID(as_uuid=True), nullable=False),
        sa.Column("market_product", sa.String(length=64), nullable=False),
        sa.Column("delivery_point_id", UUID(as_uuid=True), sa.ForeignKey("delivery_points.id"), nullable=False),
        sa.Column("availability_window", sa.String(length=16), nullable=False),
        sa.Column("low_price_per_mt_usd", sa.Numeric(10, 2), nullable=False),
        sa.Column("mid_price_per_mt_usd", sa.Numeric(10, 2), nullable=False),
        sa.Column("high_price_per_mt_usd", sa.Numeric(10, 2), nullable=False),
        sa.Column("model_name", sa.String(length=64), nullable=False),
        sa.Column("model_version", sa.String(length=64), nullable=True),
        sa.Column("source", sa.String(length=64), nullable=False),
        sa.Column("source_event_id", sa.String(length=128), nullable=True),
        sa.Column(
            "trusted_ingestion_run_id",
            UUID(as_uuid=True),
            sa.ForeignKey("market_signal_ingestion_runs.id"),
            nullable=True,
        ),
        sa.Column("is_demo", sa.Boolean(), nullable=False, server_default=sa.text("true")),
        sa.Column("is_verified_real", sa.Boolean(), nullable=False, server_default=sa.text("false")),
        sa.Column("verified_real_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("observed_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
        sa.CheckConstraint(MARKET_PRODUCT_CHECK, name="ck_fair_price_bands_market_product"),
        sa.CheckConstraint(WINDOW_CHECK, name="ck_fair_price_bands_availability_window"),
        sa.CheckConstraint("low_price_per_mt_usd > 0", name="ck_fair_price_bands_low_positive"),
        sa.CheckConstraint("mid_price_per_mt_usd >= low_price_per_mt_usd", name="ck_fair_price_bands_mid_above_low"),
        sa.CheckConstraint("high_price_per_mt_usd >= mid_price_per_mt_usd", name="ck_fair_price_bands_high_above_mid"),
        sa.CheckConstraint(REAL_VERIFICATION_CHECK, name="ck_fair_price_bands_real_verification"),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index(
        "ix_fair_price_bands_board_lookup",
        "fair_price_bands",
        ["market_product", "delivery_point_id", "availability_window", "observed_at", "created_at", "id"],
    )
    op.create_index(
        "uq_fair_price_bands_source_event",
        "fair_price_bands",
        ["source", "source_event_id"],
        unique=True,
        sqlite_where=sa.text("source_event_id IS NOT NULL"),
        postgresql_where=sa.text("source_event_id IS NOT NULL"),
    )

    op.create_table(
        "physical_stems",
        sa.Column("id", UUID(as_uuid=True), nullable=False),
        sa.Column("market_product", sa.String(length=64), nullable=False),
        sa.Column("delivery_point_id", UUID(as_uuid=True), sa.ForeignKey("delivery_points.id"), nullable=False),
        sa.Column("availability_window", sa.String(length=16), nullable=False),
        sa.Column("quantity_mt", sa.Numeric(12, 2), nullable=False),
        sa.Column("stem_start", sa.DateTime(timezone=True), nullable=True),
        sa.Column("stem_end", sa.DateTime(timezone=True), nullable=True),
        sa.Column("status", sa.String(length=24), nullable=False),
        sa.Column("source", sa.String(length=64), nullable=False),
        sa.Column("stem_uid", sa.String(length=128), nullable=False),
        sa.Column("source_record_id", sa.String(length=128), nullable=True),
        sa.Column("source_event_id", sa.String(length=128), nullable=True),
        sa.Column(
            "trusted_ingestion_run_id",
            UUID(as_uuid=True),
            sa.ForeignKey("market_signal_ingestion_runs.id"),
            nullable=True,
        ),
        sa.Column("is_demo", sa.Boolean(), nullable=False, server_default=sa.text("true")),
        sa.Column("is_verified_real", sa.Boolean(), nullable=False, server_default=sa.text("false")),
        sa.Column("verified_real_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("observed_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
        sa.CheckConstraint(MARKET_PRODUCT_CHECK, name="ck_physical_stems_market_product"),
        sa.CheckConstraint(WINDOW_CHECK, name="ck_physical_stems_availability_window"),
        sa.CheckConstraint("quantity_mt > 0", name="ck_physical_stems_quantity_positive"),
        sa.CheckConstraint("status IN ('AVAILABLE', 'TENTATIVE', 'ALLOCATED', 'CANCELLED')", name="ck_physical_stems_status"),
        sa.CheckConstraint("stem_end IS NULL OR stem_start IS NULL OR stem_end >= stem_start", name="ck_physical_stems_date_order"),
        sa.CheckConstraint("stem_uid <> ''", name="ck_physical_stems_stem_uid_nonempty"),
        sa.CheckConstraint(REAL_VERIFICATION_CHECK, name="ck_physical_stems_real_verification"),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index(
        "ix_physical_stems_board_lookup",
        "physical_stems",
        ["market_product", "delivery_point_id", "availability_window", "observed_at", "created_at", "id"],
    )
    op.create_index(
        "ix_physical_stems_latest_uid",
        "physical_stems",
        ["market_product", "delivery_point_id", "availability_window", "source", "stem_uid", "observed_at", "created_at", "id"],
    )
    op.create_index(
        "uq_physical_stems_source_event",
        "physical_stems",
        ["source", "source_event_id"],
        unique=True,
        sqlite_where=sa.text("source_event_id IS NOT NULL"),
        postgresql_where=sa.text("source_event_id IS NOT NULL"),
    )
    op.create_index(
        "ix_physical_stems_source_record",
        "physical_stems",
        ["source", "source_record_id", "observed_at", "created_at", "id"],
        sqlite_where=sa.text("source_record_id IS NOT NULL"),
        postgresql_where=sa.text("source_record_id IS NOT NULL"),
    )


def downgrade() -> None:
    op.drop_index("ix_physical_stems_source_record", table_name="physical_stems")
    op.drop_index("uq_physical_stems_source_event", table_name="physical_stems")
    op.drop_index("ix_physical_stems_latest_uid", table_name="physical_stems")
    op.drop_index("ix_physical_stems_board_lookup", table_name="physical_stems")
    op.drop_table("physical_stems")
    op.drop_index("uq_fair_price_bands_source_event", table_name="fair_price_bands")
    op.drop_index("ix_fair_price_bands_board_lookup", table_name="fair_price_bands")
    op.drop_table("fair_price_bands")
    op.drop_index("ix_market_indications_source_record", table_name="market_indications")
    op.drop_index("uq_market_indications_source_event", table_name="market_indications")
    op.drop_index("ix_market_indications_latest_side", table_name="market_indications")
    op.drop_index("ix_market_indications_board_lookup", table_name="market_indications")
    op.drop_table("market_indications")
    op.drop_table("market_signal_ingestion_runs")

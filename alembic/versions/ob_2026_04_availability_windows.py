"""Canonicalize availability window codes for orderbook and RFQ records.

Revision ID: ob_2026_04_availability
Revises: neg_2026_04b
Create Date: 2026-04-08
"""

from alembic import op
import sqlalchemy as sa


revision = "ob_2026_04_availability"
down_revision = "neg_2026_04b"
branch_labels = None
depends_on = None


def _rewrite(table_name: str) -> None:
    op.execute(
        sa.text(
            f"""
            UPDATE {table_name}
            SET availability_window = CASE availability_window
                WHEN 'Spot' THEN 'SPOT'
                WHEN 'SPOT' THEN 'SPOT'
                WHEN 'Q1_2025' THEN '2025-Q1'
                WHEN 'Q2_2025' THEN '2025-Q2'
                WHEN 'Q3_2025' THEN '2025-Q3'
                WHEN 'Q4_2025' THEN '2025-Q4'
                WHEN 'Q1 2025' THEN '2025-Q1'
                WHEN 'Q2 2025' THEN '2025-Q2'
                WHEN 'Q3 2025' THEN '2025-Q3'
                WHEN 'Q4 2025' THEN '2025-Q4'
                WHEN 'Q1_2026' THEN '2026-Q1'
                WHEN 'Q2_2026' THEN '2026-Q2'
                WHEN 'Q3_2026' THEN '2026-Q3'
                WHEN 'Q4_2026' THEN '2026-Q4'
                WHEN 'Q1 2026' THEN '2026-Q1'
                WHEN 'Q2 2026' THEN '2026-Q2'
                WHEN 'Q3 2026' THEN '2026-Q3'
                WHEN 'Q4 2026' THEN '2026-Q4'
                WHEN 'FORWARD_2027' THEN '2027-CAL'
                WHEN 'FORWARD_2028' THEN '2028-CAL'
                WHEN 'Forward 2027' THEN '2027-CAL'
                WHEN 'Forward 2028' THEN '2028-CAL'
                ELSE availability_window
            END
            """
        )
    )


def upgrade() -> None:
    _rewrite("orderbook_orders")
    _rewrite("rfqs")

    op.alter_column(
        "orderbook_orders",
        "availability_window",
        existing_type=sa.String(length=50),
        server_default="SPOT",
        existing_nullable=True,
    )
    op.alter_column(
        "rfqs",
        "availability_window",
        existing_type=sa.String(),
        server_default="SPOT",
        existing_nullable=False,
    )


def downgrade() -> None:
    op.execute(
        sa.text(
            """
            UPDATE orderbook_orders
            SET availability_window = CASE availability_window
                WHEN 'SPOT' THEN 'Spot'
                WHEN '2025-Q1' THEN 'Q1_2025'
                WHEN '2025-Q2' THEN 'Q2_2025'
                WHEN '2025-Q3' THEN 'Q3_2025'
                WHEN '2025-Q4' THEN 'Q4_2025'
                WHEN '2026-Q1' THEN 'Q1_2026'
                WHEN '2026-Q2' THEN 'Q2_2026'
                WHEN '2026-Q3' THEN 'Q3_2026'
                WHEN '2026-Q4' THEN 'Q4_2026'
                WHEN '2027-CAL' THEN 'FORWARD_2027'
                WHEN '2028-CAL' THEN 'FORWARD_2028'
                ELSE availability_window
            END
            """
        )
    )
    op.execute(
        sa.text(
            """
            UPDATE rfqs
            SET availability_window = CASE availability_window
                WHEN 'SPOT' THEN 'Spot'
                WHEN '2025-Q1' THEN 'Q1_2025'
                WHEN '2025-Q2' THEN 'Q2_2025'
                WHEN '2025-Q3' THEN 'Q3_2025'
                WHEN '2025-Q4' THEN 'Q4_2025'
                WHEN '2026-Q1' THEN 'Q1_2026'
                WHEN '2026-Q2' THEN 'Q2_2026'
                WHEN '2026-Q3' THEN 'Q3_2026'
                WHEN '2026-Q4' THEN 'Q4_2026'
                WHEN '2027-CAL' THEN 'FORWARD_2027'
                WHEN '2028-CAL' THEN 'FORWARD_2028'
                ELSE availability_window
            END
            """
        )
    )

    op.alter_column(
        "orderbook_orders",
        "availability_window",
        existing_type=sa.String(length=50),
        server_default="Spot",
        existing_nullable=True,
    )
    op.alter_column(
        "rfqs",
        "availability_window",
        existing_type=sa.String(),
        server_default="Spot",
        existing_nullable=False,
    )

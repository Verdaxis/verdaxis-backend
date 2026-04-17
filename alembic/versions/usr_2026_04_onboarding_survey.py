"""Add onboarding survey fields to users

Revision ID: usr_2026_04_onboarding_survey
Revises: sb_2026_04_live_slice_benchmarks
Create Date: 2026-04-18

Two nullable columns capture a one-time post-verification survey:
  - onboarding_use_case: self-reported role (buyer / supplier / financier_other)
  - onboarding_referral_source: free-text attribution (how they heard about us)

Both are ADD COLUMN ... DEFAULT NULL — no table rewrite, no locking on Postgres 11+.
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op


revision = "usr_2026_04_onboarding_survey"
down_revision = "sb_2026_04_live_slice_benchmarks"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column(
        "users",
        sa.Column(
            "onboarding_use_case",
            sa.String(50),
            nullable=True,
            comment="Self-reported role from post-verification survey: buyer | supplier | financier_other",
        ),
    )
    op.add_column(
        "users",
        sa.Column(
            "onboarding_referral_source",
            sa.String(500),
            nullable=True,
            comment="Free-text attribution from post-verification survey",
        ),
    )


def downgrade() -> None:
    op.drop_column("users", "onboarding_referral_source")
    op.drop_column("users", "onboarding_use_case")

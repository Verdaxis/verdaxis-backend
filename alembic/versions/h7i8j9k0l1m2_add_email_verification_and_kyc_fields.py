"""add email verification and kyc fields

Revision ID: h7i8j9k0l1m2
Revises: g6h7i8j9k0l1
Create Date: 2026-03-03 10:00:00.000000

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


revision: str = "h7i8j9k0l1m2"
down_revision: Union[str, None] = "g6h7i8j9k0l1"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # Email verification fields (STORY-010a)
    op.add_column(
        "users",
        sa.Column(
            "email_verified",
            sa.Boolean(),
            nullable=False,
            server_default="false",
        ),
    )
    op.add_column(
        "users",
        sa.Column(
            "email_verification_token",
            sa.String(64),
            nullable=True,
        ),
    )

    # KYC fields (STORY-010b)
    op.add_column(
        "users",
        sa.Column(
            "kyc_status",
            sa.String(20),
            nullable=False,
            server_default="PENDING",
        ),
    )
    op.add_column(
        "users",
        sa.Column(
            "kyc_rejection_reason",
            sa.Text(),
            nullable=True,
        ),
    )


def downgrade() -> None:
    op.drop_column("users", "kyc_rejection_reason")
    op.drop_column("users", "kyc_status")
    op.drop_column("users", "email_verification_token")
    op.drop_column("users", "email_verified")

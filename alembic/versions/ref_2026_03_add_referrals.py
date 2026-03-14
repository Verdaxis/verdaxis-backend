"""add referral_code to users and referrals table

Revision ID: ref_2026_03
Revises: sub_2026_03
Create Date: 2026-03-14
"""
from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects.postgresql import UUID

revision = "ref_2026_03"
down_revision = "alerts_2026_03"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("users", sa.Column("referral_code", sa.String(10), nullable=True))
    op.add_column(
        "users",
        sa.Column("referred_by_id", UUID(as_uuid=True), sa.ForeignKey("users.id"), nullable=True),
    )
    op.create_unique_constraint("uq_users_referral_code", "users", ["referral_code"])
    op.create_index("ix_users_referral_code", "users", ["referral_code"])

    op.create_table(
        "referrals",
        sa.Column("id", UUID(as_uuid=True), nullable=False),
        sa.Column("referrer_id", UUID(as_uuid=True), nullable=False),
        sa.Column("referred_user_id", UUID(as_uuid=True), nullable=False),
        sa.Column("referral_code_used", sa.String(), nullable=False),
        sa.Column("status", sa.String(), nullable=False, server_default="SIGNED_UP"),
        sa.Column(
            "created_at", sa.DateTime(timezone=True),
            server_default=sa.text("now()"), nullable=False,
        ),
        sa.Column("verified_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("activated_at", sa.DateTime(timezone=True), nullable=True),
        sa.ForeignKeyConstraint(["referrer_id"], ["users.id"]),
        sa.ForeignKeyConstraint(["referred_user_id"], ["users.id"]),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint("referred_user_id", name="uq_referrals_referred_user"),
    )
    op.create_index("ix_referrals_referrer_id", "referrals", ["referrer_id"])


def downgrade() -> None:
    op.drop_index("ix_referrals_referrer_id", table_name="referrals")
    op.drop_table("referrals")
    op.drop_index("ix_users_referral_code", table_name="users")
    op.drop_constraint("uq_users_referral_code", "users", type_="unique")
    op.drop_column("users", "referred_by_id")
    op.drop_column("users", "referral_code")

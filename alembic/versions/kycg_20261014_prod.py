"""KYC execution gate: one-time grace snapshot for existing traders.

Revision ID: kycg_20261014_prod
Revises: blng_20261014_prod
Create Date: 2026-10-14

The bio-LNG revision (blng_20261014_prod) precedes this KYC snapshot.
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op
from sqlalchemy.dialects import postgresql


revision = "kycg_20261014_prod"
down_revision = "blng_20261014_prod"
branch_labels = None
depends_on = None

# Frozen locally: a migration must not import application policy.
# 14 days as an absolute duration. A '14 days' interval would follow the
# session time zone and gain or lose an hour across a DST change.
GRACE_PERIOD = "336 hours"


def upgrade() -> None:
    op.create_table(
        "kyc_execution_grace",
        sa.Column("id", postgresql.UUID(as_uuid=True), nullable=False),
        sa.Column("user_id", postgresql.UUID(as_uuid=True), nullable=False),
        sa.Column("organization_id", postgresql.UUID(as_uuid=True), nullable=False),
        sa.Column("kyc_status_at_enrollment", sa.String(length=20), nullable=False),
        sa.Column("enrolled_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("expires_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("created_by", sa.String(length=64), nullable=False),
        sa.CheckConstraint(
            "kyc_status_at_enrollment IN ('PENDING', 'SUBMITTED')",
            name="ck_kyc_execution_grace_enrollment_status",
        ),
        sa.CheckConstraint(
            f"expires_at = enrolled_at + interval '{GRACE_PERIOD}'",
            name="ck_kyc_execution_grace_period",
        ),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.ForeignKeyConstraint(
            ["organization_id"], ["organizations.id"], ondelete="CASCADE"
        ),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint(
            "user_id",
            "organization_id",
            name="uq_kyc_execution_grace_user_organization",
        ),
    )
    # The runtime role receives SELECT only, through the declarative ACL
    # policy that deploy convergence applies after this revision. PUBLIC
    # never holds authority on the snapshot.
    op.execute(sa.text("REVOKE ALL ON kyc_execution_grace FROM PUBLIC"))

    # One snapshot with one enrollment instant: now() is the transaction
    # start, so every row shares the same enrolled_at and expires_at.
    op.execute(
        sa.text(
            f"""
            INSERT INTO kyc_execution_grace (
                id, user_id, organization_id, kyc_status_at_enrollment,
                enrolled_at, expires_at, created_by
            )
            SELECT gen_random_uuid(), users.id, users.organization_id,
                   users.kyc_status, now(), now() + interval '{GRACE_PERIOD}',
                   :revision
            FROM users
            JOIN organizations ON organizations.id = users.organization_id
            WHERE users.status = 'APPROVED'
              AND organizations.verification_status = 'APPROVED'
              AND users.kyc_status IN ('PENDING', 'SUBMITTED')
            """
        ).bindparams(revision=revision)
    )


def downgrade() -> None:
    bind = op.get_bind()
    # The snapshot has users and organizations FKs. Lock all three relations
    # up front so removal fails fast instead of waiting behind active writes.
    bind.execute(
        sa.text(
            "LOCK TABLE kyc_execution_grace, users, organizations "
            "IN ACCESS EXCLUSIVE MODE NOWAIT"
        )
    )
    op.drop_table("kyc_execution_grace")

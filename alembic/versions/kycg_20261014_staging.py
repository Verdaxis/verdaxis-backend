"""Snapshot fourteen-day organization-bound KYC execution grace.

Revision ID: kycg_20261014_staging
Revises: rcp_20261003_command_results
"""
import sqlalchemy as sa
from sqlalchemy.dialects import postgresql
from alembic import op

revision = "kycg_20261014_staging"
down_revision = "rcp_20261003_command_results"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "kyc_execution_grace",
        sa.Column("id", postgresql.UUID(as_uuid=True), nullable=False),
        sa.Column("user_id", postgresql.UUID(as_uuid=True), nullable=False),
        sa.Column("organization_id", postgresql.UUID(as_uuid=True), nullable=False),
        sa.Column("kyc_status_at_enrollment", sa.String(20), nullable=False),
        sa.Column("enrolled_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("expires_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("created_by", sa.String(64), nullable=False),
        sa.PrimaryKeyConstraint("id"),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="RESTRICT"),
        sa.ForeignKeyConstraint(["organization_id"], ["organizations.id"], ondelete="RESTRICT"),
        sa.UniqueConstraint("user_id", "organization_id", name="uq_kyc_execution_grace_user_org"),
    )
    op.create_table(
        "kyc_grace_notices",
        sa.Column("grace_id", postgresql.UUID(as_uuid=True), nullable=False),
        sa.Column("claimed_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("sent_at", sa.DateTime(timezone=True), nullable=True),
        sa.PrimaryKeyConstraint("grace_id"),
        sa.ForeignKeyConstraint(["grace_id"], ["kyc_execution_grace.id"], ondelete="RESTRICT"),
    )
    # PostgreSQL now() is fixed at transaction start: every enrolled user gets
    # the same fourteen-day deadline, independent of subsequent uploads.
    op.execute("""
        INSERT INTO kyc_execution_grace
            (id, user_id, organization_id, kyc_status_at_enrollment,
             enrolled_at, expires_at, created_by)
        SELECT gen_random_uuid(), u.id, u.organization_id, u.kyc_status,
               now(), now() + interval '336 hours', 'kycg_20261014_staging'
        FROM users u JOIN organizations o ON o.id = u.organization_id
        WHERE u.status = 'APPROVED' AND o.verification_status = 'APPROVED'
          AND u.kyc_status IN ('PENDING', 'SUBMITTED')
    """)
    # Freeze the deployed identities locally; migrations must not import policy.
    op.execute("""
        DO $$ BEGIN
          IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'verdaxis_app_staging') THEN
            REVOKE ALL ON kyc_execution_grace FROM verdaxis_app_staging;
            GRANT SELECT ON kyc_execution_grace TO verdaxis_app_staging;
          END IF;
        END $$;
    """)


def downgrade() -> None:
    op.execute("LOCK TABLE kyc_execution_grace IN ACCESS EXCLUSIVE MODE NOWAIT")
    op.drop_table("kyc_grace_notices")
    op.drop_table("kyc_execution_grace")

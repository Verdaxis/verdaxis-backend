"""Retain application-encrypted KYC documents.

Revision ID: kycd_20261015_staging
Revises: kycg_20261014_staging
"""
import sqlalchemy as sa

from alembic import op

revision = "kycd_20261015_staging"
down_revision = "kycg_20261014_staging"
branch_labels = None
depends_on = None


def upgrade():
    op.create_table(
        "kyc_documents",
        sa.Column("id", sa.Uuid(), primary_key=True),
        sa.Column("submission_id", sa.Uuid(), nullable=False),
        sa.Column("user_id", sa.Uuid(), sa.ForeignKey("users.id", ondelete="RESTRICT"), nullable=False),
        sa.Column("organization_id", sa.Uuid(), sa.ForeignKey("organizations.id", ondelete="RESTRICT"), nullable=False),
        sa.Column("kind", sa.Text(), nullable=False),
        sa.Column("content_type", sa.Text(), nullable=False),
        sa.Column("size_bytes", sa.Integer(), nullable=False),
        sa.Column("key_id", sa.Text(), nullable=False),
        sa.Column("wrapped_key", sa.LargeBinary(), nullable=False),
        sa.Column("key_nonce", sa.LargeBinary(), nullable=False),
        sa.Column("data_nonce", sa.LargeBinary(), nullable=False),
        sa.Column("ciphertext", sa.LargeBinary(), nullable=False),
        sa.Column("created_at", sa.DateTime(timezone=True), server_default=sa.func.now(), nullable=False),
        sa.CheckConstraint("kind IN ('IDENTITY', 'COMPANY_REGISTRATION')", name="ck_kyc_documents_kind"),
        sa.CheckConstraint("content_type IN ('image/jpeg', 'image/png', 'application/pdf')", name="ck_kyc_documents_content_type"),
        sa.CheckConstraint("size_bytes > 0", name="ck_kyc_documents_size"),
    )
    op.create_index("ix_kyc_documents_user_created", "kyc_documents", ["user_id", "created_at"])
    op.execute("REVOKE ALL ON TABLE kyc_documents FROM PUBLIC")


def downgrade():
    # An empty disposable schema can downgrade; retained evidence cannot.
    connection = op.get_bind()
    connection.execute(sa.text("LOCK TABLE kyc_documents IN ACCESS EXCLUSIVE MODE"))
    if connection.execute(sa.text("SELECT EXISTS (SELECT 1 FROM kyc_documents)")).scalar():
        raise RuntimeError("Cannot downgrade while KYC documents are retained")
    op.drop_table("kyc_documents")

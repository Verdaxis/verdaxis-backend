"""Add password reset tokens owned by the Rust credential service.

Additive only: one table and one index, one row per user. Python does not read
or write this table; the shared users.password_reset_token_hash column stays
with the invitation flow. Downgrade drops both objects.
"""
import sqlalchemy as sa
from sqlalchemy.dialects import postgresql

from alembic import op

revision = "p3_20261020_auth_tokens"
down_revision = "prc_20261026_changes"
branch_labels = None
depends_on = None


def upgrade():
    # The foreign key needs SHARE ROW EXCLUSIVE on users. Take it without
    # waiting, so the migration fails fast behind an active user write instead
    # of queueing behind it and blocking every later user write.
    op.execute("LOCK TABLE public.users IN SHARE ROW EXCLUSIVE MODE NOWAIT")
    op.create_table(
        "password_reset_tokens",
        sa.Column("user_id", postgresql.UUID(as_uuid=True), primary_key=True),
        sa.Column("token_hash", sa.Text(), nullable=False),
        sa.Column("expires_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column(
            "created_at",
            sa.DateTime(timezone=True),
            server_default=sa.text("now()"),
            nullable=False,
        ),
        sa.Column("used_at", sa.DateTime(timezone=True), nullable=True),
        sa.ForeignKeyConstraint(
            ["user_id"],
            ["users.id"],
            name="fk_password_reset_tokens_user_id_users",
            ondelete="CASCADE",
        ),
        sa.UniqueConstraint("token_hash", name="uq_password_reset_tokens_token_hash"),
    )
    op.create_index(
        "ix_password_reset_tokens_expires_at", "password_reset_tokens", ["expires_at"]
    )


def downgrade():
    # Removing the foreign key also locks users; refuse instead of queueing.
    op.execute("LOCK TABLE public.users, public.password_reset_tokens IN ACCESS EXCLUSIVE MODE NOWAIT")
    op.drop_index("ix_password_reset_tokens_expires_at", table_name="password_reset_tokens")
    op.drop_table("password_reset_tokens")

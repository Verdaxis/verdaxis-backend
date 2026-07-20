"""Add hashed email verification expiry and refresh sessions.

The legacy email_verification_token column is retained for exactly one
compatibility window. Existing plaintext values are hashed without logging
and expire 24 hours from this migration. The next linear revision refuses to
drop the column until that window has elapsed.
"""

from datetime import UTC, datetime, timedelta
import hashlib
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects import postgresql


revision: str = "sec_20260720_identity"
down_revision: Union[str, None] = "rh_20260720_runtime_metadata"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.add_column("users", sa.Column("email_verification_token_hash", sa.String(64), nullable=True))
    op.add_column("users", sa.Column("email_verification_token_expires_at", sa.DateTime(timezone=True), nullable=True))
    op.create_index("ix_users_email_verification_token_hash", "users", ["email_verification_token_hash"])
    bind = op.get_bind()
    compatibility_deadline = datetime.now(UTC) + timedelta(hours=24)
    rows = bind.execute(
        sa.text(
            "SELECT id, email_verification_token FROM users "
            "WHERE email_verification_token IS NOT NULL"
        )
    )
    while batch := rows.fetchmany(500):
        for row in batch:
            bind.execute(
                sa.text(
                    "UPDATE users SET email_verification_token_hash = :token_hash, "
                    "email_verification_token_expires_at = :expires_at WHERE id = :user_id"
                ),
                {
                    "token_hash": hashlib.sha256(row.email_verification_token.encode("utf-8")).hexdigest(),
                    "expires_at": compatibility_deadline,
                    "user_id": row.id,
                },
            )
    op.create_table(
        "refresh_sessions",
        sa.Column("id", postgresql.UUID(as_uuid=True), primary_key=True),
        sa.Column("user_id", postgresql.UUID(as_uuid=True), sa.ForeignKey("users.id"), nullable=False),
        sa.Column("family_id", postgresql.UUID(as_uuid=True), nullable=False),
        sa.Column("jti_hash", sa.String(64), nullable=False, unique=True),
        sa.Column("replaced_by_jti_hash", sa.String(64), nullable=True),
        sa.Column("revoked", sa.Boolean(), nullable=False, server_default=sa.false()),
        sa.Column("expires_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("created_at", sa.DateTime(timezone=True), server_default=sa.func.now(), nullable=False),
        sa.Column("last_used_at", sa.DateTime(timezone=True), nullable=True),
    )
    op.create_index("ix_refresh_sessions_user_id", "refresh_sessions", ["user_id"])
    op.create_index("ix_refresh_sessions_family_id", "refresh_sessions", ["family_id"])
    op.create_index("ix_refresh_sessions_expires_at", "refresh_sessions", ["expires_at"])
    op.create_index(
        "ix_users_email_verification_token_expires_at",
        "users",
        ["email_verification_token_expires_at"],
    )
    op.create_index("ix_users_password_reset_expires", "users", ["password_reset_expires"])
    # jti_hash is already covered by the table's single UNIQUE constraint.
    # Do not add a second unique index for the same key.


def downgrade() -> None:
    op.drop_index("ix_users_password_reset_expires", table_name="users")
    op.drop_index("ix_users_email_verification_token_expires_at", table_name="users")
    op.drop_index("ix_refresh_sessions_expires_at", table_name="refresh_sessions")
    op.drop_index("ix_refresh_sessions_family_id", table_name="refresh_sessions")
    op.drop_index("ix_refresh_sessions_user_id", table_name="refresh_sessions")
    op.drop_table("refresh_sessions")
    op.drop_index("ix_users_email_verification_token_hash", table_name="users")
    op.drop_column("users", "email_verification_token_expires_at")
    op.drop_column("users", "email_verification_token_hash")

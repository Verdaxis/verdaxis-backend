"""Add hashed email verification expiry and refresh sessions.

The legacy email_verification_token column is retained for a compatibility
window. Existing and newly written plaintext values are hashed without logging
and expire 24 hours after their latest write. The next linear revision refuses
to drop the column while any compatibility link remains active.
"""

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
    op.execute(
        sa.text(
            "UPDATE users SET "
            "email_verification_token_hash = encode("
            "sha256(convert_to(email_verification_token, 'UTF8')), 'hex'), "
            "email_verification_token_expires_at = CURRENT_TIMESTAMP + interval '24 hours' "
            "WHERE email_verification_token IS NOT NULL"
        )
    )
    op.execute(
        sa.text(
            """
            CREATE FUNCTION verdaxis_sync_legacy_verification_token()
            RETURNS trigger
            LANGUAGE plpgsql
            AS $$
            BEGIN
                IF TG_OP = 'INSERT'
                   OR NEW.email_verification_token IS DISTINCT FROM OLD.email_verification_token THEN
                    IF NEW.email_verification_token IS NULL THEN
                        NEW.email_verification_token_hash := NULL;
                        NEW.email_verification_token_expires_at := NULL;
                    ELSE
                        NEW.email_verification_token_hash := encode(
                            sha256(convert_to(NEW.email_verification_token, 'UTF8')), 'hex'
                        );
                        NEW.email_verification_token_expires_at :=
                            CURRENT_TIMESTAMP + interval '24 hours';
                    END IF;
                END IF;
                RETURN NEW;
            END
            $$
            """
        )
    )
    op.execute(
        sa.text(
            "CREATE TRIGGER trg_users_sync_legacy_verification_token "
            "BEFORE INSERT OR UPDATE OF email_verification_token ON users "
            "FOR EACH ROW EXECUTE FUNCTION verdaxis_sync_legacy_verification_token()"
        )
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
    op.execute(sa.text("DROP TRIGGER trg_users_sync_legacy_verification_token ON users"))
    op.execute(sa.text("DROP FUNCTION verdaxis_sync_legacy_verification_token()"))
    op.drop_index("ix_users_password_reset_expires", table_name="users")
    op.drop_index("ix_users_email_verification_token_expires_at", table_name="users")
    op.drop_index("ix_refresh_sessions_expires_at", table_name="refresh_sessions")
    op.drop_index("ix_refresh_sessions_family_id", table_name="refresh_sessions")
    op.drop_index("ix_refresh_sessions_user_id", table_name="refresh_sessions")
    op.drop_table("refresh_sessions")
    op.drop_index("ix_users_email_verification_token_hash", table_name="users")
    op.drop_column("users", "email_verification_token_expires_at")
    op.drop_column("users", "email_verification_token_hash")

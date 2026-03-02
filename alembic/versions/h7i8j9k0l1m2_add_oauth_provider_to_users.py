"""add oauth_provider to users

Revision ID: h7i8j9k0l1m2
Revises: g6h7i8j9k0l1
Create Date: 2026-03-02 21:00:00.000000

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


revision: str = "h7i8j9k0l1m2"
down_revision: Union[str, None] = "g6h7i8j9k0l1"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.add_column("users", sa.Column("oauth_provider", sa.String(20), nullable=True))
    # Make password_hash nullable for OAuth users who authenticate externally
    op.alter_column("users", "password_hash", existing_type=sa.String(), nullable=True)


def downgrade() -> None:
    # Restore NOT NULL on password_hash (set empty string for any nulls first)
    op.execute("UPDATE users SET password_hash = '' WHERE password_hash IS NULL")
    op.alter_column("users", "password_hash", existing_type=sa.String(), nullable=False)
    op.drop_column("users", "oauth_provider")

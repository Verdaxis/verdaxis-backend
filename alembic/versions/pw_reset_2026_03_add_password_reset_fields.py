"""add password reset fields

Revision ID: pw_reset_2026_03
Revises: h7i8j9k0l1m2
Create Date: 2026-03-05

"""
from alembic import op
import sqlalchemy as sa

revision = 'pw_reset_2026_03'
down_revision = 'h7i8j9k0l1m2'
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column('users', sa.Column('password_reset_token_hash', sa.String(64), nullable=True))
    op.add_column('users', sa.Column('password_reset_expires', sa.DateTime(timezone=True), nullable=True))


def downgrade() -> None:
    op.drop_column('users', 'password_reset_expires')
    op.drop_column('users', 'password_reset_token_hash')

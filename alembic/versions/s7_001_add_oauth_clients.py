"""add oauth_clients table

Revision ID: s7_001_add_oauth_clients
Revises: s6_001_surveillance_events
Create Date: 2026-03-19

Creates the oauth_clients table for the OAuth2 client_credentials flow.
Uses UUID columns (VARCHAR storage for SQLite compat).
"""
from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects import postgresql

# revision identifiers, used by Alembic.
revision = "s7_001_add_oauth_clients"
down_revision = "s6_001_surveillance_events"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "oauth_clients",
        sa.Column(
            "id",
            postgresql.UUID(as_uuid=True),
            primary_key=True,
            nullable=False,
        ),
        sa.Column(
            "client_id",
            postgresql.UUID(as_uuid=True),
            unique=True,
            nullable=False,
        ),
        sa.Column("client_secret_hash", sa.String(), nullable=False),
        sa.Column("name", sa.String(length=100), nullable=False),
        sa.Column(
            "scopes",
            postgresql.JSON(astext_type=sa.Text()),
            nullable=False,
            server_default="[]",
        ),
        sa.Column(
            "rate_limit_tier",
            sa.String(length=20),
            nullable=False,
            server_default="free",
        ),
        sa.Column(
            "created_by",
            postgresql.UUID(as_uuid=True),
            sa.ForeignKey("users.id"),
            nullable=False,
        ),
        sa.Column(
            "created_at",
            sa.DateTime(timezone=True),
            nullable=False,
            server_default=sa.func.now(),
        ),
    )
    op.create_index("ix_oauth_clients_created_by", "oauth_clients", ["created_by"])


def downgrade() -> None:
    op.drop_index("ix_oauth_clients_created_by", table_name="oauth_clients")
    op.drop_table("oauth_clients")

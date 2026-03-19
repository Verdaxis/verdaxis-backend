"""add dashboards and dashboard_widgets tables

Revision ID: s8_001_add_dashboard_widgets
Revises: s7_001_add_oauth_clients
Create Date: 2026-03-19

Creates the dashboards and dashboard_widgets tables for the platform
stickiness sprint. Also adds TRADER to the userrole enum.
Uses VARCHAR columns for UUIDs (SQLite compat).
"""
from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects import postgresql

# revision identifiers, used by Alembic.
revision = "s8_001_add_dashboard_widgets"
down_revision = "s7_001_add_oauth_clients"
branch_labels = None
depends_on = None


def upgrade() -> None:
    # Add TRADER to userrole enum (PostgreSQL only — SQLite uses VARCHAR so no-op)
    op.execute("DO $$ BEGIN "
               "  IF NOT EXISTS (SELECT 1 FROM pg_type t JOIN pg_enum e ON t.oid = e.enumtypid "
               "                  WHERE t.typname = 'userrole' AND e.enumlabel = 'TRADER') THEN "
               "    ALTER TYPE userrole ADD VALUE 'TRADER'; "
               "  END IF; "
               "END $$;")

    op.create_table(
        "dashboards",
        sa.Column(
            "id",
            postgresql.UUID(as_uuid=True),
            primary_key=True,
            nullable=False,
        ),
        sa.Column(
            "user_id",
            postgresql.UUID(as_uuid=True),
            sa.ForeignKey("users.id"),
            nullable=False,
        ),
        sa.Column("name", sa.String(length=100), nullable=False),
        sa.Column(
            "layout",
            postgresql.JSON(astext_type=sa.Text()),
            nullable=True,
            server_default="{}",
        ),
        sa.Column("is_default", sa.Boolean(), nullable=False, server_default="false"),
        sa.Column(
            "created_at",
            sa.DateTime(timezone=True),
            nullable=False,
            server_default=sa.func.now(),
        ),
        sa.Column(
            "updated_at",
            sa.DateTime(timezone=True),
            nullable=False,
            server_default=sa.func.now(),
        ),
    )
    op.create_index("ix_dashboards_user_id", "dashboards", ["user_id"])

    op.create_table(
        "dashboard_widgets",
        sa.Column(
            "id",
            postgresql.UUID(as_uuid=True),
            primary_key=True,
            nullable=False,
        ),
        sa.Column(
            "dashboard_id",
            postgresql.UUID(as_uuid=True),
            sa.ForeignKey("dashboards.id", ondelete="CASCADE"),
            nullable=False,
        ),
        sa.Column("widget_type", sa.String(length=50), nullable=False),
        sa.Column(
            "config",
            postgresql.JSON(astext_type=sa.Text()),
            nullable=True,
            server_default="{}",
        ),
        sa.Column(
            "position",
            postgresql.JSON(astext_type=sa.Text()),
            nullable=True,
            server_default="{}",
        ),
        sa.Column(
            "created_at",
            sa.DateTime(timezone=True),
            nullable=False,
            server_default=sa.func.now(),
        ),
    )
    op.create_index("ix_dashboard_widgets_dashboard_id", "dashboard_widgets", ["dashboard_id"])


def downgrade() -> None:
    op.drop_index("ix_dashboard_widgets_dashboard_id", table_name="dashboard_widgets")
    op.drop_table("dashboard_widgets")
    op.drop_index("ix_dashboards_user_id", table_name="dashboards")
    op.drop_table("dashboards")
    # Note: PostgreSQL enums cannot easily remove values; TRADER removal omitted

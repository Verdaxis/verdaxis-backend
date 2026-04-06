"""add missing negotiation columns, constraints, and indexes

Revision ID: neg_2026_04b
Revises: neg_2026_04
Create Date: 2026-04-06
"""
from alembic import op
import sqlalchemy as sa

revision = "neg_2026_04b"
down_revision = "neg_2026_04"
branch_labels = None
depends_on = None


def upgrade() -> None:
    # updated_at — needed for audit, cache invalidation, debugging
    op.add_column(
        "negotiations",
        sa.Column(
            "updated_at",
            sa.DateTime(timezone=True),
            server_default=sa.func.now(),
            nullable=False,
        ),
    )

    # initiator_side — "BUYER" or "SELLER", required for correct trade role assignment
    op.add_column(
        "negotiations",
        sa.Column("initiator_side", sa.String(10), nullable=False, server_default="BUYER"),
    )
    # Remove the temporary server_default once backfill is done (handled app-side)

    # Unique constraint on rounds to prevent duplicate round numbers
    op.create_unique_constraint(
        "uq_neg_rounds_negotiation_round",
        "negotiation_rounds",
        ["negotiation_id", "round_number"],
    )

    # Partial index for expiry sweep jobs (only active negotiations need this)
    op.execute(
        """
        CREATE INDEX ix_negotiations_expires_at
        ON negotiations (expires_at)
        WHERE status IN ('OPEN', 'COUNTERED')
        """
    )

    # Composite index for filtered + sorted listing
    op.create_index(
        "ix_negotiations_status_created",
        "negotiations",
        ["status", sa.text("created_at DESC")],
    )


def downgrade() -> None:
    op.drop_index("ix_negotiations_status_created")
    op.execute("DROP INDEX IF EXISTS ix_negotiations_expires_at")
    op.drop_constraint("uq_neg_rounds_negotiation_round", "negotiation_rounds")
    op.drop_column("negotiations", "initiator_side")
    op.drop_column("negotiations", "updated_at")

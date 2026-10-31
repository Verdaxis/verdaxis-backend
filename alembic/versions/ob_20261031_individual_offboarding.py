"""Add the individual account state marker and its append-only history.

Revision ID: ob_20261031_offboarding
Revises: p5_20261020_read_indexes
Create Date: 2026-10-31

``users.offboarded_at`` is nullable. A user with no marker is unchanged. The
status enum gains no value: a marked user keeps the existing ``REJECTED``
value, so every reader that knows only the current statuses keeps working.
Two named CHECK constraints make a marked row ``REJECTED`` and detached from
any organization, so an ordinary approval or join cannot restore a marked
account while the marker is present.

``individual_offboardings`` is the append-only history of each action. Its
foreign keys restrict deletion of the people and the organization it names.
The reason code and the case reference are bounded here to a shape; the exact
reason allowlist and reference grammar belong to the application.

The downgrade refuses while any marker or record exists, because dropping
either would restore access or erase evidence.
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op

revision = "ob_20261031_offboarding"
down_revision = "p5_20261020_read_indexes"
branch_labels = None
depends_on = None

REJECTED_CONSTRAINT = "ck_users_offboarded_requires_rejected"
DETACHED_CONSTRAINT = "ck_users_offboarded_requires_detached"


def upgrade() -> None:
    # Take every lock first and without waiting, so the migration fails fast
    # behind an active user or organization write instead of queueing behind
    # it and blocking later writers. The column needs ACCESS EXCLUSIVE on
    # users; the foreign keys need SHARE ROW EXCLUSIVE on both parents.
    op.execute("LOCK TABLE public.users IN ACCESS EXCLUSIVE MODE NOWAIT")
    op.execute("LOCK TABLE public.organizations IN SHARE ROW EXCLUSIVE MODE NOWAIT")

    # NULL keeps every existing row exactly as it is; there is no backfill.
    op.add_column(
        "users",
        sa.Column("offboarded_at", sa.DateTime(timezone=True), nullable=True),
    )
    op.create_check_constraint(
        REJECTED_CONSTRAINT, "users", "offboarded_at IS NULL OR status = 'REJECTED'"
    )
    op.create_check_constraint(
        DETACHED_CONSTRAINT, "users", "offboarded_at IS NULL OR organization_id IS NULL"
    )

    op.create_table(
        "individual_offboardings",
        sa.Column("operation_id", sa.Uuid(), primary_key=True),
        sa.Column("subject_user_id", sa.Uuid(), nullable=False),
        sa.Column("former_organization_id", sa.Uuid(), nullable=True),
        sa.Column("actor_user_id", sa.Uuid(), nullable=False),
        sa.Column(
            "occurred_at",
            sa.DateTime(timezone=True),
            server_default=sa.func.now(),
            nullable=False,
        ),
        sa.Column("reason_code", sa.Text(), nullable=False),
        sa.Column("evidence_case_ref", sa.Text(), nullable=False),
        sa.Column("urgent", sa.Boolean(), server_default=sa.false(), nullable=False),
        sa.ForeignKeyConstraint(
            ["subject_user_id"],
            ["users.id"],
            name="fk_individual_offboardings_subject_user_id_users",
            ondelete="RESTRICT",
        ),
        sa.ForeignKeyConstraint(
            ["former_organization_id"],
            ["organizations.id"],
            name="fk_individual_offboardings_former_organization_id_organizations",
            ondelete="RESTRICT",
        ),
        sa.ForeignKeyConstraint(
            ["actor_user_id"],
            ["users.id"],
            name="fk_individual_offboardings_actor_user_id_users",
            ondelete="RESTRICT",
        ),
        sa.CheckConstraint(
            "subject_user_id <> actor_user_id",
            name="ck_individual_offboardings_not_self",
        ),
        # Upper-case words joined by underscores, at most 48 characters.
        sa.CheckConstraint(
            "reason_code ~ '^[A-Z][A-Z0-9_]{1,47}$'",
            name="ck_individual_offboardings_reason_code",
        ),
        # An opaque reference: 1 to 64 printable ASCII characters, no spaces.
        sa.CheckConstraint(
            "evidence_case_ref ~ '^[!-~]{1,64}$'",
            name="ck_individual_offboardings_case_ref",
        ),
    )
    op.create_index(
        "ix_individual_offboardings_subject_time",
        "individual_offboardings",
        ["subject_user_id", "occurred_at"],
    )


def downgrade() -> None:
    # Dropping the marker would restore access to marked accounts, and dropping
    # the table would erase the evidence. Refuse once either has ever been
    # persisted. Take every lock before reading the guard; NOWAIT fails safely
    # instead of waiting behind runtime traffic.
    bind = op.get_bind()
    bind.execute(
        sa.text(
            "LOCK TABLE public.users, public.organizations, public.individual_offboardings "
            "IN ACCESS EXCLUSIVE MODE NOWAIT"
        )
    )
    retained = bind.execute(
        sa.text(
            "SELECT EXISTS (SELECT 1 FROM individual_offboardings) "
            "OR EXISTS (SELECT 1 FROM users WHERE offboarded_at IS NOT NULL)"
        )
    ).scalar_one()
    if retained:
        raise RuntimeError(
            "cannot downgrade individual offboarding while markers or records exist"
        )
    op.drop_index(
        "ix_individual_offboardings_subject_time", table_name="individual_offboardings"
    )
    op.drop_table("individual_offboardings")
    op.drop_constraint(DETACHED_CONSTRAINT, "users", type_="check")
    op.drop_constraint(REJECTED_CONSTRAINT, "users", type_="check")
    op.drop_column("users", "offboarded_at")

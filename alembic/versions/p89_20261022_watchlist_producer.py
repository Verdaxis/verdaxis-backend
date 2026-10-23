"""Add producer identity, internal source evidence and a consumer watermark."""

import sqlalchemy as sa
from sqlalchemy.dialects import postgresql

from alembic import op

revision = "p89_20261022_watchlist_producer"
down_revision = "prc_20261018_cache"
branch_labels = None
depends_on = None


def upgrade():
    heads = op.get_bind().execute(sa.text("SELECT version_num FROM alembic_version")).scalars().all()
    if len(heads) != 1:
        raise RuntimeError("Producer schema requires one migration head")

    op.add_column(
        "watchlist_events",
        sa.Column(
            "producer", sa.String(32),
            sa.CheckConstraint("producer IN ('legacy-python', 'rust-market-v1')"),
            nullable=False, server_default="legacy-python",
        ),
    )
    op.add_column("watchlist_events", sa.Column("source_event_id", sa.UUID(), nullable=True))
    op.add_column("watchlist_events", sa.Column("source_stream_seq", sa.BigInteger(), nullable=True))
    op.add_column("watchlist_events", sa.Column("scope_attested_at", sa.DateTime(timezone=True), nullable=True))
    op.add_column("watchlist_events", sa.Column("derived_event_key", sa.String(255), nullable=True))
    op.add_column("market_event_outbox", sa.Column("producer_evidence", postgresql.JSONB(), nullable=True))
    op.create_table(
        "watchlist_producer_state",
        sa.Column("singleton", sa.Boolean(), sa.CheckConstraint("singleton"), primary_key=True, nullable=False),
        sa.Column(
            "consumed_through_seq", sa.BigInteger(),
            sa.CheckConstraint("consumed_through_seq >= 0"), nullable=False, server_default="0",
        ),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.text("now()")),
    )
    op.create_check_constraint(
        "ck_watchlist_events_producer_identity", "watchlist_events",
        "(producer = 'legacy-python' AND source_event_id IS NULL AND source_stream_seq IS NULL AND scope_attested_at IS NULL AND derived_event_key IS NULL) OR (producer = 'rust-market-v1' AND source_event_id IS NOT NULL AND source_stream_seq IS NOT NULL AND scope_attested_at IS NOT NULL AND derived_event_key IS NOT NULL AND char_length(derived_event_key) > 0)",
    )
    op.create_index(
        "uq_watchlist_events_producer_source_target_derived", "watchlist_events",
        ["producer", "source_event_id", "watchlist_target_id", "derived_event_key"],
        unique=True, postgresql_where=sa.text("source_event_id IS NOT NULL"),
    )
    op.execute("UPDATE watchlist_events SET producer = 'legacy-python'")

    heads = op.get_bind().execute(sa.text("SELECT version_num FROM alembic_version")).scalars().all()
    if len(heads) != 1:
        raise RuntimeError("Producer schema requires one migration head")


def downgrade():
    trusted_rows = op.get_bind().execute(sa.text("SELECT EXISTS (SELECT 1 FROM watchlist_events WHERE producer = 'rust-market-v1')")).scalar_one()
    producer_evidence = op.get_bind().execute(sa.text("SELECT EXISTS (SELECT 1 FROM market_event_outbox WHERE producer_evidence IS NOT NULL)")).scalar_one()
    if trusted_rows or producer_evidence:
        raise RuntimeError("Retained producer evidence prevents downgrade")
    op.drop_index("uq_watchlist_events_producer_source_target_derived", table_name="watchlist_events")
    op.drop_table("watchlist_producer_state")
    op.drop_column("market_event_outbox", "producer_evidence")
    op.drop_column("watchlist_events", "derived_event_key")
    op.drop_column("watchlist_events", "scope_attested_at")
    op.drop_column("watchlist_events", "source_stream_seq")
    op.drop_column("watchlist_events", "source_event_id")
    op.drop_column("watchlist_events", "producer")

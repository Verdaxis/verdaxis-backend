"""Shared SSE transport: durable stream sequencing for the market outbox.

Revision ID: sse_20260720_market_event_stream
Revises: mi_20260720_market_integrity

Adds the delivery-side contract for the durable shared SSE dispatcher:

- ``market_event_stream_seq`` — a global monotonic sequence. A single
  leader (PostgreSQL advisory lock) assigns values to committed outbox rows
  AFTER their producing transactions commit, so the visible maximum only
  grows and a subscriber cursor of ``stream_seq > last_seen`` can never skip
  a row that becomes visible later. Holes (crashed assignment transactions)
  are permitted and carry no meaning.
- ``market_event_outbox.stream_seq`` — nullable until assigned, unique once
  assigned. Rows with NULL ``stream_seq`` are pending dispatch.
- A partial index over pending rows for the sequencer claim query.

All expressions are immutable migration-local literals. Importing this file
does not import application configuration or require secrets.
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op

revision = "sse_20260720_market_event_stream"
down_revision = "mi_20260720_market_integrity"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.execute("CREATE SEQUENCE market_event_stream_seq")
    op.add_column(
        "market_event_outbox",
        sa.Column("stream_seq", sa.BigInteger(), nullable=True),
    )
    op.create_unique_constraint(
        "uq_market_event_outbox_stream_seq",
        "market_event_outbox",
        ["stream_seq"],
    )
    op.create_index(
        "ix_market_event_outbox_unsequenced",
        "market_event_outbox",
        ["created_at"],
        postgresql_where=sa.text("stream_seq IS NULL"),
    )


def downgrade() -> None:
    # Dropping the sequence/column would invalidate every subscriber's
    # Last-Event-ID cursor and could silently replay or skip events after a
    # re-upgrade. Refuse before issuing any DDL, consistent with the market
    # integrity revision: restore a parent-schema backup instead.
    raise RuntimeError(
        "sse_20260720_market_event_stream downgrade is unsupported: "
        "restore a parent-schema backup instead"
    )

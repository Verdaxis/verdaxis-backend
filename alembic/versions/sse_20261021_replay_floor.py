"""Durable replay floor, retention ownership fence and replay membership index.

Revision ID: sse_20261021_replay_floor
Revises: p0_20261016_union

Additive schema for private market replay:

- ``market_event_replay_state`` — one row that records the highest outbox
  ``stream_seq`` whose deletion has been committed. A subscriber cursor below
  this value cannot prove complete coverage and must resync from REST.
- ``market_event_retention_control`` — one row that records who may prune the
  outbox. The deployed default is ``blocked``; only an operator transition to
  ``rust_active`` permits a delete.
- A ``BEFORE DELETE`` row trigger advances the barrier in the same transaction
  as the delete and refuses unsequenced rows and every delete without a valid
  owner record.
- A ``BEFORE TRUNCATE`` statement trigger refuses truncation.
- ``ix_market_event_outbox_participants_gin`` — expression index supporting the
  existing ``participant_org_ids ? '<uuid>'`` membership predicate.

Bootstrap takes ``SHARE ROW EXCLUSIVE`` on the outbox with ``NOWAIT`` so live
producers and the sequencer are fenced out instead of queued. The barrier
starts at the current high-water mark, which conservatively resets every older
cursor once. The migration never deletes, rewrites or renumbers retained rows.

Downgrade refuses: dropping the barrier would silently downgrade replay
coverage. A forward revert retains the tables, triggers and index.
"""
from __future__ import annotations

from alembic import op

revision = "sse_20261021_replay_floor"
down_revision = "p0_20261016_union"
branch_labels = None
depends_on = None


def upgrade() -> None:
    # Fence producers (INSERT/UPDATE) and the sequencer for the whole
    # migration transaction. NOWAIT fails fast rather than waiting behind live
    # writers; the statement limit bounds the index build.
    op.execute("SET LOCAL lock_timeout = '5s'")
    op.execute("SET LOCAL statement_timeout = '5min'")
    op.execute(
        "LOCK TABLE public.market_event_outbox "
        "IN SHARE ROW EXCLUSIVE MODE NOWAIT"
    )

    op.execute(
        """
        CREATE TABLE public.market_event_replay_state (
            singleton boolean PRIMARY KEY DEFAULT true,
            discarded_through_seq bigint NOT NULL DEFAULT 0,
            CONSTRAINT ck_market_event_replay_state_singleton CHECK (singleton),
            CONSTRAINT ck_market_event_replay_state_floor
                CHECK (discarded_through_seq >= 0)
        )
        """
    )
    op.execute(
        "COMMENT ON TABLE public.market_event_replay_state IS "
        "'Durable replay coverage barrier; migration-owned, app read-only'"
    )
    op.execute(
        """
        INSERT INTO public.market_event_replay_state
            (singleton, discarded_through_seq)
        SELECT true, COALESCE(MAX(stream_seq), 0)
        FROM public.market_event_outbox
        """
    )

    op.execute(
        """
        CREATE TABLE public.market_event_retention_control (
            singleton boolean PRIMARY KEY DEFAULT true,
            mode text NOT NULL DEFAULT 'blocked',
            ownership_epoch bigint NOT NULL DEFAULT 0,
            environment text,
            release_sha text,
            selector_config_digest text,
            drain_completed_at timestamptz,
            updated_at timestamptz NOT NULL DEFAULT now(),
            CONSTRAINT ck_market_event_retention_control_singleton
                CHECK (singleton),
            CONSTRAINT ck_market_event_retention_control_mode
                CHECK (mode IN ('blocked', 'rust_active')),
            CONSTRAINT ck_market_event_retention_control_epoch
                CHECK (ownership_epoch >= 0),
            CONSTRAINT ck_market_event_retention_control_release
                CHECK (release_sha IS NULL OR release_sha ~ '^[0-9a-f]{40}$'),
            CONSTRAINT ck_market_event_retention_control_selector
                CHECK (
                    selector_config_digest IS NULL
                    OR selector_config_digest ~ '^[0-9a-f]{64}$'
                )
        )
        """
    )
    op.execute(
        "COMMENT ON TABLE public.market_event_retention_control IS "
        "'Retention ownership fence; migration-owned, app read-only'"
    )
    op.execute(
        """
        INSERT INTO public.market_event_retention_control
            (singleton, mode, ownership_epoch)
        VALUES (true, 'blocked', 0)
        """
    )

    op.execute(
        """
        CREATE FUNCTION public.market_event_outbox_advance_replay_floor()
        RETURNS trigger
        LANGUAGE plpgsql
        SECURITY DEFINER
        SET search_path = pg_catalog, public
        AS $$
        DECLARE
            owner_valid boolean;
            advanced_floor bigint;
        BEGIN
            SELECT control.mode = 'rust_active'
                   AND control.environment IS NOT NULL
                   AND control.environment <> ''
                   AND control.release_sha IS NOT NULL
                   AND control.selector_config_digest IS NOT NULL
                   AND control.drain_completed_at IS NOT NULL
              INTO owner_valid
              FROM public.market_event_retention_control AS control
             WHERE control.singleton
               FOR SHARE;

            IF NOT COALESCE(owner_valid, false) THEN
                RAISE EXCEPTION
                    'market_event_outbox delete is refused: retention ownership is not rust_active'
                    USING ERRCODE = '42501';
            END IF;

            IF OLD.stream_seq IS NULL THEN
                RAISE EXCEPTION
                    'market_event_outbox delete is refused for an unsequenced row'
                    USING ERRCODE = '23514';
            END IF;

            UPDATE public.market_event_replay_state
               SET discarded_through_seq = GREATEST(
                       discarded_through_seq, OLD.stream_seq
                   )
             WHERE singleton
            RETURNING discarded_through_seq INTO advanced_floor;

            IF advanced_floor IS NULL THEN
                RAISE EXCEPTION
                    'market_event_replay_state is missing; replay coverage is unavailable'
                    USING ERRCODE = '55000';
            END IF;

            RETURN OLD;
        END;
        $$
        """
    )

    op.execute(
        """
        CREATE FUNCTION public.market_event_outbox_refuse_truncate()
        RETURNS trigger
        LANGUAGE plpgsql
        SECURITY DEFINER
        SET search_path = pg_catalog, public
        AS $$
        BEGIN
            RAISE EXCEPTION
                'market_event_outbox truncate is refused: restore a backup or record an ownership transition'
                USING ERRCODE = '42501';
        END;
        $$
        """
    )

    # Trigger functions run through the trigger machinery, never through an
    # application EXECUTE grant.
    op.execute(
        "REVOKE ALL ON FUNCTION "
        "public.market_event_outbox_advance_replay_floor() FROM PUBLIC"
    )
    op.execute(
        "REVOKE ALL ON FUNCTION "
        "public.market_event_outbox_refuse_truncate() FROM PUBLIC"
    )

    op.execute(
        """
        CREATE TRIGGER trg_market_event_outbox_replay_floor
        BEFORE DELETE ON public.market_event_outbox
        FOR EACH ROW
        EXECUTE FUNCTION public.market_event_outbox_advance_replay_floor()
        """
    )
    op.execute(
        """
        CREATE TRIGGER trg_market_event_outbox_refuse_truncate
        BEFORE TRUNCATE ON public.market_event_outbox
        FOR EACH STATEMENT
        EXECUTE FUNCTION public.market_event_outbox_refuse_truncate()
        """
    )
    # ALWAYS also fires under session_replication_role = replica, so an owner
    # session cannot use that setting as a pruning bypass.
    op.execute(
        "ALTER TABLE public.market_event_outbox "
        "ENABLE ALWAYS TRIGGER trg_market_event_outbox_replay_floor"
    )
    op.execute(
        "ALTER TABLE public.market_event_outbox "
        "ENABLE ALWAYS TRIGGER trg_market_event_outbox_refuse_truncate"
    )

    op.execute(
        """
        CREATE INDEX ix_market_event_outbox_participants_gin
        ON public.market_event_outbox
        USING gin ((participant_org_ids::jsonb) jsonb_ops)
        """
    )
    op.execute(
        "COMMENT ON INDEX public.ix_market_event_outbox_participants_gin IS "
        "'Participant membership for scoped replay pages'"
    )


def downgrade() -> None:
    raise RuntimeError(
        "sse_20261021_replay_floor downgrade is unsupported: dropping the "
        "replay barrier would silently reduce replay coverage; perform a "
        "forward revert that retains the tables, triggers and index"
    )

"""Fixture helpers for the durable replay floor schema proofs.

The deployed retention owner record defaults to ``blocked``. A disposable
fixture records a test-only ``rust_active`` row before it deletes retained
outbox rows, so the proofs exercise the deployed trigger instead of weakening
it. Nothing here may be imported by application code.
"""

from __future__ import annotations

from sqlalchemy import text

REVISION = "sse_20261021_replay_floor"
PARENT_REVISION = "p0_20261016_union"
TEST_RELEASE_SHA = "a" * 40
TEST_SELECTOR_DIGEST = "b" * 64

_RECORD_CONTROL = text(
    """
    INSERT INTO public.market_event_retention_control
        (singleton, mode, ownership_epoch, environment, release_sha,
         selector_config_digest, drain_completed_at)
    VALUES (true, :mode, :epoch, :environment, :release_sha,
            :selector_digest, CASE WHEN :drained THEN now() ELSE NULL END)
    """
)


async def record_ownership(
    connection,
    *,
    mode: str = "rust_active",
    drained: bool = True,
    epoch: int = 1,
    environment: str | None = "test",
    release_sha: str | None = TEST_RELEASE_SHA,
    selector_digest: str | None = TEST_SELECTOR_DIGEST,
) -> None:
    """Replace the singleton owner record with an explicit fixture state."""

    await connection.execute(text("DELETE FROM public.market_event_retention_control"))
    await connection.execute(
        _RECORD_CONTROL,
        {
            "mode": mode,
            "epoch": epoch,
            "environment": environment,
            "release_sha": release_sha,
            "selector_digest": selector_digest,
            "drained": drained,
        },
    )


async def reset_outbox(connection) -> None:
    """Empty the outbox on a disposable test database.

    The deployed row trigger refuses every unsequenced-row delete and the
    statement trigger refuses TRUNCATE, so fixture isolation disables only the
    row trigger as the owning role for the duration of this transaction; a
    rollback also restores the trigger. Application and operator prune paths
    never call this helper and never bypass the trigger.
    """

    await connection.execute(
        text(
            "ALTER TABLE public.market_event_outbox DISABLE TRIGGER "
            "trg_market_event_outbox_replay_floor"
        )
    )
    await connection.execute(text("DELETE FROM public.market_event_outbox"))
    await connection.execute(
        text(
            "ALTER TABLE public.market_event_outbox ENABLE ALWAYS TRIGGER "
            "trg_market_event_outbox_replay_floor"
        )
    )


async def clear_retention(connection) -> None:
    """Empty the outbox and reset the coverage barrier on a disposable database."""

    await reset_outbox(connection)
    await connection.execute(text("DELETE FROM public.market_event_replay_state"))
    await connection.execute(
        text(
            "INSERT INTO public.market_event_replay_state "
            "(singleton, discarded_through_seq) VALUES (true, 0)"
        )
    )

-- Give stream sequence values to committed outbox rows that have none yet.
--
-- Run only inside the assignment transaction of the session that holds the
-- shared sequencer advisory lock. A producer's uncommitted rows are not
-- visible here, so no row receives a sequence value before its producer
-- commits. The single bound parameter is the batch row limit.
WITH pending AS (
    SELECT id
    FROM market_event_outbox
    WHERE stream_seq IS NULL
    ORDER BY created_at, id
    LIMIT $1
    FOR UPDATE SKIP LOCKED
)
UPDATE market_event_outbox AS outbox
SET stream_seq = nextval('market_event_stream_seq'),
    dispatched_at = now(),
    delivery_attempts = outbox.delivery_attempts + 1
FROM pending
WHERE outbox.id = pending.id

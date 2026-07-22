# Market Event Dispatch (durable shared SSE transport)

Status: built on the integration branch (Stage 5). Nothing here activates a
deployment; staging/production enablement stays behind the combined gates
and owner approval.

## What it is

Private market lifecycle events (`order_created`, trade lifecycle,
negotiation/RFQ updates, …) are persisted as `market_event_outbox` rows in
the same transaction as the economic change
(`app/services/market_events.py`). The dispatcher
(`app/services/market_event_dispatch.py`) makes those rows reach every SSE
subscriber regardless of which Uvicorn worker serves the connection:

1. **Producer wake.** `enqueue_market_events` emits
   `NOTIFY verdaxis_market_events`; PostgreSQL delivers it only when the
   producing transaction commits. NOTIFY is a lossy wake signal only — the
   outbox table is the durable source of truth and a poll fallback (2 s)
   covers lost signals.
2. **Sequencer (single leader).** One worker per database holds a
   session-level advisory lock (`SEQUENCER_ADVISORY_LOCK_KEY`). The leader
   assigns `stream_seq` from the `market_event_stream_seq` sequence to
   committed-but-unsequenced rows (`stream_seq IS NULL`, oldest first),
   stamps `dispatched_at`, commits, then NOTIFYs again. Assigning **after**
   commit through a **single serialized leader** guarantees the visible
   maximum sequence only grows, so `stream_seq > cursor` can never skip a
   row that becomes visible later (this is the race that forbids assigning
   sequences in the producing transaction). Sequence holes from crashed
   assignment transactions are allowed and meaningless. If the leader dies,
   its connection drops, the lock releases, and another worker takes over.
   The assignment UPDATE runs on the exact session that holds the advisory
   lock (the dedicated listener connection), never on a pool connection:
   losing that connection aborts any in-flight assignment and releases the
   lock atomically, so a successor leader can never assign concurrently
   with a still-running assignment from the previous leader.
3. **Hub (every worker).** Each worker fans newly sequenced rows out to the
   in-process bus on org-bound channels `trades:{organization_id}` derived
   from the row's persisted participant list.
4. **SSE delivery** (`GET /api/stream/trades`). Channel identity comes only
   from the validated stream token's organization — never from client
   input. Frames carry `id: <stream_seq>`. On reconnect the standard
   `Last-Event-ID` header (or an initial `last_event_id` query parameter)
   replays everything committed for that organization after the cursor
   straight from the outbox, then live delivery resumes; the cursor
   de-duplicates overlap between replay and the live bus.

## Connection budget

Each worker holds **one** dedicated (non-pool) LISTEN/leadership connection;
all queries use the normal engine pool in short transactions. With the
deployed defaults that adds 4 connections per service on top of the
documented `2 services x 4 workers x (2+1) + 20 reserve = 44`, i.e. 52 of
`max_connections=100`. `app.database.configured_connection_total` models the
listener connections, so the boot-time capacity guard fails closed if the
full budget (pools + listeners + reserve) exceeds the server's
`max_connections`. Revisit the budget doc if pool sizes change.

## Outbox prune policy (documented, NOT armed)

Nothing in this repository deletes outbox rows automatically, and no timer
for it exists or is installed.

- Retention window: keep sequenced rows (`stream_seq IS NOT NULL`) for **14
  days**. The window bounds `Last-Event-ID` replay: a client reconnecting
  with a cursor older than the horizon receives only retained events and
  must treat the gap as a full-resync signal (refetch via REST).
- Rows with `stream_seq IS NULL` are pending dispatch and must never be
  pruned.
- Operator statement (manual, environment-attested session, migrator role):

  ```sql
  DELETE FROM market_event_outbox
  WHERE stream_seq IS NOT NULL
    AND created_at < now() - interval '14 days';
  ```

- If pruning is ever automated it needs its own reviewed one-shot CLI plus
  an environment-specific systemd timer (the news-refresh/prune pattern);
  web workers must not schedule it. The app role deliberately has no DELETE
  grant on the table.

## Failure modes

- **Lost NOTIFY** → poll fallback delivers within ~2 s.
- **Leader crash mid-assignment** → transaction rolls back (`stream_seq`
  stays NULL), another worker takes the lock and re-assigns with higher
  sequence values. Enqueue order and sequence order can diverge across a
  crash; clients order by `id`/`stream_seq`, which is the canonical order.
- **Subscriber overload** → the in-process bus queue (100) drops the
  subscriber's queue on overflow; the client reconnects with
  `Last-Event-ID` and replays from the outbox, so nothing is lost.
- **Replay query failure** → the stream emits a terminal `error` event and
  closes; the client retries.
- **Wedged-but-alive leader** (holds the lock, stops assigning) → the only
  external signal is a growing unsequenced backlog.
  `deploy/monitor/outbox_backlog_probe.py` exposes
  `count(*) WHERE stream_seq IS NULL` plus the oldest pending age via a
  read-only psql query (exit 1 on threshold breach). It ships in the
  attested monitor manifest but is deliberately NOT armed by any
  service/timer in this repository.

## Proof

`tests/postgres/test_market_event_dispatch.py` runs a real
`uvicorn --workers 4` server against one disposable PostgreSQL 17 database
and asserts: cross-worker delivery (subscribers on at least two distinct
worker PIDs all receive an event committed through one worker's HTTP
request), `Last-Event-ID` replay after disconnect, and cross-tenant
isolation under concurrent producers.

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
   captures a sequence high-water mark, pages every authorized row through
   that boundary straight from the outbox, then merges queued live rows in
   sequence order. The cursor de-duplicates overlap between replay and the
   live bus. Global sequence holes and rows for other organizations are
   expected; neither condition ends catch-up. Token expiry and mutable user
   and organization admission remain enforced while pages are delivered.

### Heartbeat and system status (all SSE streams)

`/api/stream/prices`, `/api/stream/orderbook`, `/api/stream/trades` and
`/api/stream/activity` carry a named `heartbeat` event instead of the former `: keepalive` comment, which
browsers' `EventSource` never exposes:

```
event: heartbeat
data: {"status": "operational", "message": null, "server_time": "2026-10-28T12:00:00Z"}

```

- `status` is `operational`, `degraded` or `maintenance`; `message` is a
  string or `null`; `server_time` is UTC ISO-8601 with a trailing `Z`.
- Values come from `SYSTEM_STATUS` (default `operational`, case-insensitive,
  any other value fails startup) and `SYSTEM_STATUS_MESSAGE` (default unset,
  at most 280 characters, blank means `null`). They are read from the service
  environment at startup; changing them needs a restart of every worker. No
  migration is involved.
- Heartbeats never carry an `id:` line, so they cannot move the client's
  `Last-Event-ID` cursor.
- One heartbeat is sent when a stream opens, then one every 30 seconds on a
  fixed cadence, whether the stream is idle or busy. Data frames do not
  postpone it, because clients mark a connection stale when heartbeats stop.
  On the private stream the 15 second authorization revalidation wake-ups do
  not produce heartbeats (the old keepalive was sent on each such wake-up).
- Placement on the private and activity streams: after replay completes and after the first
  expiry/authorization check passes. Replay frames therefore keep their exact
  order and ids, and `auth_expired`, `auth_revoked`, `error` and the overflow
  `reset` are never preceded by a status frame. A very large replay backlog
  delays the first heartbeat until the backlog is delivered.
- The too-many-connections `error` frame is unchanged and is the only frame
  on that response.

Every committed market-event payload includes additive `schema_version: 1`.
The dispatcher also supplies that field when it reads older retained rows, so
replay and live delivery use the same compatible payload shape.

Eligible public projection writers add a transport-owned marker to their
participant-scoped outbox row. The dispatcher removes that marker before
private live delivery or replay. After it drains all currently sequenced rows,
it coalesces one or more markers into one `market_invalidated` event on each
of the process-local `prices` and `orderbook` channels. The public payload
is exactly `{"schema_version": 1, "resync_required": true}` and has no event
ID, identity, market slice, reason, price, or quantity.

The signal tells clients to reload authoritative REST data; it is not a delta,
checksum, or replay cursor. Existing queue overflow resets, reconnect cache
invalidation, snapshots, and bounded polling remain recovery paths. Time alone
does not write an outbox row. An expired order therefore disappears from REST
queries at its deadline, while the refresh signal can be delayed until an
existing expiry writer next commits that transition. No new expiry timer is
part of this transport.

### Public stream contract (`prices`, `orderbook`)

The public streams carry exactly these frames:

- `market_invalidated`, the only data frame. Private order and trade events
  (`order_created`, `order_cancelled`, `orders_matched`, ...) are published
  to `trades:{organization_id}` only and never reach `prices` or
  `orderbook`.
- `heartbeat`, the terminal `reset` and the too-many-connections `error`.

Contract for clients:

- Public frames have **no `id:` line** and the payload carries no sequence,
  cursor or previous-frame field. They are idempotent "refetch" signals.
- There is **no ordering, duplicate or gap guarantee**. The number of frames
  per committed change is not specified (a hub coalesces markers per drain,
  and different workers may coalesce differently).
- Clients refetch authoritative REST data on every `market_invalidated`
  frame and on every (re)connect. A missed frame is recovered by the next
  frame, the reconnect refetch, the overflow `reset` or bounded polling.
- The public streams ignore `Last-Event-ID`; there is no public replay.
- A marker whose hub drain step failed after its cursor moved past the row is
  kept and published by the next drain, so a transient database error does
  not swallow a refresh signal.

Why ids are not exposed: the only available sequence is the global outbox
`stream_seq`, shared with every private event of every tenant. Putting it on
an unauthenticated stream would let anyone infer the aggregate private market
volume from id jumps. A dense per-channel counter would need new shared
state and a migration, and it would add nothing for a refetch signal.

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
- **Subscriber overload** → the in-process bus queue (100) replaces the
  undeliverable backlog with one terminal `reset` event carrying
  `resync_required: true`, removes the subscriber from fan-out, and closes
  the stream after that frame. The client retains its last acknowledged
  private cursor, invalidates relevant cached reads, reloads authoritative REST
  data, and reconnects from that cursor for durable replay.
- **Replay query failure** → the stream emits a terminal `error` event and
  closes; the client retries.
- **Wedged-but-alive leader** (holds the lock, stops assigning) → the only
  external signal is a growing unsequenced backlog.
  `deploy/monitor/outbox_backlog_probe.py` exposes
  `count(*) WHERE stream_seq IS NULL` plus the oldest pending age via a
  read-only psql query (exit 1 on threshold breach). It ships in the
  attested monitor manifest, but the manifest and source presence do not prove
  installation. The tracked external-monitor source invokes the probe, but both
  database targets are hard-coded to `127.0.0.1:5432`. This wiring cannot serve
  the split production and staging targets and must remain unpromoted until a
  reviewed integration separates the targets and supplies noninteractive libpq
  authentication. There is no separate probe service or timer.
  Alert categories remain stable while the changing count and age stay in
  structured monitor status, so a growing backlog cannot bypass the hourly
  recovery and diagnosis cooldown.

## Proof

`tests/postgres/test_market_event_dispatch.py` runs a real
`uvicorn --workers 4` server against one disposable PostgreSQL 17 database
and asserts: cross-worker delivery (subscribers on at least two distinct
worker PIDs all receive an event committed through one worker's HTTP
request), `Last-Event-ID` replay after disconnect, and cross-tenant
isolation under concurrent producers.

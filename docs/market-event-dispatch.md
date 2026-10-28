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

## Price-alert evaluation

The Rust stream runtime also consumes the sequenced rows it already reads:
`trade_auto_matched` (auto-confirmed on creation) and `trade_confirmed` (a
counterparty confirmation) are the two events that carry a newly executed
price. Their payloads do not reliably name the product and delivery point, so
the evaluator reads the trade row by id and uses only a formal snapshot with a
REAL buyer and a REAL seller; DEMO, TEST, CANARY, unknown and mixed pairs never
fire an alert.

- **Once per alert, cluster-wide.** Every worker runs a dispatcher and sees
  every event, so the evaluator is idempotent instead of elected. Candidate
  alerts are locked in id order, then deactivated with
  `UPDATE price_alerts SET is_active = false, triggered_at = $now
  WHERE id = ANY($ids) AND is_active RETURNING ...`. A second worker waits for
  the first commit, finds no active row and writes nothing.
- **One transaction.** The deactivation, one `notifications` row per approved
  member of the alert's organization (type `SYSTEM`, so Python can still read
  it) and one organization-private `price_alert_triggered` outbox row commit
  together. The outbox row is sequenced by the normal sequencer and reaches the
  org-bound trades stream and replay. If the transaction fails, the alert stays
  active.
- **No loops.** Only the two execution event types are evaluated.
- **Dispatch is never held up.** The dispatch sink only calls `try_send` on a
  bounded queue. A full queue drops the offer with a warning; failed
  evaluations are retried with the next event, or after five seconds, up to
  three attempts. The dispatcher cursor is not involved.
- **Limits.** A print committed while no Rust stream runtime is running is not
  evaluated; the alert fires on the next qualifying print. The evaluator is off
  where the Rust stream runtime is off.

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
  days**. The durable `market_event_replay_state.discarded_through_seq` coverage
  barrier advances in the same transaction as each authorized deletion. A Rust
  replay cursor below that barrier receives a terminal `replay_retention` reset
  and must reload REST state. Cursor equal to the barrier remains usable.
- Rows with `stream_seq IS NULL` are pending dispatch and cannot be deleted.
  `TRUNCATE` is refused, including under replica session mode. Do not disable
  triggers for online pruning.
- `market_event_retention_control` defaults to `blocked`. Keep it blocked while
  Python owns any private stream, during selector changes or drains, when drain
  evidence is unknown, or while Rust streams are unavailable. A selector reload
  alone does not authorize pruning. Only a separately authorized operator transaction after verified
  Rust-only ownership and drain may record `rust_active` with exact environment,
  release SHA, selector digest, completed drain and increasing ownership epoch.
  The deployment owner supplies the reviewed ownership-transition procedure;
  this migration does not authorize activation.
- Applying `sse_20261021_replay_floor`: the migration holds `SHARE ROW
  EXCLUSIVE` on the outbox until it commits, including the participant index
  build. `NOWAIT` only stops the migration from waiting for live writers;
  producer inserts and sequencer updates that arrive later wait behind it and
  fail after the app role's 3-second lock timeout. Apply it in a quiet window
  and record the outbox row count as a release input. `validate_roles.sql`
  expects the retention tables, triggers and functions only from this
  checkpoint on.
- Before rollback or a selector change to Python, commit `blocked` under
  `FOR UPDATE` on the ownership row. This waits for any authorized in-flight
  prune, which holds `FOR SHARE`. Record the current floor and drain evidence
  privately; Python active cursor coverage is unknown. Leave the additive
  tables, triggers and floor installed. Never lower the floor.
- Operator statement (manual, environment-attested session, migrator role,
  **only after** independent ownership authorization):

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

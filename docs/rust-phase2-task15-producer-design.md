# Task 15 evidence and the PD4-G trusted producer design

Status: Task 15 is ported. The producer design below is a proposal for PM
approval; no producer code is written.

## Task 15 — feed and read state (W07, W08)

`rs/read/src/crud/watchlists/events.rs` serves both routes through the shared
authenticated transaction. It reads rows the current producer commits; it
produces none.

- The PD4-G guard runs in SQL before `LIMIT`, so a page is full while visible
  rows remain instead of shrinking by the hidden rows.
- Cursor: RFC 3339 UTC with microsecond precision, then `|`, then the event id.
  The decoder also accepts `Z`, a numeric offset, a space separator and a naive
  timestamp, so a cursor issued by either stack is valid while both serve the
  group.
- Acknowledgement is refused before any write for a guarded event, writes
  nothing for an already-read row, and commits the read before reporting an
  absent target. The update requires `is_read=false`, so two readers that both
  saw an unread row serialize on the row lock and the loser writes no version.
- Client contract: `tests/rust_contract/phase2/watchlist-events.json` pins the
  exact field set, nullability, types, page bounds and payload defaults the
  frontend reads. Tests assert the serialized document against it.

Evidence: 21 Rust tests, including 10 disposable-PostgreSQL cases (tied
timestamps, guard-before-limit with a following page, exactly-full page,
committed read state and repeat no-write, concurrent acknowledgement write
count, guarded refusals without writes, foreign container, missing target, no
prune on read, snapshot point-name fallback). No Python comparison is a gate.

## PD4-G trusted producer — design proposal

### Why a producer is needed

Python writes `watchlist_events` inside the same transaction as the order
change. Those rows carry no attestation that their payload matches committed
market state, so PD4-G hides every `SLICE_BEST_PRICE_MOVED` row: the payload's
old/new aggregate prices cannot be tied to a committed, publicly scoped read.
The feed becomes trustworthy when the event is derived from committed state by
an identified producer, not when the clock passes a cutoff.

### Input and single owner

The producer consumes committed `market_event_outbox` rows. Sequencing stays
with the existing single-leader sequencer; the producer is a **consumer**, not
a second dispatcher. It reads rows in `stream_seq` order, keeps its own
watermark, and never assigns `stream_seq`.

Order writes must atomically add to their outbox row:

- a durable mutation identity (`event_type`, `aggregate_type`, `aggregate_id`,
  and the outbox row id);
- the affected slice keys (market product, delivery point, window, side) and
  the affected order id;
- before/after evidence for price and remaining quantity, taken from the
  committed row rather than recomputed later;
- the participant scope, as today, for private SSE delivery.

The producer must not use `participant_org_ids` for watchlist fan-out. Watchlist
events are public-scope: derivation uses only the public projection
(`market_product`, point, window, side, price, quantity, status) and the shared
full public-order relation.

### Identity and schema

Add to `watchlist_events` (additive, nullable):

| column | meaning |
| --- | --- |
| `producer` | `legacy-python` for existing rows; `rust-market-v1` for the new producer |
| `source_event_id` | the outbox row id the event was derived from |
| `source_stream_seq` | the sequence assigned to that outbox row |
| `scope_attested_at` | committed instant at which the public-scope check passed |

`producer` backfills to `legacy-python`; a unique index on
`(producer, source_event_id, watchlist_target_id)` makes replay idempotent.
Python ignores the columns, so its reads and target cascades keep working.

The consumer needs `SELECT` on `market_event_outbox` and the
`watchlist_events` insert path; it needs no `UPDATE`/`DELETE` on the outbox.
Crate choice (decision 111): `sqlx` for the consumer transaction and
`sqlx::postgres::PgListener` for the `NOTIFY` wake, with a bounded poll
fallback — the same primitives the existing dispatcher uses, no new dependency.

### Derivation and ordering

For each sequenced outbox row, in `stream_seq` order:

1. Confirm the order is in the public scope at the producing commit; otherwise
   record the watermark and write nothing, or write a `PIN_WITHDRAWN`-class
   transition only when the order had previously produced a visible event.
2. Match PIN targets by `order_id` and SLICE targets by the slice key.
3. Diff against the stored target snapshot (price, remaining, status) to choose
   the event type — never against a value recomputed at consume time.
4. Set `created_at` from the outbox row's commit instant, so feed order is
   stable across restarts and independent of consumer wake time.
5. Insert with `ON CONFLICT DO NOTHING` on the identity index.

Watermark = greatest consumed `stream_seq`, stored in a one-row consumer-state
table. A restart resumes after it. Re-processing a window is harmless; a hole is
skipped, as sequence holes are expected by design.

### Visibility transitions

Attestation proves scope at commit, not forever. Every read still applies the
**current** public-order scope:

- a trusted event whose source order has left the public scope is hidden at
  read time, as it is today;
- a best-price event is visible only when `producer='rust-market-v1'` **and**
  `scope_attested_at >= PD5_ACTIVATION`;
- counts and latest-event metrics use the same predicate as the feed.

A single `event_visible` decision serves feed, acknowledgement and metrics; it
takes the event's producer and attestation plus the current scope. Tests must
prove feed and counts agree for a transitioning order.

### Activation

Per environment, in runtime release metadata with the full release SHA:

- `WATCHLIST_EVENT_PRODUCER=legacy-python|rust-market-v1`;
- `PD5_ACTIVATION` instant, independent per environment, with no clock or config
  default.

Only one producer writes per environment. Python's emitter currently lives in
the order-write paths, so switching it off is a Python edit. That edit is not
authorized today (decision 47) and belongs to the order-write group cutover
(Task 25) or a separately approved change. Until then the Rust producer can run
in **shadow**: it derives and records events with `producer='rust-shadow'`,
which reads never serve, and the comparison of shadow rows against Python's
rows becomes the acceptance evidence. Activation is the single reviewed switch
from shadow to serving.

### Rollback

One-change rollback stays intact:

- stop the producer (service stop or switch back to `legacy-python`);
- legacy Python rows are untouched and Python resumes emitting;
- trusted rows remain readable and deletable through the existing target
  cascade; Python serves them as ordinary events;
- the additive columns stay; no schema rollback is needed.

### When the PD4-G read guard can go

All of:

1. activation metadata present and the producer serving in that environment;
2. zero `SLICE_BEST_PRICE_MOVED` rows with `producer='legacy-python'` inside the
   90-day retention window — either aged out or migrated with evidence;
3. tests proving feed, unread counts and latest-event metrics share one
   trusted-source predicate, including an order that leaves the public scope;
4. PM approval of the removal, as a separate change with its own rollback.

Until then the guard stays exactly as ported.

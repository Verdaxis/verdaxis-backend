# Public read cache infrastructure checkpoint

## Schema and promotion

Proposed literal checkpoint: `prc_20261018_cache`, directly after
`p0_20261016_union`. The only new transition is
`p0_20261016_union -> prc_20261018_cache`, plus its no-change transition.
Production remains at `p0_20261016_union`. Staging and production each need an
independently approved source SHA and the canonical deploy helper's exact current
and target revisions. The PM owns those deploys. No schema action ran on either
live database. This checkpoint does not activate trading caching.

The additive migration creates three tables, one restricted trigger function,
and statement triggers on existing dependency tables. It changes no existing
column and drops no object. It takes `SHARE ROW EXCLUSIVE` locks on every
dependency table with `NOWAIT` before any DDL, so a deploy behind an active
writer fails fast instead of queueing and blocking later writers; readers are
not blocked. Downgrade refuses; rollback is a reviewed forward revert. Model
changes are declarations only, registered for Alembic drift checks. The runtime
receives SELECT only. Migrator ownership remains the canonical DDL and
trusted-trigger authority. No runtime sequence, UPDATE, INSERT or DELETE
privilege is added for cache metadata.

The compiled Rust revision allowlist (`deploy/rust/allowed-revisions.json`)
accepts `prc_20261018_cache` and keeps both earlier checkpoints, so one binary
starts against a database before and after this checkpoint.

## Coarse trading dependency matrix

All INSERT, UPDATE, DELETE and TRUNCATE operations on each listed table advance
the single `trading` generation in the same transaction. This intentionally covers
all columns, including future columns, rather than inferring economic relevance
from a private SSE event. Events carry only domain and generation. An update that
changes no value can invalidate; this costs work but cannot expose stale data.
The trigger is a statement trigger, so it fires once per statement even when no
row matches.

Each trigger is `ENABLE ALWAYS`: it fires under every `session_replication_role`,
so tooling that skips ordinary triggers cannot hide a write from the cache. The
runtime role cannot change the trigger or that setting (proved by test). The Rust
listener refuses to attach unless all sixteen triggers still have exactly that
shape (`dependency_triggers_complete`).

| Dependency tables | Read meaning |
| --- | --- |
| orderbook_orders | resting economics, quantity, state, expiry, terms |
| trades | immutable snapshots, confirmation and lifecycle |
| products, delivery_points | catalog identity, lane/activity metadata |
| users, organizations | admission, KYC binding, organization provenance |
| kyc_execution_grace | migration-owned execution eligibility |
| market_row_quarantines, organization_market_approvals | trusted visibility controls |
| market_indications, fair_price_bands, physical_stems | curve evidence |
| market_signal_ingestion_runs | signal trust |
| benchmarks, live_slice_benchmarks | external and same-side reference evidence |
| market_event_outbox | existing economic event writes |

Clock-dependent expiry, grace, lookback and day boundaries are not database
mutations. Task 8 must prove a valid-until boundary or bypass the cache. No TTL is
allowed to substitute for trading freshness.

### Closed-world ledger

`tests/postgres/test_public_read_cache_events.py` closes the ledger in both
directions:

- Every table a Rust query names (scanned over `rs/read/sql` and `rs/read/src`
  against the migrated database's real table list) must be a dependency above or
  appear in a reviewed exclusion with its reason: `news_items`, `ports` and
  `port_intelligence` belong to the separate 60-second reference cache;
  `notifications`, `referrals` and `watchlists` appear only in private
  authenticated code that is never cached; `alembic_version` is a startup
  revision attestation; the three cache tables are the infrastructure itself.
  A new Rust query over an unreviewed table fails the test.
- The Rust list (`TRADING_DEPENDENCY_TABLES`) equals the migration list, and every
  dependency appears in this document.
- Every dependency carries the always-enabled statement trigger, and each of
  INSERT, UPDATE, DELETE and TRUNCATE on each one advances the generation.

The Rust sources today do not name `market_row_quarantines`,
`organization_market_approvals`, `live_slice_benchmarks` or
`market_event_outbox`. They stay in the matrix because the Python policy that the
Rust routes mirror reads them; over-invalidation is safe, a missing dependency
is not. Narrowing is a later, separately reviewed change.

Writers are covered at both levels: direct statements on every table, and the
real Python routes (place, amend, cancel, publish and cross an order) each
advance the generation with no gap in the event sequence. Concurrent writers
receive distinct, contiguous generations. The deployed application role performs
a business statement and the generation append in one transaction and has no
INSERT, UPDATE, DELETE or TRUNCATE on any cache table.

### Known write cost (not measured)

Every statement on a dependency table updates the one `trading` generation row
and keeps its row lock until commit, so writing transactions serialize on that
row after their first dependency write, and a transaction that later needs a lock
another transaction holds while waiting for the generation row can deadlock
(PostgreSQL aborts one with 40P01). The existing PostgreSQL concurrency suites,
including the simultaneous crossing-post and market-integrity schedules, pass with
the triggers in place. No contention or speed measurement ran (decision 96): the
write cost and the deadlock rate under realistic load are open items for the
phase gate, and trading caching stays off if either is unacceptable. Per-slice
generations would reduce both and are a separate, reviewed change.

## Authority boundary

Database incarnation metadata starts `active=false`, authority revision zero.
Copied metadata is not proof of a primary. The only activation proof is an
**owner record outside every database**: `cache-authority.json` in the runtime
directory, written atomically by `scripts/rust_cache_authority.py`. The record is
either `active` with `(database_epoch, primary_identity, authority_revision)` or
`revoked`; its format is pinned by `rs/read/tests/data/cache_authority_records.json`,
which the Rust reader and the Python owner both pass.

A Rust process serves a trading hit or accepts a fill only while all of these
hold (`CacheAuthority`):

1. The owner record names an active authority and the process re-read it within
   `AUTHORITY_LEASE` (5 s; refreshed every `AUTHORITY_REFRESH`, 1 s).
2. The database connection that serves the request reports the same epoch,
   primary identity and revision from its own incarnation row, is not in
   recovery, and the row is `active`.
3. A change listener bound to that same identity is attached. The listener reads
   the identity from its own connection after `LISTEN`, and only attaches when
   it equals the active record and every dependency trigger is intact.
4. The request's fence token is still current. Any change of authority, listener,
   or revocation bumps a fence counter, so an entry or fill made earlier is
   never accepted again, even with equal identity and generation. Generations may
   only move forward inside one authority; a smaller one (an unrotated restore)
   revokes everything.

An unreadable, missing, symlinked, oversized, malformed or revoked record means
no hits. A process that cannot re-read the record (stopped, starved, cut off)
loses its hits by itself when its lease lapses; **no routing change or
acknowledgement is needed to stop it**. This replaces the plan's
acknowledge-or-lose-route step: safety rests on the lease, not on a message.

### Owner procedure (promotion, restore, replacement)

`scripts/rust_cache_authority.py activate` is a recovery step, never part of an
ordinary deploy (a unit test pins that `deploy.sh` and `prepare_rust_runtime.py`
do not call it, so a deploy cannot create or rotate authority). In order:

1. Publish a `revoked` record at a revision above everything seen.
2. Wait `FENCE_DRAIN_SECONDS` (10 s, twice the lease; a test ties it to the Rust
   constant). Every process has re-read the revocation or let its lease expire.
3. Through the migrator role, in one transaction, bind the incarnation row to a
   fresh random epoch and primary identity and a higher revision (refused for a
   replica, a reused identity or a lower revision).
4. Publish the matching `active` record.

Any failure between steps leaves the record revoked, so caching stays off. The
database is not trusted to remember anything: a restored copy keeps its old epoch,
primary identity and generation, and only the rotation above makes it servable.

### What is proved and where

- `rs/read/tests/cache_authority.rs`: lease, fence, listener binding, generation
  rollback, fill ticket and record vectors, with an explicit clock.
- `rs/read/tests/cache_events_postgres.rs`: notifications follow commit; a
  rolled-back change delivers nothing; a lost listener connection detaches, hits
  stop, and reconnect finds the commit made meanwhile; a listener refuses a
  database that is not the active authority or whose triggers are weakened.
- `rs/read/tests/cache_failover_postgres.rs`, on two migrated incarnations:
  `promotion_reuses_generation_without_reusing_cache`,
  `restore_rotates_authority_before_activation`,
  `old_primary_connection_is_fenced`, `old_epoch_fill_cannot_publish` (equal,
  ahead and rolled-back generations, at both insertion and response),
  `uncertain_authority_disables_hits` (lease lapse of a stuck process, missing,
  malformed and symlinked records, mismatched metadata) and
  `missing_generation_infrastructure_disables_caching`.
- `tests/postgres/test_public_read_cache_failover.py`: the owner script against
  the real row, including refusal of copied identities and lower revisions.

## Connection budget

The listener is one dedicated connection outside the Rust pool. The budget
artifact (`budget.json`) carries an optional `cache_listener` (0 or 1); it is
absent, and the artifact byte-identical, for every disabled-cache start. Both the
Python preparer and the Rust validator count it, refuse any other value, and
require the whole total to fit the configured maximum. `CacheEvents::connect`
refuses an unbudgeted listener before it opens any connection.

## Not yet activated

Nothing starts the listener or the refresh loop, and no deploy passes
`--cache-listener 1`. Trading caching stays disabled. Remaining for Task 8: start
the refresh loop and listener under an explicit switch with the budgeted slot,
clear the reference cache when `current_fence()` changes, the commit-safe hit
check in each route, and the time-boundary rules. This document is not a Task 8
completion or a release verdict.

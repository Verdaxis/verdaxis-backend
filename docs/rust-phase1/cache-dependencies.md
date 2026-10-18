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
column and drops no object. Downgrade refuses; rollback is a reviewed forward
revert. Model changes are declarations only, registered for Alembic drift checks.
The runtime receives SELECT only. Migrator ownership remains the canonical DDL
and trusted-trigger authority. No runtime sequence, UPDATE, INSERT or DELETE
privilege is added for cache metadata.

## Coarse trading dependency matrix

All INSERT, UPDATE, DELETE and TRUNCATE operations on each listed table advance
the single `trading` generation in the same transaction. This intentionally covers
all columns, including future columns, rather than inferring economic relevance
from a private SSE event. Events carry only domain and generation. An update that
changes no value can invalidate; this costs work but cannot expose stale data.

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

## Authority boundary — not yet activated

Database incarnation metadata starts `active=false`, authority revision zero.
Copied metadata is not proof of a primary. `CacheAuthority` starts disabled and
requires a separately proved identity/revision; revocation changes its fence so
an old fill token cannot publish, including after reconnect to the same identity.
No runtime caller activates it, no listener connection is opened, and no pool
budget is increased by this checkpoint.

Remaining Task 7 acceptance work: canonical deploy/recovery-owner external
activation and old-primary connection fencing, listener reconnect/event delivery,
complete two-database failover tests, exact runtime-role DML-denial tests and
connection-budget proof. Until those pass, trading caching must remain disabled.
This document is not a Task 7 completion or release verdict.

# Integrated migration cutover (enterprise-hardening RC)

Operator runbook for promoting the linearized migration chain to a deployed
environment. Deployed environments NEVER run `alembic upgrade head`; every
step below is one allowlisted checkpoint transition applied with
`scripts/apply_migration_checkpoint.py` under the deploy contract in
`CLAUDE.md` (approved release SHA + expected current revision + literal
target from that SHA's `deploy/migration-checkpoints.tsv`).

## The chain

```text
pa_20260715_analytics_facts
  -> rh_20260720_runtime_metadata
  -> sec_20260720_identity      (PAUSE: 24h token-compatibility window)
  -> sec_20260720_boundaries \
  -> sec_20260720_fresh       | applied as one transition
  -> sec_20260720_device     /  (PAUSE: security review)
  -> miq_20260720_market_quarantine   (PAUSE: explicit sentinel quarantine)
  -> mi_20260720_market_integrity     (PAUSE)
  -> sse_20260720_market_event_stream (head)
```

Every revision from `mi` onward refuses downgrade by design; recovery from a
bad step is a parent-schema backup restore, never `alembic downgrade`.

## Staged identity cutover (the 24-hour window)

`sec_20260720_identity` hashes legacy plaintext email-verification tokens in
bounded batches and sets every migrated token's expiry to migration time
plus 24 hours. `sec_20260720_boundaries` (the first revision of the
`sec_identity -> sec_device` transition) HARD-FAILS while any of those
compatibility tokens is unexpired — this is deliberate, not a defect.

1. Apply checkpoint `rh_20260720_runtime_metadata -> sec_20260720_identity`.
   Restart onto the paused revision; backend units verify the published
   `MIGRATION_REVISION`, so the pause is fully startable.
2. Wait the full 24 hours (or deliberately expire the outstanding links and
   notify affected users).
3. Apply checkpoint `sec_20260720_identity -> sec_20260720_device`. If the
   window has not elapsed, the transition aborts with "Legacy email
   verification compatibility window is still active" and nothing mutates —
   wait and re-run.
4. Run the read-only enforcement preflight before starting
   enforcement-capable workers: `./venv/bin/python -m scripts.security_preflight`
   (exit 2 = BLOCKED; see docs/runbooks/security-v2-operator-review.md).

Disposable/CI databases have no unexpired legacy tokens, so a single-run
upgrade passes there; the staged wait applies only to live databases carrying
legacy tokens.

## Market quarantine gate

`miq -> mi` refuses while the known zero-value sentinel order exists. The
sentinel must first be archived with the explicit operator CLI
(`scripts/remediate_market_data.py quarantine --order-id <exact id> --apply`
with operator/reason/reference); the migration then proceeds. Accepted
demo-RFQ remediation likewise requires the explicit CLI. Both remain behind
owner approval.

## Shared SSE transport checkpoint

`mi -> sse_20260720_market_event_stream` has no data preconditions: it adds
the stream sequence, the `stream_seq` column, and indexes. After restart the
in-worker dispatcher starts automatically (PostgreSQL engines only). ACL
convergence must run from the release's committed bundle as usual — the
policy now grants the app role USAGE on `market_event_stream_seq`, and
`deploy/postgres/validate_roles.sql` fails closed if the grant is missing or
wider than declared. Operational notes, connection budget, and the outbox
prune policy: docs/market-event-dispatch.md.

## Client resync horizon

If the outbox prune policy (14-day retention, manual, never armed by this
repository) is exercised, SSE clients reconnecting with a `Last-Event-ID`
older than the retained horizon receive only retained events. Frontend
guidance: treat a reconnect after prolonged absence as a full-resync signal
(refetch authoritative state via REST, then resubscribe without a cursor).

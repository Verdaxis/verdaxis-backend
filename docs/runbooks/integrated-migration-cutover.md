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

## One-time bootstrap from the legacy deploy helper

The pre-hardening live checkouts run a deploy helper and backend units that do
not understand release identities, literal migration checkpoints, or durable
deployment state. Do not invoke that legacy helper against this release: it
would run `alembic upgrade head` and bypass every pause below.

For the first promotion only, after the release branch and full SHA have been
reviewed and pushed:

1. Take and verify the backup required for the first `pa -> rh` checkpoint.
2. In the fixed environment checkout, fetch the exact remote branch and
   fast-forward to the approved SHA. Refuse a dirty tree, non-fast-forward, or
   any SHA mismatch. Keep the already-running old workers in place during this
   source-only bootstrap; do not restart them yet.
3. From that exact clean checkout, run
   `scripts/install_systemd_units.sh --dry-run`, then `--apply`, with the
   environment and approved full SHA. This installs the guard-aware units but
   does not restart or enable anything.
4. Run the new `scripts/deploy.sh --dry-run`, then the real deploy for the
   literal `pa_20260715_analytics_facts -> rh_20260720_runtime_metadata`
   checkpoint. The new helper publishes `.runtime-release.env` before the
   first restart, so the newly installed unit starts only with matching source,
   schema, and release identity.

Record the source fast-forward, installed unit digest manifest, and deploy
verdict in the cutover log. After this bootstrap, every later promotion uses
only the hardened deploy helper; do not repeat the manual source step.

## Backup gate (MANDATORY before every checkpoint from `sec_identity` on)

Because restore-from-backup is the ONLY recovery for this chain, no
checkpoint from `rh -> sec_identity` onward may be applied without a fresh,
verified dump taken after the previous checkpoint settled. The gate is
three steps, all recorded in the change log with the operator's name:

1. **Fresh dump** (run as `verdaxis-prod` on the VPS; the read-only
   `verdaxis_backup` / `verdaxis_backup_staging` role, plain format piped
   through gzip so the verifier's `PostgreSQL database dump` marker check
   applies):

   ```bash
   TS=$(date -u +%Y%m%d-%H%M%S)
   # production:
   pg_dump "dbname=verdaxis user=verdaxis_backup" \
     | gzip > /home/verdaxis-prod/backups/verdaxis-${TS}.sql.gz
   # staging:
   pg_dump "dbname=verdaxis_staging user=verdaxis_backup_staging" \
     | gzip > /home/verdaxis-prod/backups/verdaxis-staging-${TS}.sql.gz
   ```

2. **Verification green.** Run the attested verifier against the backup
   directory and require exit 0 with every check `ok` (freshness, gzip
   floor, dump marker, metadata/attempt consistency):

   ```bash
   ./venv/bin/python deploy/monitor/backup_verify.py \
     --backup-directory /home/verdaxis-prod/backups \
     --backup-status /home/verdaxis-prod/backups/status.json \
     --output /tmp/pre-checkpoint-backup-verify.json
   ```

3. **Gate record.** Append to the cutover change log BEFORE running the
   checkpoint: operator name, environment, target checkpoint, the dump
   filename from step 1, and the verifier output path/verdict from step 2.
   An unrecorded gate is a failed gate: do not proceed.

Apply the gate before each of these transitions (each is destructive or
non-downgradable): `rh -> sec_identity` (rewrites verification tokens),
`sec_identity -> sec_device` (drops the plaintext token column),
`miq` and `miq -> mi` (quarantine + integrity constraints; `mi` refuses
downgrade), and `mi -> sse_20260720_market_event_stream` (non-downgradable
head). A dump older than the previous checkpoint's application does NOT
satisfy the gate for the next one.

## Staged identity cutover (the 24-hour window)

`sec_20260720_identity` hashes legacy plaintext email-verification tokens in
bounded batches and sets every migrated token's expiry to migration time
plus 24 hours. `sec_20260720_boundaries` (the first revision of the
`sec_identity -> sec_device` transition) HARD-FAILS while any of those
compatibility tokens is unexpired — this is deliberate, not a defect.

1. Backup gate (above), then apply checkpoint
   `rh_20260720_runtime_metadata -> sec_20260720_identity`.
   Restart onto the paused revision; backend units verify the published
   `MIGRATION_REVISION`, so the pause is fully startable.
2. Wait the full 24 hours (or deliberately expire the outstanding links and
   notify affected users).
3. Backup gate again (a pre-`sec_identity` dump does not cover the token
   rewrite), then apply checkpoint
   `sec_20260720_identity -> sec_20260720_device`. If the
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
backup gate applies before the quarantine CLI and before the checkpoint. The
sentinel must first be archived with the explicit operator CLI
(`scripts/remediate_market_data.py quarantine --order-id <exact id> --apply`
with operator/reason/reference); the migration then proceeds. Accepted
demo-RFQ remediation likewise requires the explicit CLI. Both remain behind
owner approval.

## Shared SSE transport checkpoint

`mi -> sse_20260720_market_event_stream` requires the backup gate (the head
is non-downgradable) but has no data preconditions: it adds
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

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

1. Use a separate clean operator checkout at the exact approved SHA. Keep the
   fixed live checkout and running legacy workers unchanged.
2. Take and verify the backup required for `pa -> rh`, then apply that literal
   checkpoint with `scripts/apply_migration_checkpoint.py` from the operator
   checkout, passing the selected environment's fixed live `.env` as the
   explicit `--environment-file`. Repeat the backup gate and apply
   `rh -> sec_identity`. These
   revisions are additive and retain the plaintext compatibility column.
   `sec_identity` installs a compatibility trigger that hashes and expiry-binds
   every token subsequently written by the legacy release, so signup, resend,
   login, trading, and verification remain available during the 24-hour wait.
3. Do not restart the approved new application at `rh` or `sec_identity`: its
   complete models intentionally target the later market/security schema.
4. After the compatibility window, enter the public maintenance state. Disable
   and stop the selected backend, its demo-activity timer/service, and every
   environment-specific database-writing timer/service. Assert that none of
   those units, application worker processes, or application-role write
   sessions remain. Take a new backup, apply `sec_identity -> sec_device`,
   then advance to `miq` from the operator checkout. Run security review,
   exact-ID organization approval, and market quarantine before applying `mi`
   and then `sse` with a fresh backup at every required gate. Keep every writer
   disabled and stopped throughout these non-compatible intermediate schemas,
   including across a reboot.
5. Only after the database reaches `sse`, fast-forward the fixed environment
   checkout to the approved SHA. Refuse a dirty tree, non-fast-forward, or SHA
   mismatch. Run `scripts/install_systemd_units.sh --dry-run`, then `--apply`.
6. Run `scripts/deploy.sh --dry-run`, then the real no-op checkpoint deployment
   `sse_20260720_market_event_stream -> sse_20260720_market_event_stream`.
   The no-op pair is explicitly allowlisted for source-only activation. The
   helper installs dependencies, converges ACLs, publishes release identity,
   starts the hardened unit, and enforces public readiness before clearing the
   maintenance state. Explicitly enable the selected hardened backend and
   verify it remains enabled after readiness succeeds. Re-enable only the
   approved environment-specific timers after readiness; confirm each timer's
   source identity first.

Record every checkpoint, source fast-forward, installed unit digest manifest,
and deploy verdict in the cutover log. After this bootstrap, later releases use
only the hardened deploy helper, including allowlisted same-revision releases.

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

`sec_20260720_identity` hashes existing legacy plaintext email-verification
tokens and sets every migrated token's expiry to migration time plus 24 hours.
It also installs a compatibility trigger: every token created or
replaced by a still-running legacy worker receives the matching SHA-256 hash and
a fresh 24-hour expiry in the same statement. `sec_20260720_boundaries` (the first revision of the
`sec_identity -> sec_device` transition) HARD-FAILS while any of those
compatibility tokens is unexpired — this is deliberate, not a defect.

1. Backup gate (above), then apply checkpoint
   `rh_20260720_runtime_metadata -> sec_20260720_identity` from the exact clean
   operator checkout while the legacy workers remain the serving release.
2. Wait until the latest `email_verification_token_expires_at` has passed. A
   signup or resend during the window advances that deadline; use the database
   value, not the original migration timestamp, as the source of truth.
3. Backup gate again (a pre-`sec_identity` dump does not cover the token
   rewrite), enter maintenance, fully quiesce the environment as specified
   below, then apply checkpoint
   `sec_20260720_identity -> sec_20260720_device`. If the
   window has not elapsed, the transition aborts with "Legacy email
   verification compatibility window is still active" and nothing mutates —
   wait and re-run.
4. Continue to `miq`, apply the approved organization/quarantine decisions,
   then run the read-only enforcement preflight before proceeding to `mi`:
   `./venv/bin/python -m scripts.security_preflight` (exit 2 = BLOCKED; see
   docs/runbooks/security-v2-operator-review.md). Start enforcement-capable
   workers only after the complete schema reaches `sse`.

Immediately before `sec_identity -> sec_device`, both checks must return zero:

```sql
SELECT count(*) AS unbound_legacy_tokens
FROM users
WHERE email_verification_token IS NOT NULL
  AND (
    email_verification_token_hash IS NULL
    OR email_verification_token_expires_at IS NULL
    OR email_verification_token_hash <> encode(
      sha256(convert_to(email_verification_token, 'UTF8')), 'hex'
    )
  );

SELECT count(*) AS active_legacy_tokens
FROM users
WHERE email_verification_token IS NOT NULL
  AND email_verification_token_expires_at > CURRENT_TIMESTAMP;
```

### Full writer quiescence

Before applying `sec_identity -> sec_device`, disable and stop the matching
backend plus all environment-specific writers. For staging these include
`verdaxis-backend-staging.service`, `verdaxis-demo-activity-staging.timer` and
`.service`, `verdaxis-auth-maintenance-staging.timer` and `.service`,
`verdaxis-news-refresh-staging.timer` and `.service`, and
`verdaxis-product-analytics-prune-staging.timer` and `.service`. Use the
equivalent non-`-staging` units for production. Also stop any legacy canary or
scheduler configured to write to that environment.

The pre-hardening VPS uses one legacy `verdaxis-demo-activity.timer` and
`verdaxis-demo-activity.service` wrapper that writes to both databases. Disable
and stop both units before the first staging cutover, even though this pauses
production demo ticks. Never re-enable the shared legacy timer. Activate the
attested split staging timer only after staging readiness; activate the split
production timer only after production readiness. Platform traffic and real
market activity do not depend on these synthetic demo timers.

Record `systemctl is-enabled` and `systemctl is-active` output for every unit.
After stopping them, require no matching backend/demo process and no non-idle
session for the environment's application role in `pg_stat_activity`. Keep the
units disabled until the database is at `sse`, hardened units are installed,
the allowlisted `sse -> sse` deployment succeeds, and public readiness is
green. Re-enable only the approved timers, then verify their next-run time and
release identity. Explicitly `systemctl enable` the selected hardened backend
after readiness and require both `is-active` and `is-enabled` to report the
expected state; the installer deliberately does not enable units and a restart
alone is not reboot-persistent.

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
owner approval. An accepted RFQ with no exact orderless trade candidate must
be acknowledged explicitly with `--rfq-no-trade <rfq-uuid>`; the command
refuses missing acknowledgements, trade/no-trade conflicts, or any candidate
trade when no-trade was asserted.

The same `miq` pause owns explicit REAL-organization approval. Generate and
retain the dry-run output before applying exact IDs:

```bash
./venv/bin/python scripts/remediate_market_data.py \
  --database-url "$MIGRATOR_DATABASE_URL" \
  --environment "$ENVIRONMENT" \
  --attestation "$ENVIRONMENT:$DATABASE_NAME" \
  approve-real-organizations \
  --organization-id <exact-uuid>
```

Review every organization ID, full snapshot, snapshot hash, and qualifying
member UUID. The applied command additionally requires the exact dry-run hash
as `--expected-snapshot <organization-uuid>=<sha256>`, operator, reason,
reference, `--apply`, and the production approval-reference gate. Any change
between review and apply aborts the whole batch. It atomically approves the
organization, writes append-only application audit history, and records the
reviewed organization/member snapshot in the migrator-owned
`organization_market_approvals` ledger. Demo/test IDs and organizations with
no approved email-verified BUYER/SUPPLIER are refused.

The `mi` migration aborts if any reviewed organization identity or qualifying
member set changed after approval. It promotes only ledgered exact IDs to
`REAL`, then snapshots that provenance into existing orders. After `mi`, the
same command is the only allowed `UNKNOWN -> REAL` transition and remains
subject to the exact snapshot and operator controls.

Before continuing to SSE, require all three checks to return no rows or zero:

```sql
(SELECT id FROM organizations WHERE provenance = 'REAL'
 EXCEPT SELECT organization_id FROM organization_market_approvals)
UNION ALL
(SELECT organization_id FROM organization_market_approvals
 EXCEPT SELECT id FROM organizations WHERE provenance = 'REAL');

SELECT count(*) FROM organizations
WHERE provenance = 'REAL' AND verification_status <> 'APPROVED';

SELECT count(*)
FROM orderbook_orders AS orders
JOIN organizations ON organizations.id = orders.organization_id
WHERE orders.status NOT IN ('CANCELLED', 'EXPIRED')
  AND orders.provenance IS DISTINCT FROM organizations.provenance;
```

After `mi`, approval refuses an organization while it owns any nonterminal
`UNKNOWN` order, including `FILLED` orders whose pending trade could still be
declined and reopen them. Cancel, expire, or explicitly quarantine those rows
first. Only `CANCELLED`/`EXPIRED` historical orders may retain their immutable
provenance snapshot.

Before `mi`, legacy deterministic DEMO/TEST orders may still violate the new
expiry or partial-fill lifecycle constraints. Use
`scripts/remediate_market_data.py expire-invalid-legacy-synthetic-orders`
dry-run first, review its count and snapshot hash, then repeat with `--apply`
and the exact `--expected-snapshot`. The command is restricted to ownerless
rows under the compiled synthetic organization registries, archives every
original row, and expires it in place. It cannot select real, mixed, or newly
valid demo liquidity.

For the first non-downgradable staging checkpoint and the production pre-`mi`
checkpoint, restore the verified dump into an isolated database. Confirm the
Alembic revision and critical row counts, validate constraints, and run a
read-only application smoke test. Structural dump verification alone is not a
restore proof.

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

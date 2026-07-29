# Supabase Managed Database and Auth Program

**Status:** Final reviewed plan; DB-0 implemented and verified. Database and
Auth cutovers are not authorized.

**Branch:** `plan/supabase-managed-infra`

**Source baseline:** backend production commit
`7ea5d6908d0ed0d09999e1a153f42111fdff3631`

**Reviews:**

- GPT Pro conversation: `6a6a1466-9994-83ec-b8f5-9954edbcaf61`
- Claude artifact:
  `/tmp/claude-supabase-plan-review-20260729T152042Z.md`
- Final maintainer pass: 2026-07-29

## Objective

Evaluate and, only if the gates pass, move Verdaxis PostgreSQL/PostGIS to a
managed Supabase database. Keep the current Auth implementation unchanged
during that program. Supabase Auth is a separate later program, gated by a
specific customer or security requirement.

Signup, login, organization onboarding, existing sessions, order placement,
matching, audit, and market-event delivery must remain uninterrupted.

## Decisions

1. Begin provider-neutral database readiness work.
2. Use one provider project per environment. Never combine production and
   staging in one database.
3. Rehearse the database move on an empty paid Supabase project before changing
   staging traffic.
4. Keep FastAPI, SQLAlchemy, Alembic, current IDs, authorization, matching,
   outbox, PostgreSQL listeners, SSE, and commercial APIs.
5. Do not expose commercial tables through PostgREST, GraphQL, Supabase
   Realtime, Edge Functions, or browser database keys.
6. Use an offline dump/restore by default. The current databases are only about
   30 MiB, so logical replication and application dual-write are unnecessary
   unless measurements overturn that conclusion.
7. Do not change Auth during any database release.
8. Do not implement Supabase Auth until a separately approved trigger exists,
   such as signed customer SAML SSO, MFA, or a security/compliance finding.
9. Do not use the Supabase Free plan for production or production-like staging.

The target region is selected by measured application-to-database latency. The
VPS currently has IPv6 and Singapore egress, but the target project must still
be measured before choosing a region.

## Measured Baseline

Read-only measurements taken on 2026-07-29:

| Item | Production | Staging |
|---|---:|---:|
| Database size | 30,783,155 bytes | 31,659,699 bytes |
| Users | 23 | 19 |
| Bcrypt hashes | 23 | 19 |
| Other hash formats | 0 | 0 |
| `must_change_password=true` | 0 | 0 |
| Admins | 4 | 5 |
| PostgreSQL | 17.9 | 17.9 |
| PostGIS | 3.6.2 in `public` | 3.6.2 in `public` |

The backup timer is active and its latest run completed successfully about
20 hours before measurement. The host had approximately 285 GiB free (36%).
The backup producer exists outside the repository at
`/home/verdaxis-prod/backup-db.sh`; its secret-free source and restore procedure
need explicit versioned ownership before a cutover.

All current passwords use direct bcrypt. Passwords longer than 72 UTF-8 bytes
are first SHA-256 digested by `app/core/security.py`, so their hashes are not
portable to an external verifier without a first-login fallback. The number of
such users cannot be inferred from stored hashes.

## Non-Negotiable Boundaries

- Database and Auth are separate programs, releases, and rollback decisions.
- Exactly one application database accepts public writes at a time.
- FastAPI remains the only browser-facing commercial API.
- Verdaxis database state remains authoritative for role, organization,
  membership, KYC, admission, forced-password, and assisted-context decisions.
- Matching and organization onboarding remain application transactions.
- Historical Alembic revisions are not edited.
- The existing durable outbox, sequencer, listener, SSE hub, and 60-second
  stream token remain unchanged during the database move.
- No production cutover starts with a stale backup, failed restore, low disk,
  unresolved schema/ACL drift, or red critical journey.
- Production and staging cannot share a provider database because advisory-lock
  keys, the notification channel, and market-event sequence are database-wide.
- Any hosted-platform exception to current ACL ownership is documented,
  compensated, and explicitly approved before staging traffic moves.

## Current Provider Blockers

The present source cannot connect safely to Supabase without a later
runtime-enablement phase:

1. Database names and role identities are hard-coded independently in
   `app/config.py`, `app/environment_database.py`, and
   `scripts/converge_runtime_acls.py`. Supabase's integrated database is
   normally named `postgres`.
2. No runtime pool, Alembic connection, dedicated listener, or ACL-convergence
   `psql` path explicitly verifies remote TLS.
3. The listener opens a separate raw asyncpg connection and therefore does not
   inherit SQLAlchemy connection options.
4. The first migration creates PostGIS as the migrator. A hosted migrator
   cannot create the extension; it must be pre-created by the platform owner.
5. PostGIS currently lives in `public`. A different target extension schema
   requires reviewed `search_path`, drift, and ACL behavior.
6. Current policy requires the Verdaxis migrator to own the database and
   `public` schema and requires exact database/schema ACL grantors. Hosted
   provider ownership may make those invariants impossible.
7. Current connection budgeting assumes production and staging share one
   server (`DB_SERVICE_COUNT=2`). One project per environment changes the real
   budget.
8. Supavisor is not a drop-in fallback: URL usernames and query parameters can
   conflict with current exact identity validation.
9. The deployment checkpoint policy has no fresh-database transition.
10. The current hosted-target test suite assumes superuser-only operations,
    including `session_replication_role`.

## Phase 0: Current Recovery Baseline

- [x] Record exact production backend SHA and Alembic checkpoint.
- [x] Measure database sizes, account/hash classes, versions, extensions,
      connection baseline, disk, and backup age.
- [ ] Put the secret-free backup producer and restore procedure under explicit
      versioned ownership.
- [ ] Restore the newest production backup into an isolated database.
- [ ] Record restore duration and approve RPO, RTO, and write-freeze duration.
- [ ] Record endpoint p50/p95/p99 and SQL round-trip counts for login,
      organization creation, and order placement.

**Gate G0:** backup age is within schedule, an isolated restore succeeds inside
the approved RTO, disk is at least 30% free, release identity is exact, and all
critical journeys pass.

## Phase 1: DB-0 Provider-Neutral Safety Checks

Implement only:

1. A generic model-to-column-ACL conformance test that catches client-side ORM
   defaults or update defaults on restricted columns. This closes the exact
   July 29 signup regression class.
2. A disposable PostgreSQL hosted-privilege-envelope test that proves which
   current ownership and membership invariants fail when provisioning is done
   by a non-superuser that does not own the database.
3. A short provider-neutral privilege-floor document.
4. This reviewed plan.

Reuse the existing PostgreSQL/PostGIS harness, ACL policy, role validator,
migration drift checks, and cutover runbook. Do not add a second harness,
snapshot framework, workflow, or runbook.

**Gate G1:** the full unit and disposable PostgreSQL suites pass, the new tests
fail when their protected condition is deliberately reintroduced, and no
runtime, frontend, Auth, Alembic, systemd, or live configuration file changes.

DB-0 evidence on 2026-07-29:

- 1,244 unit tests passed, including the model-to-column-ACL conformance check
  and its deliberate denied-default control.
- 93 disposable PostgreSQL 17/PostGIS 3.6 tests passed, including the hosted
  privilege-envelope probe.
- The measured provider-neutral privilege floor is documented in
  `docs/managed-postgres-privilege-floor.md`.
- No application runtime, migration, frontend, deployment, or live
  configuration changed.

## Phase 2: Empty Supabase Project Rehearsal

Provision one paid test project with no Verdaxis data or Auth configuration.

- [ ] Measure the target PostgreSQL/PostGIS versions, extension schema,
      provider roles/grants, `max_connections`, direct connectivity, IPv6,
      latency, and TLS chain.
- [ ] Disable the Data API and automatic commercial-table grants.
- [ ] Restrict network access to controlled backend/operator addresses.
- [ ] Pre-create PostGIS through the provider-supported privileged path.
- [ ] Create distinct Verdaxis app, migrator, and backup roles.
- [ ] Run the hosted privilege checks without attempting to weaken or repair
      provider-owned objects.
- [ ] Run only target-compatible database tests; exclude fixtures requiring
      superuser-only `session_replication_role`.
- [ ] Verify transaction advisory locks.
- [ ] Verify session-level sequencer lock persistence, abrupt-disconnect
      release, leadership takeover, and outbox delivery after reconnect.
- [ ] Verify prepared statements and long-lived `LISTEN` on the selected
      endpoint.
- [ ] Inventory every current `validate_roles.sql` assertion the target cannot
      satisfy.

**Gate G2:** all capabilities are measured and every failed invariant is
classified. No application code or staging traffic moves yet.

### Hosted Invariant Decision

Before Phase 3, maintain a signed table containing:

| Current invariant | Likely hosted conflict | Required compensation |
|---|---|---|
| Migrator owns database | Provider owns integrated database | App-object ownership remains exact; no provider role may read commercial objects |
| Migrator owns `public` | Provider may own `public` | Exact governed-object ownership and grants; Data API disabled |
| Exact database ACL/grantor | Provider roles/grantors remain | Deny commercial-object access to all non-Verdaxis roles |
| Exact schema ACL/grantor | Provider roles/grantors remain | Same executable governed-object check |
| Protected roles have no memberships | PG16+ role creation may add membership | Prove memberships can be revoked while roles remain manageable |

If the compensating controls cannot be implemented without broad provider-role
access, stop the migration.

## Phase 3: Runtime Enablement

This is a normal reviewed code release before any database cutover:

- parameterize exact provider database identity in the three existing identity
  authorities without accepting arbitrary runtime identities;
- add verified TLS to SQLAlchemy runtime, Alembic, raw asyncpg listener, and
  ACL-convergence `psql`;
- make a wrong CA fail on every connection path;
- provision and attest target `search_path` if PostGIS is outside `public`;
- update only observed PostGIS/platform drift exclusions;
- budget one environment per provider project rather than two services on one
  server;
- preserve distinct app/migrator/backup roles;
- add a target-safe governed-object grant check;
- add a reviewed fresh-database/bootstrap checkpoint path;
- prove one ordinary post-bootstrap migration works.

**Gate G3:** current self-hosted staging remains green, target configuration
passes all new tests, wrong identity and wrong TLS fail closed, and no
authorization or market behavior changes.

## Phase 4: Staging Database Cutover

Because staging is about 31 MiB, use a bounded offline dump/restore unless the
measured rehearsal disproves it.

1. Freeze staging DDL and public writes.
2. Stop mutation canaries, jobs, listeners, and workers.
3. Confirm the old database has no remaining sequencer lock holder.
4. Dump only required application schema/data and roles through the reviewed
   provider-compatible path.
5. Restore into the target and set custom role passwords.
6. Synchronize and verify every sequence.
7. Assert
   `market_event_stream_seq.last_value >=
   MAX(market_event_outbox.stream_seq)`.
8. Run mandatory reconciliation.
9. Start target-connected workers without public writes or background jobs.
10. Pass startup identity, TLS, migration, schema, ACL, and readiness checks.
11. Run signup, login, organization, forced-password fixture, order, matching,
    cancellation, audit, admin-context, listener-failover, and SSE replay.
12. Enable one set of jobs/listeners and then staging traffic.
13. Rehearse target-to-old rollback after synthetic writes.

Keep the frontend hostname, JWT keys, cookies, token lifetimes, and Auth routes
unchanged.

**Gate G4:** every critical journey passes, mandatory reconciliation is exact,
endpoint p95 is within the owner-approved absolute budget, connection use is at
most 60% of the measured provider limit, listener failover loses no outbox
event, and rollback is rehearsed.

## Phase 5: Stabilization and Production Rehearsal

- [ ] Run staging through at least two normal releases.
- [ ] Compare endpoint latency, pool waits, errors, lock waits, listener
      reconnects, and outbox lag with Phase 0.
- [ ] Perform an isolated restore using provider backups.
- [ ] Perform a timed production-shaped dump/restore and rollback.
- [ ] Select production compute, region, PITR, and independent backup.
- [ ] Produce the exact operator commands by extending
      `docs/runbooks/integrated-migration-cutover.md`.
- [ ] Obtain explicit production cutover approval.

**Gate G5:** two releases and the observation window are clean; restore and
post-write rollback meet RPO/RTO; one ordinary migration succeeds; and owner
approval is recorded.

## Phase 6: Production Database Cutover

No Auth changes are permitted.

1. Enter the approved write freeze and stop all writers/listeners/jobs.
2. Copy, restore, synchronize sequences, and run mandatory reconciliation.
3. Start target-connected workers without public writes.
4. Run production-safe synthetic journeys.
5. During this discard-safe window, rollback may discard only explicitly
   synthetic target writes.
6. Enable one set of jobs/listeners and public traffic.
7. Observe the approved no-DDL rollback window.

After public writes open, rollback requires a new write freeze and replay or
copy-back of all target changes. At minimum:

- organizations, users, memberships, orders, trades, and audits are copied
  back;
- target-created refresh sessions may be discarded only with an explicit
  re-login consequence;
- the old `market_event_stream_seq` is advanced beyond the greatest sequence
  ever emitted by the target;
- mandatory reconciliation passes before reopening traffic.

**Gate G6:** one unexplained signup, login, organization, execution, audit, or
event-delivery failure stops writes and invokes the rehearsed rollback.

## Mandatory Reconciliation

Run on every rehearsal, promotion, and rollback:

- exact Alembic revision and application schema fingerprint;
- row counts for critical tables;
- FK orphan and duplicate natural-key counts;
- all sequence values and ownership;
- organization, membership, order, trade, and audit continuity;
- outbox maximum sequence, unsequenced count, and unpublished age;
- PostGIS geometry count, type, SRID, validity, and spatial indexes;
- one synthetic action producing exactly one expected audit row.

Use chunk checksums, distribution comparisons, or query-plan diffs only when a
mandatory check or performance measurement is suspicious.

Generated reports contain no row data, email addresses, organization names,
DSNs, credentials, tokens, or private infrastructure addresses.

## Deferred Program: Supabase Auth

Supabase Auth is not part of the database plan and will not be implemented
until a concrete SSO, MFA, or security requirement is approved.

The pre-agreed boundary is:

- Supabase owns credentials, identity verification, MFA, and SSO.
- FastAPI remains the `/api/auth/*` facade.
- FastAPI validates provider JWTs locally and exchanges the identity for the
  existing Verdaxis access token and device-bound refresh family.
- Provider refresh tokens do not become Verdaxis browser sessions.
- Database state remains authoritative for all permissions.
- `users.id` remains unchanged; a nullable unique `auth_user_id` links identity.
- Existing sessions and custom SSE stream tokens remain.
- SSO never grants organization membership automatically.

All current hashes are bcrypt and are potentially importable. Because the
application pre-hashes passwords longer than 72 bytes, an Auth migration must
retain a narrow legacy verification fallback until each imported identity has
successfully authenticated or reset. It must branch before calling
`verify_password`; replacing a hash with a non-bcrypt marker would currently
raise rather than return a normal login failure.

If authorized later, write a separate plan covering:

1. synthetic canary;
2. staff cohort;
3. all remaining users;
4. new-registration idempotency and resumable organization onboarding;
5. password reset, verification, forced-password, admin, SSO, and MFA behavior;
6. provider outage and rollback.

Do not use Auth Hooks or custom role claims initially.

## Cost Posture

- At least Pro for each active project.
- Production PITR only after a measured restore requirement justifies its
  retention and cost.
- Retain an independent off-platform logical backup.
- Continue Resend/custom SMTP for any future Auth use.
- Team is justified by operator SSO/support/governance needs.
- Enterprise is justified by a contractual uptime SLA, designated support,
  private connectivity, or negotiated compliance terms.

## Implementation Checklist

### Authorized now

- [x] DB-0 model-to-ACL conformance test.
- [x] DB-0 hosted privilege-envelope test.
- [x] Provider-neutral privilege-floor documentation.
- [x] Run full unit and disposable PostgreSQL suites.
- [x] Commit the reviewed plan and DB-0 on the feature branch.

### Requires a Supabase test project

- [ ] Phase 2 measurements and hosted invariant decision.

### Requires later code review

- [ ] Phase 3 runtime enablement.

### Requires explicit owner approval

- [ ] Staging database cutover.
- [ ] Production database cutover.
- [ ] Any Supabase Auth implementation.

## Final Review Outcome

GPT Pro conditionally supported the managed database move and proposed keeping
Supabase Auth behind FastAPI. Claude passed the database direction with
required edits and rejected the original Auth scope because it relied on an
incorrect PBKDF2 premise and delivered no immediate approved benefit.

The final plan accepts Claude's database corrections, corrects the source fact
to bcrypt, removes speculative Auth implementation from the database sequence,
and retains only a separately gated Auth architecture because the user asked
to evaluate that destination.

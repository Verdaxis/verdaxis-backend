# Runtime and test hardening

## Database connection budget

Production and staging share one PostgreSQL cluster. `UVICORN_WORKERS` is the
single worker-count setting consumed by systemd and the pool validator; each
service currently uses four workers and the same conservative SQLAlchemy defaults:

```text
services × workers × (pool_size + max_overflow) + maintenance reserve
2 × 4 × (2 + 1) + 20 = 44 ≤ PostgreSQL max_connections=100
```

`Settings` validates this shared aggregate for both deployed environments.
Production and staging require exactly two application services and a minimum
20-connection maintenance reserve, so configuration cannot undercount the
immutable deployed topology. Startup attests `current_database()`,
`current_user`, exact role properties, absence of every membership/`SET ROLE`
path, and `SHOW max_connections`; an identity or capacity mismatch aborts
startup. The reserve is for migrations, administration, and other processes;
it is not a promise that PostgreSQL creates those clients.

## KYC upload limits and service memory

`KYC_MAX_FILE_BYTES` defaults to 10 MiB per document and
`KYC_MAX_TOTAL_BYTES` defaults to 20 MiB per request. The KYC route checks the
declared size and reads in bounded chunks before sending either document to
Gemini; a per-file or aggregate violation returns HTTP 413. Settings rejects a
total limit smaller than the per-file limit.

The measured steady state is approximately 530–538 MiB per service. The
checked-in systemd units therefore start at `MemoryHigh=768M` and
`MemoryMax=1G`, leaving upload/concurrency headroom. These thresholds are a
defensible starting point, not a claim of measured safety; continue monitoring
peak RSS and cgroup usage before changing them.

## systemd topology and preflight

`deploy/systemd/verdaxis-backend.service` binds production to
`127.0.0.1:8000`; `verdaxis-backend-staging.service` binds staging to
`127.0.0.1:8001`. The public Caddy endpoints reverse-proxy to those loopback
ports. Both units order after `network-online.target` and require/order on
`postgresql.service`; neither depends on Docker.

Each unit requires the literal `MIGRATION_REVISION` published with its release
identity and verifies that the live database is at exactly that checkpoint
before Uvicorn. It does not require the checkpoint to equal a later source
head. Each unit passes the same `UVICORN_WORKERS=4` value used by application pool math,
sets `PYTHONDONTWRITEBYTECODE=1`, and applies a read-only systemd sandbox. No
broad source-tree `ReadWritePaths` grant is present. The checked-in units are
artifacts only; this change does not install or deploy them. News refresh has
exactly one external owner in each environment:
`verdaxis-news-refresh.timer` in production and
`verdaxis-news-refresh-staging.timer` in staging. Each invokes the locked
`python -m app.cli.refresh_news` entrypoint through a one-shot service. Uvicorn
workers do not create a news scheduler, the public manual refresh route is
removed, and a PostgreSQL transaction advisory lock rejects overlapping CLI
runs. Installation and timer enablement remain separate operator-held live
actions; see `docs/news-refresh-timer.md`.

Product-analytics retention likewise has exactly one external timer/service pair
per environment. Each prune service loads both `.env` and
`.runtime-release.env`, passes a literal `production` or `staging` target plus
the release SHA to the destructive CLI, and refuses an absent, development, or
mismatched identity before opening a database engine. The services want
network-online, require/order after PostgreSQL, require the backend mount, and
run `pg_isready` before the CLI. Boot-time failures retry after 30 seconds but
are bounded to five starts per 15 minutes. Their filesystem, device,
capability, address-family, and system-call sandbox matches the news units. The
timer names its matching service explicitly. Installation and timer enablement
are operator-held live actions. After separately approving and applying the
unit bundle, an operator may enable the intended timer with
`sudo systemctl enable --now verdaxis-product-analytics-prune.timer` or
`sudo systemctl enable --now verdaxis-product-analytics-prune-staging.timer`;
this branch does neither.

The units require the gitignored `.runtime-release.env` artifact containing the
exact environment, full source SHA, and literal deployed migration revision.
They also check the durable per-environment
`.runtime-deploy/<environment>.state` file. A normal
start is allowed only when state is absent; the one explicit
`DEPLOYMENT_STATE=restart-authorized` phase permits the controlled deploy
restart. Blocked and readiness-pending state refuses starts. An actual
`scripts/deploy.sh` run first rejects a dirty tree, acquires a durable
environment-specific `flock`, and atomically writes blocked state before
source mutation. It fetches and fast-forwards only the selected target branch,
but requires `APPROVED_RELEASE_SHA` to equal the full SHA printed by a prior
dry-run; a moved remote branch is refused. Before acquiring the lock or
mutating source, it also requires `MIGRATION_APPROVED_SOURCE_SHA` to equal that
same SHA and requires literal `MIGRATION_EXPECTED_CURRENT_REVISION` and
`MIGRATION_TARGET_REVISION` values. The expected/target pair must occur in the
selected SHA's committed `deploy/migration-checkpoints.tsv`; aliases such as
`head`, revision arithmetic, absent revisions, and reverse traversal refuse.
After the policy check, deploy acquires the environment lock and the clean
current checkout's exact-revision verifier reads live state through the
explicit migrator credential. A revision different from the operator-approved
current value refuses before blocked state or source fast-forward while still
preventing another deploy from racing that decision. The selected helper
repeats that check before migration and verifies the exact result afterward.
The selected SHA and approved target revision are published to
`.runtime-release.env` before invoking preflight, Python/pip, or the exact
migration checkpoint from the new tree.
The checkpoint helper re-attests the clean checkout, source SHA, migrator
identity, current revision, graph ancestry, and exact resulting revision. It
loads settings from an explicit regular `--environment-file`, ignores process
control keys in that file, activates the attested source root, and refuses if
any `app` module resolves outside it. This prevents an operator checkout from
mixing candidate and live namespace-package modules.
requires an explicit `MIGRATOR_DATABASE_URL` whose role differs from
`DATABASE_URL` before opening an engine; neither checkpoint application nor
startup revision verification ever falls back to application credentials. It
pins Alembic's effective `sqlalchemy.url` to that attested migrator URL and
passes only the literal approved target. A missing, shared, or application-role
migrator credential refuses before the live-revision read or Alembic upgrade.
The application and migrator URLs must use the same normalized host and port,
in addition to the same database and their distinct exact roles. Canonical
deploys discard ambient database URL/component overrides so preflight and both
migration checks read the same deployed `.env` that systemd will load. They
also discard Python import controls and invoke database-bearing Python helpers
and dependency installation with a minimal environment; checkpoint Git
identity uses the fixed system Git binary with global/system configuration
disabled and supplies only the exact selected source root as `safe.directory`,
so a root-owned or deployment-user-owned fixed checkout remains attestable
without trusting any ambient repository. Health attempt and delay controls are bounded decimal integers
before they can reach shell arithmetic.
The application never shells out to Git.

Immediately after the checkpoint succeeds and before any service restart,
deploy runs `scripts/converge_runtime_acls.py` from a private archive of the
approved commit together with `deploy/postgres/app_acl_policy.sql` and
`deploy/postgres/converge_runtime_object_acls.sql`. Each archived regular file
is digest-compared with its committed blob. The helper opens the deployed
`.env` once with no-follow semantics, requires it to be a regular file, binds
both database URLs to the exact environment identities, and invokes `psql` as
the migrator in one transaction. A symlinked environment file, app credential,
ambient URL override, different endpoint, wrong database/role, policy failure,
or `psql` failure leaves deployment state
blocked and prevents restart.

If release-metadata publication itself fails, durable blocked state stays
present and no new-tree unit may start. If preflight, dependency preparation,
migration, restart, readiness, or the operator process later fails, source and
release metadata remain aligned at the selected new commit and the durable
state remains fail-closed; interruption therefore cannot erase the guard
before restart/readiness. The helper never rolls metadata back independently
and never uses destructive Git reset. It clears state only after the restarted
service returns exact readiness; a restart/readiness failure stops the failed
backend service. Recovery is a corrected forward release or an explicitly
designed atomic code-and-identity restoration—not an identity-only rollback
and never an automatic schema downgrade.

`scripts/deploy.sh --dry-run` makes no deployed-state change. It requires the
live checkout to be clean and on the target branch, resolves one remote branch
to a full SHA, fetches that object, and materializes only its unit manifest,
migration policy, and manifest-selected systemd paths from an immutable Git
archive. Regular-file checks reject candidate symlinks before parsing or
hashing, and each staged digest is compared with the exact committed blob.
Release-tree attestation also refuses every tracked symlink or gitlink, so a
later approved Python/import/build path cannot escape the selected Git object.
Trusted `tar`, `sha256sum`, and `systemd-analyze` inspect the committed unit manifest, migration policy,
and exact unit bytes. Candidate Python imports, build backends, pip/Alembic,
live database/configuration, `.env`, operator home, agent, and candidate-code
network access are deliberately omitted. Trusted Git may contact the configured
remote to resolve and fetch the exact commit; no fetched executable code is
run. The output binds approval to `APPROVED_RELEASE_SHA=<sha>` and prints the
three additional migration approval inputs required for the subsequent deploy;
it explicitly reports that candidate-dependent application checks, live
migration state, and readiness were not performed. Staging and production
refuse startup without a full SHA and literal migration checkpoint;
development/test may explicitly use their named placeholder. Existing
deployments must have the guard-aware unit bundle installed before using this
contract—do not invent a placeholder SHA to bridge rollout.

`scripts/install_systemd_units.sh` provides the operator path. Every invocation
requires `--environment production|staging` and an explicit full
`--source-ref`; omitted mode safely defaults to dry-run, while mutation requires
explicit `--apply`. The selected environment maps to one fixed release
checkout. Its exact unit set comes from that commit's
`deploy/systemd/runtime-units.manifest`; the current manifest names the backend,
singleton news service/timer, and product-analytics prune service/timer. Before
preflight or mutation, the installer requires that checkout to be clean and
exactly at the source ref. Git
replacement refs and tracked `assume-unchanged`/`skip-worktree` flags are
refused, and committed blobs are read with replacement-object processing
disabled. The installer materializes every selected unit from that exact Git
commit into a new root-owned mode-0700 directory under `/run`, validates a
SHA-256 manifest there, and performs syntax checks, comparisons, and installs
only from those immutable staged bytes. A later worktree mutation cannot alter
the install candidate, and no mutable worktree unit path is reopened. It never
infers approval from another live checkout. Production and staging are
independently promoted, so run and approve them separately; their SHAs may
differ.

Unit filenames are globally unique across the manifest. Production entries
must not use the `-staging` suffix and staging entries must use it, preventing
one environment from replacing the other's destination. Canonical installer
tools use a fixed trusted path and a scrubbed Git environment. Explicit apply
re-executes under a non-blocking per-environment `flock`, so concurrent applies
cannot interleave pending state or destination replacements. Only the
root-owned re-exec may honor its lock marker, and the pending-state directory
is fixed rather than caller-selectable.

After provenance succeeds, the installer validates only the exact Git object,
unit digest manifest, and staged unit syntax. It does not execute candidate
checkout Python, application preflight, Alembic, or build metadata. `--dry-run`
then exits without changing systemd destinations or durable pending state.
Explicit `--apply` writes a durable pending record
under `/var/lib/verdaxis/systemd-units`, prepares every destination replacement
before changing a unit, and restores prior bytes if replacement is only
partially applied. It installs only staged bytes and always calls `systemctl
daemon-reload`; pending state is removed only after a successful reload, so a
reload failure leaves the installed bundle and durable retry marker for the
next apply. It
never enables, starts, or restarts a service or timer. Installation, daemon
reload, and timer enablement remain operator-held live actions.

Deploys categorically reject dirty trees before and after preparation because
a commit SHA cannot identify modified source. The readiness gate parses JSON
and requires exact `status="ok"`, `db="ok"`, target environment, and full
deployed SHA.
A rollback is a clean forward revert commit published as a new release through
the same preflight, migration, release-artifact, restart, and health gates.
Never automatically run an Alembic downgrade or edit `.runtime-release.env` to
impersonate an older checkout; schema rollback requires a separately reviewed,
forward-compatible corrective migration.

## Database roles and session timeouts

Deployed identities are exact:

| Environment | Database | Runtime app role | Migrator role |
|---|---|---|---|
| production | `verdaxis` | `verdaxis_app` | `verdaxis_migrator` |
| staging | `verdaxis_staging` | `verdaxis_app_staging` | `verdaxis_migrator_staging` |

Both environments require PostgreSQL URLs, explicit non-default passwords in
both the application and migrator URLs, a non-default JWT of at least 32
characters, and disabled auth bypass. Missing, empty, whitespace-only after URL
decoding (including percent-encoded whitespace), and known placeholder
passwords are rejected with field-only errors that never echo URL credentials. SQLite,
missing migrator URLs, shared app/migrator roles, cross-environment identities,
and every URL query parameter are rejected. Runtime and migration startup each
attest the exact `current_database()`, `current_user`, LOGIN/NOINHERIT and
non-superuser/non-createdb/non-createrole/non-replication/non-bypass-RLS role
properties, plus the absence of role memberships.

Each application connection sets:

```text
statement_timeout=30s
lock_timeout=3s
idle_in_transaction_session_timeout=60s
```

Deployed Alembic uses the required `MIGRATOR_DATABASE_URL` with a separate
migrator role. Direct development/test Alembic retains its settings-bounded
local fallback, but the deployment checkpoint and revision-verifier entry
points require explicit migrator credentials in every environment. Their
bounded policy is 300s / 30s / 300s respectively so schema changes have room
to complete without inheriting the application pool policy. The role/session
settings are applied by the asyncpg connection and are also intended to be
set at the PostgreSQL role level during integration:

```sql
ALTER ROLE verdaxis_app SET statement_timeout = '30s';
ALTER ROLE verdaxis_app SET lock_timeout = '3s';
ALTER ROLE verdaxis_app SET idle_in_transaction_session_timeout = '60s';
ALTER ROLE verdaxis_migrator SET statement_timeout = '300s';
ALTER ROLE verdaxis_migrator SET lock_timeout = '30s';
ALTER ROLE verdaxis_migrator SET idle_in_transaction_session_timeout = '300s';
```

The idempotent executable artifacts are
`deploy/postgres/bootstrap_roles.sql` and `deploy/postgres/validate_roles.sql`.
They provision app, migrator, and read-only backup roles; transfer public
database/schema and application table/sequence ownership to the migrator;
remove every protected-role membership edge (including inherited superuser and
`SET ROLE` paths); revoke stale direct/default ACLs; and reconstruct exact
least-privilege grants. A central governed-object relation covers every
ordinary table, partitioned table, child partition, and sequence. Bootstrap
discovers expanded ACL grantees with `aclexplode`, revokes every non-owner
grantee—including PUBLIC, app, backup, and unrelated roles—with intentional
deterministic `CASCADE`, separately discovers every explicit
`pg_attribute.attacl` column grant, revokes it including PUBLIC, then grants
only the declarations in `deploy/postgres/app_acl_policy.sql`. The same
deterministic repair removes delegated object privileges and role memberships,
including grant/admin-option dependents, before rebuilding authority. The app
and backup are also stripped from control and extension objects before governed
grants are rebuilt. The normalized expanded database and `public` schema
ACLs allow only the migrator owner plus the intended app/backup grants; every
unrelated explicit grantee is revoked with intentional `CASCADE`, including
privileges that grantee delegated onward. PostgreSQL's ownership authority is
represented by the migrator owner, so a redundant explicit
`pg_database_owner` schema ACL is removed rather than treated as extra access.
Validation compares the complete expanded ACL sets, including grantor and
grantability, instead of checking only named roles or relying on ACL array
ordering. Default ACL normalization covers both global
`defaclnamespace=0` rows and additive `public`-schema rows for app, migrator,
and backup owners across every PostgreSQL 17 default-ACL object type. It
discovers grantees with `aclexplode`, removes stale global and `IN SCHEMA
public` authority, restores PostgreSQL's hard-wired global defaults, and then
reconstructs only the intended migrator-owned backup defaults. A future public
table or sequence therefore gives the app no authority until an exact policy
entry is reviewed; backup receives only table/sequence `SELECT`.

The app policy is fail closed. Normal application DML exists only on explicitly
listed tables. `organizations` has table-level `SELECT`/`DELETE`, plus named
column `INSERT`/`UPDATE`. Its exact insert list is `id`, `name`, `domain`,
`type`, `supplier_tier`, `tax_id`, `country_code`, and
`created_at`; its exact update list is `name`, `domain`, `type`,
`supplier_tier`, `tax_id`, and `country_code`. Signup relies on the database
default for `verification_status`; verification and provenance columns require
a trusted writer and are denied to the runtime app, as is every absent or
future column until reviewed.
`audit_logs` and `user_status_transitions` are explicitly append-only
(`SELECT, INSERT`) for normal request transactions, with no app
`UPDATE`/`DELETE` and no backup write. The current sequence policy is empty. Policy
entries for integration-owned `seed_runs` and `market_row_quarantines` become
read-only only if those tables exist; `organization_market_approvals` is not
granted to the runtime at all. Their absence cannot create a broad grant. `alembic_version`,
`spatial_ref_sys`, extension objects, seed/quarantine/approval controls,
market evidence/provenance, and operator data are not app-writable. Unknown
governed tables and sequences receive no app grant.
Services that eventually need to mutate protected security or market state
must use an integration-owned trusted writer boundary; widening the ordinary
app role is not an integration shortcut.

Validation checks exact ownership, role properties, memberships, effective
privileges, expanded database/schema/object/column ACL equality including
grantor and grantability, exact default ACL equality, excluded-object
immutability, and exact per-database timeouts. The disposable PostgreSQL proof
injects delegated object and column grants, converges bootstrap twice, and then
uses the raw app role to execute a signup transaction and an audited
organization/admission mutation, while proving it cannot change organization
provenance, rewrite audit/status history, rewrite seed/quarantine controls,
`SET ROLE`, delegate grants, or retain an undeclared column ACL. Backup column
writes are likewise rejected.
Run them as the database owner with explicit psql variables, for example:

```bash
psql --dbname "$ADMIN_DATABASE_URL" \
  -v database_name=verdaxis_test \
  -v app_role=verdaxis_app_test \
  -v migrator_role=verdaxis_migrator_test \
  -v backup_role=verdaxis_backup_test \
  -f deploy/postgres/bootstrap_roles.sql
psql --dbname "$ADMIN_DATABASE_URL" \
  -v database_name=verdaxis_test \
  -v app_role=verdaxis_app_test \
  -v migrator_role=verdaxis_migrator_test \
  -v backup_role=verdaxis_backup_test \
  -f deploy/postgres/validate_roles.sql
```

Passwords are intentionally absent. Provision them through the secret manager.
This pass deliberately does not add RLS. SQLAlchemy engines use
`hide_parameters=True`; database error logs contain only exception class,
SQLSTATE, request ID, and route. Market lock timeout, deadlock, and
serialization failures return a generic bounded `503` with `Retry-After: 1`.

## Integration safety

`TEST_API_URL` is mandatory. Mutating helpers require both:

```text
ALLOW_TEST_MUTATIONS=I_UNDERSTAND_TEST_MUTATIONS
TEST_RUNTIME_ENV=staging|disposable
```

The attestation is positive, not inferred from a hostname. `staging` accepts
only `https://api-staging.verdaxis.exchange` on port 443 (or the explicitly
documented `http://127.0.0.1:8001` loopback target when a test genuinely needs
it). `disposable` accepts only an explicitly ported loopback target with a
database name ending in `_test`; it reserves/rejects both live ports 8000 and
8001. Production hostnames, aliases, userinfo, scheme tricks, IPv6 aliases,
the production VPS IP `144.126.151.136`, and absent attestation are
categorically refused. Read-only helpers still require an explicit URL but do
not require the mutation acknowledgement.

Prefer the disposable PostgreSQL 17/PostGIS 3.6 runner (the deployment is
PostgreSQL 17.9/PostGIS 3.6.2; the live-version preflight is read-only). It
creates a unique Docker container,
uses a Docker-assigned loopback port, runs migration and drift checks, and
removes only its own container. Redis and the compose topology remain intact.
The helper image is digest-pinned and verifies numeric PostgreSQL 17 and
`PostGIS_Lib_Version()` 3.6 values before migrations.

## Seeder safety and credential rotation

Seed entrypoints use `app.seeds.safety` and require four independent values:
`SEED_DATABASE_URL`, `ALLOW_SEED_MUTATIONS=I_UNDERSTAND_SEED_MUTATIONS`,
`SEED_RUNTIME_ENV=staging|disposable`, and an exact `SEED_TARGET_DATABASE`.
Only loopback targets are accepted. Production/system databases and superuser
roles are always denied; disposable names end in `_test`, staging is exactly
`verdaxis_staging`, and availability-window literals must already be canonical.
Every connection-routing URL query parameter (including `host` and `database`)
is rejected. Seeders and Alembic attest `current_database()` before mutations.

The removed tracked credential must be rotated as a separate operator action.
Current-tree source cleanup is insufficient, and repository history rewriting
is not a substitute for credential rotation. The approved operator procedure
is: create and verify a replacement least-privilege credential, update the
secret store and deployed environment, attest the replacement role, then revoke
the exposed credential. No live credential, database, or service is changed by
this branch.

## Health and credentialed CORS

`/health/live` is process-only. `/health/ready` (and the legacy `/health`
alias) performs a bounded database probe and returns a sanitized 503 on timeout
or failure. Both success and failure JSON include only the validated
`environment` and `release_sha`, allowing an external monitor to compare the
immutable expected artifact. Off-host monitors and deploy checks use
`/health/ready`; liveness is not a deployment/readiness signal.

The deploy checker does not substring-match this response. It parses JSON and
requires exact `status="ok"`, `db="ok"`, target environment, and expected full
SHA; wrong database status, environment, SHA, malformed JSON, and degraded
status all fail.

Credentialed CORS has exact environment allowlists: production permits only
`https://verdaxis.exchange` and `https://app.verdaxis.exchange`; staging only
`https://staging.verdaxis.exchange`; development/test only enumerated localhost
origins. Cross-environment values, URL credentials/paths, and wildcards fail
settings initialization.

## Migration verification

`scripts/verify_migrations.sh` requires an explicit database URL ending in
`_test`, runs `alembic upgrade head`, proves the revision is exactly at a head
with `alembic current --check-heads`, and runs `alembic check`. This is a
disposable test-schema completeness harness, not the production/staging
deployment command. Production and staging never traverse to `head`; they use
the source-attested literal checkpoint contract described above.

Alembic compares columns, foreign keys, indexes, types, defaults, and comments.
`app.migration_drift` excludes only schema-qualified PostGIS/geocoder
fingerprints and documented historical legacy tables. External columns,
indexes, and constraints include exact schema/table/name/shape fingerprints;
there is no name-only unique-constraint suppression. Its normalizations are
narrow: exact Enum/String length and value fingerprints, and exact
Python-owned default fingerprints where metadata intentionally has no SQL
server default. Explicit SQL defaults remain comparable; no category-wide
false callback exists. The only reflected RFQ exclusions are exact
security/market integration fingerprints and are temporary until linearized.
A SQLite comparator test proves that an omitted foreign key produces an
`add_fk` operation.

The runtime metadata migration fixes actual fresh-schema drift, including the
inventory fuel width and stale orderbook availability/default/nullability/date
columns, rather than hiding those differences. It retains
`delivery_window_start`/`delivery_window_end` because realistic orderbook
seeding uses them, backfills before tightening required fields, and never
drops data in this branch. The exact redundant single-column orderbook indexes
(`ix_orderbook_orders_side`, `ix_orderbook_orders_status`, and
`ix_orderbook_orders_org`) are removed by the migration and model; the active
slice lookup index remains. Downgrade refuses narrowing `fuel_type` to eight
characters while values such as `Biomethane` exist and reports the required
cleanup, rather than failing with an opaque truncation error. Roundtrip coverage
builds the down revision independently and compares every touched column and
index after downgrade; in particular, `commissions.match_id` is restored as
`NOT NULL`. This migration contains runtime metadata normalization only: it
does not create/drop tables or include KYC, security, or market feature DDL.

## Alembic integration order

The standalone runtime head `rh_20260720_runtime_metadata` descends from
`pa_20260715_analytics_facts` and remains there in this isolated branch. The
combined-tree integration linearization is:

```text
pa_20260715_analytics_facts
  -> rh_20260720_runtime_metadata
  -> sec_20260720_identity (reparented during integration)
  -> remaining security revisions
  -> sec_20260720_device
  -> market-owned revisions when integrated
```

Do not reparent runtime after security in this branch. Integration reparents
only `sec_20260720_identity` onto `rh_20260720_runtime_metadata`, preserves the
remaining security chain through `sec_20260720_device`, then composes
market-owned DDL after that head. Do not create an empty merge-head workaround.
The runtime migration remains runtime metadata normalization only.

Checkpoint policy is the cutover boundary, not merely a graph-validity check.
This branch permits `pa_20260715_analytics_facts ->
rh_20260720_runtime_metadata` and an idempotent `rh -> rh` deployment. Combined
integration must add separately reviewed literal transitions for the intended
security identity, security device, and market quarantine pauses only after
those revisions exist in the combined source. An operator approves exact
source SHA, expected live revision, and one target for each promotion; a later
head cannot be reached accidentally through an earlier approval. Backend unit
preflight compares the live revision with the published checkpoint rather than
with source `head`, so an intentional pause remains restartable and
readiness-gated even when selected source contains later revisions. Do not
import or invent the security/market migrations in this runtime branch.

The combined security tree must extend the ACL policy at the same checkpoint
that introduces admission/KYC schema. In particular, it must review exact
table authority for `pending_registrations`, `organization_join_requests`, and
`refresh_sessions`, and replace broad `users` mutation with reviewed column
entries before adding identity/KYC fields such as
`email_verification_token_hash`, `email_verification_token_expires_at`,
`kyc_organization_id`, `kyc_external_evidence_reference`, `kyc_review_note`,
`kyc_reviewed_by`, and `kyc_reviewed_at`. Only columns emitted by the combined
signup and trusted administrator workflows are added; absence or a newly added
column must remain denied. Organization provenance promotion is not one of
those app columns.

The current legacy market-signal ingestion CLI cannot justify app-role writes
to `market_signal_ingestion_runs`, `market_indications`, `fair_price_bands`, or
`physical_stems`. Combined integration must assign that CLI a separately
approved operator identity and exact ACLs before deployed use. Seed,
quarantine, and market-evidence authority stays closed in this branch.

This isolated runtime branch's immutable installer manifest intentionally
contains five units per environment. The combined security tree must add the
audited unit bytes and exact manifest entries for
`verdaxis-auth-maintenance.service` and
`verdaxis-auth-maintenance.timer` to production, plus
`verdaxis-auth-maintenance-staging.service` and
`verdaxis-auth-maintenance-staging.timer` to staging, while preserving exact
environment-specific provenance. The installer consumes the committed manifest
and immutable archive, so this extension requires no hard-coded installer list
and no mutable checkout reopen. Until the migration chain, checkpoint policy,
protected-state writer paths, and these unit bytes are integrated, this branch
must not be represented as an
independently deployable combined runtime+security release.

The local-monitor branch's reviewed identity-only EXIT rollback is also an
integration blocker: after source has advanced, restoring only an old release
SHA recreates stale identity over new bytes. The combined deploy path must
remove that rollback and preserve the forward-aligned identity under the
deployment guard, or atomically restore both code and identity. It must not use
a destructive Git reset or automatically downgrade schema. The canonical
successful readiness payload shared with that monitor is exactly
`status="ok"`, `db="ok"`, the target environment, and the full release SHA.

Integration must retain the KYC route-level body bound and bounded streaming
regression coverage for missing and lying `Content-Length`. Gemini document
analysis remains advisory only: pass, fail, and unavailable results all leave
KYC pending and never activate or reject an account. Only trusted administrator
approve/reject routes are authoritative. Integration must also preserve the
absence of per-worker news schedulers and the removed legacy compliance-ledger,
compliance-verify, and dashboard-health routes; every scheduled job has one
external scheduler. Process-local SSE transport remains an explicit combined
integration problem; this branch preserves the four-worker systemd, settings,
and connection-budget contract and does not hide the transport issue by
reducing workers.

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

Each unit runs `alembic current --check-heads` before Uvicorn and passes the
same `UVICORN_WORKERS=4` value used by application pool math,
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

The units also require the gitignored `.runtime-release.env` artifact. After a
successful fast-forward and migration, `scripts/deploy.sh` resolves the full
40-hex commit ID from the checked-out artifact, writes `ENVIRONMENT` and
`RELEASE_SHA` to a mode-0600 temporary file, then atomically renames it before
the service restart. The application never shells out to Git. Staging and
production refuse startup without a full SHA; development/test may explicitly
use their named placeholder. Existing deployments need the updated unit and a
deploy-helper run together—do not invent a placeholder SHA to bridge rollout.

`scripts/install_systemd_units.sh` provides the operator path. Every invocation
requires `--environment production|staging` and an explicit full
`--source-ref`; omitted mode safely defaults to dry-run, while mutation requires
explicit `--apply`. The selected environment maps to one fixed release
checkout. Before preflight or mutation, the installer requires that checkout
to be clean and exactly at the source ref. Git
replacement refs and tracked `assume-unchanged`/`skip-worktree` flags are
refused, and committed blobs are read with replacement-object processing
disabled. The installer then reads every selected backend/news unit from that
same Git commit and attests the working bytes and SHA-256 digest against the
release artifact. It never sources units from the invoking worktree or infers
approval from another live checkout. Production and staging are independently
promoted, so run and approve them separately; their SHAs may differ.

After provenance succeeds, the installer validates the selected environment's
release identity, application database/CORS/auth configuration, exact Alembic
heads, and unit syntax. `--dry-run` then exits without changes. Explicit
`--apply` installs only changed units and calls `systemctl daemon-reload` only
after a change. It never enables, starts, or restarts a service or timer.
Installation, daemon reload, and timer enablement remain operator-held live
actions.

Deploys categorically reject dirty trees before and after preparation because
a commit SHA cannot identify modified source. The readiness gate parses JSON
and requires exact `status="ok"`, target environment, and full deployed SHA.
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
characters, and disabled auth bypass. Missing and known placeholder passwords
are rejected with field-only errors that never echo URL credentials. SQLite,
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

Alembic can use `MIGRATOR_DATABASE_URL` with a separate migrator role. Its
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
least-privilege grants. The normalized expanded database and `public` schema
ACLs allow only the migrator owner plus the intended app/backup grants; every
unrelated explicit grantee is revoked. PostgreSQL's ownership authority is
represented by the migrator owner, so a redundant explicit
`pg_database_owner` schema ACL is removed rather than treated as extra access.
Validation compares the complete expanded ACL sets, including grantor and
grantability, instead of checking only named roles or relying on ACL array
ordering. App-owned objects exclude `alembic_version`,
`spatial_ref_sys`, and extension-owned objects. The app receives table
`SELECT/INSERT/UPDATE/DELETE` and sequence `USAGE/SELECT/UPDATE`; backup receives
only table/sequence `SELECT`. Validation checks exact ownership, role
properties, memberships, effective privileges, unexpected database/schema/
object grantees, exact default ACL set equality (including grantor and grant
option), backup write absence, excluded-object immutability, and exact
per-database timeouts.
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
requires the exact success status, target environment, and expected full SHA;
wrong environment, wrong SHA, malformed JSON, and degraded status all fail.

Credentialed CORS has exact environment allowlists: production permits only
`https://verdaxis.exchange` and `https://app.verdaxis.exchange`; staging only
`https://staging.verdaxis.exchange`; development/test only enumerated localhost
origins. Cross-environment values, URL credentials/paths, and wildcards fail
settings initialization.

## Migration verification

`scripts/verify_migrations.sh` requires an explicit database URL ending in
`_test`, runs `alembic upgrade head`, proves the revision is exactly at a head
with `alembic current --check-heads`, and runs `alembic check`.

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
`pa_20260715_analytics_facts`. The exact integration linearization is:

```text
pa_20260715_analytics_facts
  -> sec_20260720_identity
  -> sec_20260720_boundaries
  -> mi_20260720_market_integrity
  -> rh_20260720_runtime_metadata (rebased, or folded into one combined migration)
```

Rebase the market branch onto the security head, resolve any shared model
ownership there, then rebase this runtime branch onto that single combined
line. Do not create an empty merge-head workaround. The runtime migration is
independently testable now and can be folded into the final combined migration
if exact model alignment makes that simpler.

Integration must retain the KYC route-level body bound and bounded streaming
regression coverage for missing and lying `Content-Length`. Gemini document
analysis remains advisory only: pass, fail, and unavailable results all leave
KYC pending and never activate or reject an account. Only trusted administrator
approve/reject routes are authoritative. Integration must also preserve the
absence of per-worker news schedulers and the removed legacy compliance-ledger,
compliance-verify, and dashboard-health routes; every scheduled job has one
external scheduler.

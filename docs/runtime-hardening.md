# Runtime and test hardening

## Database connection budget

Production and staging share one PostgreSQL cluster. `UVICORN_WORKERS` is the
single worker-count setting consumed by systemd and the pool validator; each
service currently uses four workers and the same conservative SQLAlchemy defaults:

```text
services × workers × (pool_size + max_overflow) + maintenance reserve
2 × 4 × (2 + 1) + 20 = 44 ≤ PostgreSQL max_connections=100
```

`Settings` validates this shared aggregate for both deployed environments. Startup
queries `current_user`, `pg_roles.rolsuper`, and `SHOW max_connections`; an
effective URL/connected-role mismatch, superuser connection, or aggregate over
the observed server limit aborts startup. It rejects
non-positive values, a reserved budget at or above the PostgreSQL limit, and a
worker capacity that exceeds the available connection budget. The reserve is
for migrations, administration, and other processes; it is not a promise that
PostgreSQL creates those clients.

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
artifacts only; this change does not install or deploy them.

The units also require the gitignored `.runtime-release.env` artifact. After a
successful fast-forward and migration, `scripts/deploy.sh` resolves the full
40-hex commit ID from the checked-out artifact, writes `ENVIRONMENT` and
`RELEASE_SHA` to a mode-0600 temporary file, then atomically renames it before
the service restart. The application never shells out to Git. Staging and
production refuse startup without a full SHA; development/test may explicitly
use their named placeholder. Existing deployments need the updated unit and a
deploy-helper run together—do not invent a placeholder SHA to bridge rollout.

## Database roles and session timeouts

The application URL must use a least-privilege runtime role (for example
`verdaxis_app`, not `postgres`). Each application connection sets:

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
table/sequence ownership to the migrator; grant existing and default table and
sequence privileges; restrict database DDL authority to the migrator; revoke
public defaults; and set per-database timeouts.
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
This pass deliberately does not add RLS. Market lock timeout, deadlock, and
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

The removed tracked credential must be rotated as a separate operator action:
create/verify a replacement secret and least-privilege role, update the secret
store and deployed environment, then revoke the exposed credential. No live
credential, database, or service is changed by this branch.

## Health and credentialed CORS

`/health/live` is process-only. `/health/ready` (and the legacy `/health`
alias) performs a bounded database probe and returns a sanitized 503 on timeout
or failure. Both success and failure JSON include only the validated
`environment` and `release_sha`, allowing an external monitor to compare the
immutable expected artifact. Off-host monitors and deploy checks use
`/health/ready`; liveness is not a deployment/readiness signal.

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
cleanup, rather than failing with an opaque truncation error.

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

The security branch owns the KYC route/body workflow. Integration must retain
its route-level body bound while preserving this branch's bounded streaming
regression coverage for missing and lying `Content-Length`; neither branch
should overwrite the other's upload semantics.
Gemini document analysis remains advisory only: it must never activate an
account by itself. A trusted administrator approves or rejects KYC after
review, and integration must preserve that human-approval boundary.

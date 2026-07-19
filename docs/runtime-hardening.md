# Runtime and test hardening

## Database connection budget

Production and staging both run four Uvicorn workers with the same conservative
SQLAlchemy defaults:

```text
workers × (pool_size + max_overflow) + reserved connections
4 × (5 + 2) + 20 = 48 ≤ PostgreSQL max_connections=100
```

`Settings` validates this aggregate for both deployed environments. It rejects
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

The checked-in systemd units use `MemoryHigh=512M` as the pressure threshold and
`MemoryMax=768M` as the hard ceiling. These are measured-safe starting values
for four workers plus bounded uploads; monitor actual service usage before
raising either limit.

## systemd topology and preflight

`deploy/systemd/verdaxis-backend.service` binds production to
`127.0.0.1:8000`; `verdaxis-backend-staging.service` binds staging to
`127.0.0.1:8001`. The public Caddy endpoints reverse-proxy to those loopback
ports. Both units order after `network-online.target` and require/order on
`postgresql.service`; neither depends on Docker.

Each unit runs `alembic current --check-heads` before Uvicorn, uses four workers,
sets `PYTHONDONTWRITEBYTECODE=1`, and applies a read-only systemd sandbox. No
broad source-tree `ReadWritePaths` grant is present. The checked-in units are
artifacts only; this change does not install or deploy them.

## Integration safety

`TEST_API_URL` is mandatory. Mutating helpers require both:

```text
ALLOW_TEST_MUTATIONS=I_UNDERSTAND_TEST_MUTATIONS
TEST_RUNTIME_ENV=staging|disposable
```

The attestation is positive, not inferred from a hostname. `staging` accepts
only `https://api-staging.verdaxis.exchange`; `disposable` accepts only a
loopback target. Production hostnames, the production VPS IP
`144.126.151.136`, and every loopback URL on port 8000 are categorically
refused. Read-only helpers still require an explicit URL but do not require the
mutation acknowledgement.

Prefer the disposable PostGIS runner. It creates a unique Docker container,
uses a Docker-assigned loopback port, runs migration and drift checks, and
removes only its own container. Redis and the compose topology remain intact.

## Migration verification

`scripts/verify_migrations.sh` requires an explicit database URL ending in
`_test`, runs `alembic upgrade head`, proves the revision is exactly at a head
with `alembic current --check-heads`, and runs `alembic check`.

Alembic compares columns, foreign keys, indexes, types, defaults, and comments.
`app.migration_drift` excludes only explicitly enumerated PostGIS/geocoder
system tables and documented historical legacy tables. Its two normalizations
are narrow: Python enums stored as non-native strings, and Python-owned
defaults where metadata intentionally has no SQL server default. Explicit SQL
defaults remain comparable. A SQLite comparator test proves that an omitted
foreign key produces an `add_fk` operation.

The runtime metadata migration fixes actual fresh-schema drift, including the
inventory fuel width and stale orderbook availability/default/nullability/date
columns, rather than hiding those differences.

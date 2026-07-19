# Runtime and test hardening

## Database pool capacity

`DB_POOL_SIZE`, `DB_MAX_OVERFLOW`, and the timeout/recycle settings configure
the SQLAlchemy pool for each application worker. SQLite keeps its existing
unpooled behavior. The defaults are intentionally conservative for a
PostgreSQL deployment with `max_connections=100`:

```text
workers × (pool_size + max_overflow) + reserved connections
4 × (5 + 2) + 20 = 48 ≤ 100
```

`Settings` rejects non-positive values, a reserved budget at or above the
PostgreSQL limit, and any worker capacity that exceeds the available budget.
When changing the Uvicorn worker count, set `DB_POOL_WORKERS` to the same
number and keep this inequality true. The reserved 20 connections cover
migrations, admin/maintenance clients, and other processes; they are not a
promise that PostgreSQL will create those clients.

## systemd services

`deploy/systemd/verdaxis-backend.service` and
`deploy/systemd/verdaxis-backend-staging.service` are checked-in templates for
the live paths. They bind Uvicorn to `127.0.0.1:8000` for the reverse proxy,
wait for network/Docker startup ordering, verify the environment and database
before launch, use four workers, send `SIGTERM` with a 30-second grace period,
and apply resource limits plus a practical read-only/system-call sandbox.
These files are artifacts only; this change does not install or deploy them.

## Integration safety

`TEST_API_URL` is mandatory. No test or helper silently targets localhost,
and `verdaxis.exchange` production API hosts are refused. The integration
directory contains mutating tests, so any non-loopback URL also requires the
exact acknowledgement:

```text
ALLOW_REMOTE_TEST_MUTATIONS=I_UNDERSTAND_REMOTE_TEST_MUTATIONS
```

Prefer a local disposable backend and PostGIS database. The PostGIS helper
uses a uniquely named container/project label and Docker-assigned loopback
port, runs Alembic verification, and cleans up only its own container. It
never stops or reuses an existing container.

## Migration verification

`scripts/verify_migrations.sh` requires an explicit database URL ending in
`_test`, runs `alembic upgrade head`, then runs `alembic check`. CI performs
both operations against its fresh PostGIS service before the PostgreSQL
correctness tests, so a migration that cannot build a fresh schema or omits a
current ORM-owned table/column fails the job. The Alembic comparison ignores
PostGIS extension tables, historical legacy tables, and old type/index/FK
differences that are deliberately outside the current ORM metadata; these
otherwise make a fresh head database report noise rather than actionable
application drift.

#!/usr/bin/env bash
# Phase 10 order-expiry reminder job against a disposable migrated schema.
#
# The target needs the real Alembic schema (triggers, constraints, the JSON
# notification column), the committed ACL bundle and the retained Python
# reminder functions, so it runs here rather than in run_postgres.sh. Docker
# binds a loopback port; the container is removed on exit. Never point this at
# staging or production.
set -euo pipefail
: "${RUST_TEST_PYTHON:?Set an absolute Python path with the project database dependencies}"
export SQLX_OFFLINE=true
RELEASE_SHA=${RELEASE_SHA:-$(git rev-parse HEAD)}
export RELEASE_SHA
image='postgis/postgis:17-3.6-alpine@sha256:49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272'
container=''
log=''
cleanup() {
  local status=$?
  if [[ -n "$container" ]]; then docker rm -f -v "$container" >/dev/null 2>&1 || true; fi
  if [[ -n "$log" ]]; then rm -f "$log"; fi
  exit "$status"
}
trap cleanup EXIT
container=$(docker run --platform linux/amd64 -d --rm -e POSTGRES_PASSWORD=disposable_admin_pw -p 127.0.0.1::5432 "$image")
ready=0
for _ in {1..120}; do
  if docker exec "$container" pg_isready -h 127.0.0.1 -U postgres >/dev/null 2>&1; then ready=1; break; fi
  sleep 1
done
(( ready == 1 )) || { echo "PostgreSQL did not become ready" >&2; exit 1; }
port=$(docker port "$container" 5432/tcp | head -1)
port=${port##*:}
docker exec -i "$container" psql -U postgres -v ON_ERROR_STOP=1 <<'SQL'
CREATE ROLE rust_app LOGIN NOINHERIT PASSWORD 'disposable_app_pw';
CREATE ROLE rust_migrator LOGIN NOINHERIT PASSWORD 'disposable_migrator_pw';
CREATE ROLE rust_backup LOGIN NOINHERIT PASSWORD 'disposable_backup_pw';
CREATE DATABASE verdaxis_reminders_test OWNER rust_migrator;
\connect verdaxis_reminders_test
CREATE EXTENSION postgis;
ALTER SCHEMA public OWNER TO rust_migrator;
REVOKE ALL ON SCHEMA public FROM PUBLIC;
GRANT USAGE ON SCHEMA public TO rust_app, rust_backup;
SQL
database=verdaxis_reminders_test
migrator_url="postgresql+asyncpg://rust_migrator:disposable_migrator_pw@127.0.0.1:$port/$database"
ENVIRONMENT=test JWT_SECRET=test-secret-key-for-testing-minimum-32-chars \
  DATABASE_URL="$migrator_url" MIGRATOR_DATABASE_URL="$migrator_url" \
  "$RUST_TEST_PYTHON" -m alembic upgrade head
PGHOST=127.0.0.1 PGPORT="$port" PGDATABASE="$database" PGUSER=rust_migrator PGPASSWORD=disposable_migrator_pw \
  psql -X --single-transaction -v ON_ERROR_STOP=1 -v database_name="$database" \
  -v app_role=rust_app -v migrator_role=rust_migrator -v backup_role=rust_backup \
  -f deploy/postgres/converge_runtime_object_acls.sql >/dev/null

export RUST_REMINDERS_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/$database"
export RUST_REMINDERS_OWNER_DATABASE_URL="postgres://rust_migrator:disposable_migrator_pw@127.0.0.1:$port/$database"
export RUST_REMINDERS_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/$database"
export RUST_REMINDERS_PYTHON_DATABASE_URL="postgresql+asyncpg://rust_app:disposable_app_pw@127.0.0.1:$port/$database"

log=$(mktemp "${TMPDIR:-/tmp}/rust-reminder-tests.XXXXXX")
set +e
cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read --features contract-tests \
  --test phase10_reminders_postgres -- --include-ignored --test-threads=1 2>&1 | tee "$log"
cargo_status=${PIPESTATUS[0]}
set -e
if (( cargo_status != 0 )); then exit "$cargo_status"; fi
# A green run that executed no test, or ignored one, is a false pass.
if ! grep -Eq '^test result: ok\. [1-9][0-9]* passed; 0 failed; 0 ignored' "$log"; then
  echo "the reminder target ran no test or ignored one: refusing a green result" >&2
  exit 1
fi

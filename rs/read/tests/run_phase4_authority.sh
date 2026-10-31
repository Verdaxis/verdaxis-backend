#!/usr/bin/env bash
# Migrated exact-role authority acceptance, separate from the minimal workspace fixture.
set -euo pipefail
export SQLX_OFFLINE=true
export RELEASE_SHA=$(git rev-parse HEAD)
run() {
  local name=$1; shift
  printf 'GATE=%s SHA=%s UTC=%s\n' "$name" "$RELEASE_SHA" "$(date -u +%FT%TZ)"
  local rc=0
  "$@" || rc=$?
  printf 'GATE=%s EXIT=%s\n' "$name" "$rc"
  return "$rc"
}
container=''
trap 'if [ -n "$container" ]; then docker rm -f -v "$container" >/dev/null 2>&1; fi' EXIT
container=$(docker run --platform linux/amd64 -d --rm -e POSTGRES_PASSWORD=disposable_admin_pw -p 127.0.0.1::5432 postgis/postgis:17-3.6-alpine@sha256:49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272)
for i in {1..90}; do docker exec "$container" pg_isready -h 127.0.0.1 -U postgres >/dev/null 2>&1 && break; sleep 1; done
port=$(docker port "$container" 5432/tcp); port=${port##*:}
docker exec -i "$container" psql -U postgres -v ON_ERROR_STOP=1 <<'SQL'
CREATE ROLE verdaxis_app_staging LOGIN NOINHERIT NOSUPERUSER PASSWORD 'disposable_app_pw';
CREATE ROLE verdaxis_migrator_staging LOGIN NOINHERIT NOSUPERUSER PASSWORD 'disposable_migrator_pw';
CREATE ROLE verdaxis_backup_staging LOGIN NOINHERIT NOSUPERUSER PASSWORD 'disposable_backup_pw';
CREATE DATABASE verdaxis_authority_test OWNER verdaxis_migrator_staging;
\connect verdaxis_authority_test
CREATE EXTENSION postgis;
ALTER SCHEMA public OWNER TO verdaxis_migrator_staging;
REVOKE ALL ON SCHEMA public FROM PUBLIC;
GRANT USAGE ON SCHEMA public TO verdaxis_app_staging,verdaxis_backup_staging;
SQL
export ENVIRONMENT=test JWT_SECRET=test-secret-key-for-testing-minimum-32-chars
export DATABASE_URL="postgresql+asyncpg://verdaxis_migrator_staging:disposable_migrator_pw@127.0.0.1:$port/verdaxis_authority_test"
export MIGRATOR_DATABASE_URL="$DATABASE_URL"
run migrate "${RUST_TEST_PYTHON:-python3}" -m alembic upgrade head
export PGHOST=127.0.0.1 PGPORT="$port" PGDATABASE=verdaxis_authority_test PGUSER=verdaxis_migrator_staging PGPASSWORD=disposable_migrator_pw
run acl psql -X --single-transaction -v ON_ERROR_STOP=1 -v database_name=verdaxis_authority_test -v app_role=verdaxis_app_staging -v migrator_role=verdaxis_migrator_staging -v backup_role=verdaxis_backup_staging -f deploy/postgres/converge_runtime_object_acls.sql
printf 'INDEX_INVENTORY_BEGIN\n'
psql -X -At -v ON_ERROR_STOP=1 -c "SELECT json_build_object('table',tablename,'name',indexname,'definition',indexdef) FROM pg_indexes WHERE schemaname='public' ORDER BY tablename,indexname"
printf 'INDEX_INVENTORY_END\n'
export RUST_TEST_DATABASE_URL="postgres://verdaxis_app_staging:disposable_app_pw@127.0.0.1:$port/verdaxis_authority_test"
export RUST_TEST_MIGRATOR_DATABASE_URL="postgres://verdaxis_migrator_staging:disposable_migrator_pw@127.0.0.1:$port/verdaxis_authority_test"
unset DATABASE_URL MIGRATOR_DATABASE_URL PGPASSWORD
run phase4_db_authority cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read --features contract-tests --test phase4_db_authority

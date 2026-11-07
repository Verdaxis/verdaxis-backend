#!/usr/bin/env bash
# Real auth HTTP checks on one migrated local disposable PostgreSQL template.
set -euo pipefail
export SQLX_OFFLINE=true
RELEASE_SHA=$(git rev-parse HEAD)
export RELEASE_SHA
image='postgis/postgis:17-3.6-alpine@sha256:49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272'
template=verdaxis_abilities_test
container=''
cleanup() {
  local status=$?
  if [[ -n "$container" ]]; then docker rm -f -v "$container" >/dev/null 2>&1 || true; fi
  exit "$status"
}
trap cleanup EXIT
container=$(docker run --platform linux/amd64 -d --rm -e POSTGRES_PASSWORD=disposable_admin_pw -p 127.0.0.1::5432 "$image")
ready=0
for _ in $(seq 1 120); do
  if docker exec "$container" pg_isready -h 127.0.0.1 -U postgres >/dev/null 2>&1; then ready=1; break; fi
  sleep 1
done
[[ "$ready" == 1 ]] || { printf 'database did not become ready\n' >&2; exit 2; }
port=$(docker port "$container" 5432/tcp)
port=${port##*:}
docker exec -i "$container" psql -U postgres -v ON_ERROR_STOP=1 <<'SQL'
CREATE ROLE verdaxis_migrator_staging LOGIN NOINHERIT NOSUPERUSER PASSWORD 'disposable_role_pw';
CREATE ROLE rust_app LOGIN NOINHERIT NOSUPERUSER PASSWORD 'disposable_role_pw';
CREATE ROLE rust_backup NOLOGIN;
CREATE DATABASE verdaxis_abilities_test OWNER verdaxis_migrator_staging;
\connect verdaxis_abilities_test
CREATE EXTENSION postgis;
ALTER SCHEMA public OWNER TO verdaxis_migrator_staging;
GRANT USAGE ON SCHEMA public TO rust_app;
SQL
# shellcheck disable=SC2086
RUST_RUNNER_ENVIRONMENT=staging RUST_RUNNER_MIGRATOR_DATABASE_URL="postgresql://verdaxis_migrator_staging:disposable_role_pw@127.0.0.1:$port/$template" \
  ${CARGO_RUNNER:-} cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read \
  --features contract-tests --test phase10_migrate_postgres \
  runner_database_adopts_to_the_baseline -- --ignored
PGHOST=127.0.0.1 PGPORT="$port" PGDATABASE="$template" PGUSER=verdaxis_migrator_staging PGPASSWORD=disposable_role_pw \
  psql -X --single-transaction -q -v ON_ERROR_STOP=1 -v database_name="$template" \
  -v app_role=rust_app -v migrator_role=verdaxis_migrator_staging -v backup_role=rust_backup \
  -f deploy/postgres/converge_runtime_object_acls.sql >/dev/null
unset DATABASE_URL MIGRATOR_DATABASE_URL
admin="postgresql://postgres:disposable_admin_pw@127.0.0.1:$port"
export RUST_TEMPLATE_ADMIN_DATABASE_URL="$admin/postgres"
export RUST_ABILITIES_ADMIN_DATABASE_URL="$admin/$template"
export RUST_ABILITIES_DATABASE_URL="postgresql://rust_app:disposable_role_pw@127.0.0.1:$port/$template"
export RUST_ABILITIES_TEMPLATE_DATABASE="$template"
# CARGO_RUNNER carries the caller's shared target-directory lock.
# shellcheck disable=SC2086
${CARGO_RUNNER:-} cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read \
  --features contract-tests --test abilities_postgres --test abilities_http -- --include-ignored --test-threads=1

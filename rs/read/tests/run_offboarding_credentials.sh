#!/usr/bin/env bash
# Account credential cutoff acceptance on a disposable PostgreSQL database.
#
# Starts one disposable container on a Docker-assigned loopback port, migrates
# one database to the Alembic head, applies the runtime ACL bundle, and runs the
# ignored cases of `offboarding_credentials_postgres` with the runtime role and
# the schema owner. The stream and notice cases start the unchanged Python
# application on a loopback port. The container is removed on exit. Never point
# this at a live host.
#
# Optional environment:
#   DOCKER_RUN_EXTRA  extra `docker run` flags (for example a log driver)
#   CARGO_RUNNER      command words placed before every cargo invocation
#   RUST_TEST_PYTHON  interpreter with the application dependencies (default: python3)
set -euo pipefail
export SQLX_OFFLINE=true
RELEASE_SHA=$(git rev-parse HEAD)
export RELEASE_SHA
image='postgis/postgis:17-3.6-alpine@sha256:49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272'
python="${RUST_TEST_PYTHON:-python3}"
export RUST_TEST_PYTHON="$python"

container=''
log=''
cleanup() {
  # Keep the real exit status: the cleanup commands must not replace it.
  local status=$?
  if [ -n "$container" ]; then docker rm -f -v "$container" >/dev/null 2>&1 || true; fi
  if [ -n "$log" ]; then rm -f "$log"; fi
  exit "$status"
}
trap cleanup EXIT

# shellcheck disable=SC2086
container=$(docker run --platform linux/amd64 -d ${DOCKER_RUN_EXTRA:-} \
  -e POSTGRES_PASSWORD=disposable_admin_pw -p 127.0.0.1::5432 "$image")
ready=0
for _ in $(seq 1 120); do
  if docker exec "$container" pg_isready -h 127.0.0.1 -U postgres >/dev/null 2>&1; then ready=1; break; fi
  sleep 1
done
[ "$ready" = 1 ] || { echo "database did not become ready" >&2; exit 2; }
port=$(docker port "$container" 5432/tcp | head -1)
port=${port##*:}

docker exec -i "$container" psql -U postgres -v ON_ERROR_STOP=1 <<'SQL'
CREATE ROLE verdaxis_app_staging LOGIN NOINHERIT NOSUPERUSER PASSWORD 'disposable_app_pw';
CREATE ROLE verdaxis_migrator_staging LOGIN NOINHERIT NOSUPERUSER PASSWORD 'disposable_migrator_pw';
CREATE ROLE verdaxis_backup_staging LOGIN NOINHERIT NOSUPERUSER PASSWORD 'disposable_backup_pw';
CREATE DATABASE verdaxis_offboarding_credentials_test OWNER verdaxis_migrator_staging;
\connect verdaxis_offboarding_credentials_test
CREATE EXTENSION postgis;
ALTER SCHEMA public OWNER TO verdaxis_migrator_staging;
REVOKE ALL ON SCHEMA public FROM PUBLIC;
GRANT USAGE ON SCHEMA public TO verdaxis_app_staging,verdaxis_backup_staging;
SQL

database=verdaxis_offboarding_credentials_test
export ENVIRONMENT=test JWT_SECRET=test-secret-key-for-testing-minimum-32-chars
export DATABASE_URL="postgresql+asyncpg://verdaxis_migrator_staging:disposable_migrator_pw@127.0.0.1:$port/$database"
export MIGRATOR_DATABASE_URL="$DATABASE_URL"
"$python" -m alembic upgrade head
printf 'GATE=migrate EXIT=0\n'

export PGHOST=127.0.0.1 PGPORT="$port" PGDATABASE="$database"
export PGUSER=verdaxis_migrator_staging PGPASSWORD=disposable_migrator_pw
psql -X --single-transaction -q -v ON_ERROR_STOP=1 \
  -v database_name="$database" -v app_role=verdaxis_app_staging \
  -v migrator_role=verdaxis_migrator_staging -v backup_role=verdaxis_backup_staging \
  -f deploy/postgres/converge_runtime_object_acls.sql >/dev/null

  # The M runtime only serves an adopted database: record the exact
  # adoption row (the schema stays at the Alembic head).
  RUST_RUNNER_ENVIRONMENT=staging RUST_RUNNER_MIGRATOR_DATABASE_URL="postgresql://verdaxis_migrator_staging:disposable_migrator_pw@127.0.0.1:$port/$database" \
  cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read \
    --features contract-tests --test phase10_migrate_postgres \
    runner_database_adopts_to_the_baseline -- --ignored >/dev/null
printf 'GATE=acl EXIT=0\n'

export OFFBOARDING_TEST_DATABASE_URL="postgres://verdaxis_app_staging:disposable_app_pw@127.0.0.1:$port/$database"
export OFFBOARDING_TEST_OWNER_URL="postgres://verdaxis_migrator_staging:disposable_migrator_pw@127.0.0.1:$port/$database"
unset DATABASE_URL MIGRATOR_DATABASE_URL PGPASSWORD PGHOST PGPORT PGDATABASE PGUSER

log=$(mktemp "${TMPDIR:-/tmp}/offboarding-credentials.XXXXXX")
set +e
# shellcheck disable=SC2086
${CARGO_RUNNER:-} cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read \
  --features contract-tests --test offboarding_credentials_postgres \
  -- --include-ignored --test-threads=1 2>&1 | tee "$log"
cargo_status=${PIPESTATUS[0]}
set -e
printf 'GATE=offboarding_credentials_postgres EXIT=%s\n' "$cargo_status"
if (( cargo_status != 0 )); then exit "$cargo_status"; fi
# A green run that executed no case is a false pass; every named case must have run.
for case in cutoff_is_taken_after_user_lock cutoffs_never_move_back \
  all_refresh_families_and_credentials_are_invalidated \
  old_equal_malformed_and_future_iat_fail_closed \
  credential_cutoff_requires_the_held_user_lock \
  credential_cutoff_refuses_without_the_caller_lock \
  detached_streams_close_after_membership_recheck \
  detachment_suppresses_future_org_notices withdrawn_order_has_no_future_expiry_notice; do
  grep -q "test $case ... ok" "$log" || { echo "case did not run: $case" >&2; exit 1; }
done

#!/usr/bin/env bash
# Offboarded-account admission on a disposable PostgreSQL database.
#
# Starts one disposable container on a Docker-assigned loopback port, migrates
# one database from the SQL baseline, applies the runtime ACL bundle and runs the
# ignored cases of `offboarding_admission_postgres` as the runtime role. The
# container is removed on exit. Never point this at a live host.
#
# Optional environment:
#   DOCKER_RUN_EXTRA  extra `docker run` flags (for example a log driver)
#   CARGO_RUNNER      command words placed before every cargo invocation
set -euo pipefail
export SQLX_OFFLINE=true
RELEASE_SHA=$(git rev-parse HEAD)
export RELEASE_SHA
image='postgis/postgis:17-3.6-alpine@sha256:49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272'

run() {
  local name=$1; shift
  printf 'GATE=%s SHA=%s UTC=%s\n' "$name" "$RELEASE_SHA" "$(date -u +%FT%TZ)"
  local rc=0
  "$@" || rc=$?
  printf 'GATE=%s EXIT=%s\n' "$name" "$rc"
  return "$rc"
}

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
CREATE DATABASE verdaxis_offboarding_test OWNER verdaxis_migrator_staging;
\connect verdaxis_offboarding_test
CREATE EXTENSION postgis;
ALTER SCHEMA public OWNER TO verdaxis_migrator_staging;
REVOKE ALL ON SCHEMA public FROM PUBLIC;
GRANT USAGE ON SCHEMA public TO verdaxis_app_staging,verdaxis_backup_staging;
SQL

# shellcheck disable=SC2086
RUST_RUNNER_ENVIRONMENT=staging RUST_RUNNER_MIGRATOR_DATABASE_URL="postgresql://verdaxis_migrator_staging:disposable_migrator_pw@127.0.0.1:$port/verdaxis_offboarding_test" \
  ${CARGO_RUNNER:-} cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read \
  --features contract-tests --test phase10_migrate_postgres \
  runner_database_adopts_to_the_baseline -- --ignored

export PGHOST=127.0.0.1 PGPORT="$port" PGDATABASE=verdaxis_offboarding_test
export PGUSER=verdaxis_migrator_staging PGPASSWORD=disposable_migrator_pw
run acl psql -X --single-transaction -v ON_ERROR_STOP=1 \
  -v database_name=verdaxis_offboarding_test -v app_role=verdaxis_app_staging \
  -v migrator_role=verdaxis_migrator_staging -v backup_role=verdaxis_backup_staging \
  -f deploy/postgres/converge_runtime_object_acls.sql

export RUST_TEST_DATABASE_URL="postgres://verdaxis_app_staging:disposable_app_pw@127.0.0.1:$port/verdaxis_offboarding_test"
export RUST_TEST_MIGRATOR_DATABASE_URL="postgres://verdaxis_migrator_staging:disposable_migrator_pw@127.0.0.1:$port/verdaxis_offboarding_test"
unset DATABASE_URL MIGRATOR_DATABASE_URL PGPASSWORD PGHOST PGPORT PGDATABASE PGUSER

log=$(mktemp "${TMPDIR:-/tmp}/offboarding-admission.XXXXXX")
set +e
# shellcheck disable=SC2086
${CARGO_RUNNER:-} cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read \
  --features contract-tests --test offboarding_admission_postgres \
  -- --include-ignored --test-threads=1 2>&1 | tee "$log"
cargo_status=${PIPESTATUS[0]}
set -e
printf 'GATE=offboarding_admission_postgres EXIT=%s\n' "$cargo_status"
if (( cargo_status != 0 )); then exit "$cargo_status"; fi
# A green run that executed no case is a false pass; every named case must have run.
for case in marked_login_refresh_and_issuance_denied marked_account_denied_before_receipt_replay \
  marked_user_not_in_review_queue marked_account_approval_refused marked_token_shapes_fail_closed \
  offboarding_commit_wins_over_waiting_mutation marker_refuses_where_every_other_rule_admits; do
  grep -q "test $case ... ok" "$log" || { echo "case did not run: $case" >&2; exit 1; }
done

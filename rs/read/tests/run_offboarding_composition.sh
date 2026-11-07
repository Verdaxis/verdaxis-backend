#!/usr/bin/env bash
# Composed account offboarding acceptance on disposable PostgreSQL databases.
#
# Starts one disposable container on a Docker-assigned loopback port, migrates
# one template database from the SQL baseline with the runtime ACL bundle, and
# runs the ignored cases of `offboarding_composition_postgres`; each case
# clones the template. Every named case must run. The container is removed on
# exit. Never point this at a live host.
#
# Optional environment:
#   DOCKER_RUN_EXTRA  extra `docker run` flags (for example a log driver)
#   CARGO_RUNNER      command words placed before every cargo invocation
set -euo pipefail
export SQLX_OFFLINE=true
RELEASE_SHA=$(git rev-parse HEAD)
export RELEASE_SHA
image='postgis/postgis:17-3.6-alpine@sha256:49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272'
template=verdaxis_offboarding_composition_test
cases=(
  commit_contains_all_account_market_audit_and_outbox_changes
  market_failure_rolls_back_credentials_and_membership
  concurrent_offboard_and_mutation_has_one_ordered_winner
  unknown_outcome_can_be_resolved_by_admin_user_read
  commit_time_termination_is_unknown_until_the_user_is_read
  termination_before_commit_keeps_its_classification
)

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
CREATE ROLE verdaxis_migrator_staging LOGIN NOINHERIT NOSUPERUSER PASSWORD 'disposable_role_pw';
CREATE ROLE rust_app LOGIN NOINHERIT NOSUPERUSER PASSWORD 'disposable_role_pw';
CREATE ROLE rust_backup NOLOGIN;
SQL
docker exec "$container" psql -U postgres -v ON_ERROR_STOP=1 \
  -c "CREATE DATABASE $template OWNER verdaxis_migrator_staging" >/dev/null
docker exec "$container" psql -U postgres -d "$template" -v ON_ERROR_STOP=1 \
  -c 'CREATE EXTENSION postgis; ALTER SCHEMA public OWNER TO verdaxis_migrator_staging; GRANT USAGE ON SCHEMA public TO rust_app' >/dev/null

RUST_RUNNER_ENVIRONMENT=staging RUST_RUNNER_MIGRATOR_DATABASE_URL="postgresql://verdaxis_migrator_staging:disposable_role_pw@127.0.0.1:$port/$template" \
  cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read \
  --features contract-tests --test phase10_migrate_postgres \
  runner_database_adopts_to_the_baseline -- --ignored
PGHOST=127.0.0.1 PGPORT="$port" PGDATABASE="$template" \
  PGUSER=verdaxis_migrator_staging PGPASSWORD=disposable_role_pw \
  psql -X --single-transaction -q -v ON_ERROR_STOP=1 \
  -v database_name="$template" -v app_role=rust_app \
  -v migrator_role=verdaxis_migrator_staging -v backup_role=rust_backup \
  -f deploy/postgres/converge_runtime_object_acls.sql >/dev/null

printf 'GATE=migrate-%s EXIT=0\n' "$template"
unset DATABASE_URL MIGRATOR_DATABASE_URL

admin="postgresql://postgres:disposable_admin_pw@127.0.0.1:$port"
export RUST_TEMPLATE_ADMIN_DATABASE_URL="$admin/postgres"
export RUST_OFFBOARDING_COMPOSITION_ADMIN_DATABASE_URL="$admin/$template"
export RUST_OFFBOARDING_COMPOSITION_DATABASE_URL="postgresql://rust_app:disposable_role_pw@127.0.0.1:$port/$template"
export RUST_OFFBOARDING_COMPOSITION_TEMPLATE_DATABASE="$template"

log=$(mktemp "${TMPDIR:-/tmp}/offboarding-composition.XXXXXX")
set +e
# shellcheck disable=SC2086
${CARGO_RUNNER:-} cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read \
  --features contract-tests --test offboarding_composition_postgres \
  -- --include-ignored --test-threads=1 --show-output 2>&1 | tee "$log"
cargo_status=${PIPESTATUS[0]}
set -e
printf 'GATE=offboarding_composition_postgres EXIT=%s\n' "$cargo_status"
if (( cargo_status != 0 )); then exit "$cargo_status"; fi
# A green run that executed fewer cases is a false pass.
grep -Eq "^test result: ok\. ${#cases[@]} passed; 0 failed; 0 ignored" "$log" \
  || { echo "expected ${#cases[@]} executed cases" >&2; exit 1; }
for case in "${cases[@]}"; do
  grep -q "test $case ... ok" "$log" || { echo "case did not run: $case" >&2; exit 1; }
done

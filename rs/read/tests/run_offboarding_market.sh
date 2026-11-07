#!/usr/bin/env bash
# Market invalidation, responsibility reassignment, matching, conservation and
# lock acceptance on disposable PostgreSQL databases.
#
# Starts one disposable container on a Docker-assigned loopback port. Four
# databases are migrated from the SQL baseline with the runtime ACL bundle and
# serve as templates that the tests clone per case; one more database holds the
# hand-made schemas of the invalidation and lock targets. Each target runs its
# ignored cases, and each must run exactly its expected number of cases. The
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

# Each target and the number of cases it must run.
targets=(
  "offboarding_market_postgres 15"
  "offboarding_reassignment_postgres 10"
  "phase4_market_invalidation 26"
  "market_write_pg_matching 22"
  "market_write_pg_conservation 8"
  "market_write_pg_locks 22"
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

templates=(verdaxis_offboarding_market_test verdaxis_offboarding_reassignment_test verdaxis_matching_test verdaxis_effects_test)
# The runtime and migrator roles use disposable credentials in this container.
docker exec -i "$container" psql -U postgres -v ON_ERROR_STOP=1 <<'SQL'
CREATE ROLE verdaxis_migrator_staging LOGIN NOINHERIT NOSUPERUSER PASSWORD 'disposable_role_pw';
CREATE ROLE rust_app LOGIN NOINHERIT NOSUPERUSER PASSWORD 'disposable_role_pw';
CREATE ROLE rust_backup NOLOGIN;
SQL
for database in "${templates[@]}" verdaxis_rejection_test; do
  docker exec "$container" psql -U postgres -v ON_ERROR_STOP=1 \
    -c "CREATE DATABASE $database OWNER verdaxis_migrator_staging" >/dev/null
  docker exec "$container" psql -U postgres -d "$database" -v ON_ERROR_STOP=1 \
    -c 'CREATE EXTENSION postgis; ALTER SCHEMA public OWNER TO verdaxis_migrator_staging; GRANT USAGE ON SCHEMA public TO rust_app' >/dev/null
done

for database in "${templates[@]}"; do
  # shellcheck disable=SC2086
  RUST_RUNNER_ENVIRONMENT=staging RUST_RUNNER_MIGRATOR_DATABASE_URL="postgresql://verdaxis_migrator_staging:disposable_role_pw@127.0.0.1:$port/$database" \
    ${CARGO_RUNNER:-} cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read \
    --features contract-tests --test phase10_migrate_postgres \
    runner_database_adopts_to_the_baseline -- --ignored
  PGHOST=127.0.0.1 PGPORT="$port" PGDATABASE="$database" \
    PGUSER=verdaxis_migrator_staging PGPASSWORD=disposable_role_pw \
    psql -X --single-transaction -q -v ON_ERROR_STOP=1 \
    -v database_name="$database" -v app_role=rust_app \
    -v migrator_role=verdaxis_migrator_staging -v backup_role=rust_backup \
    -f deploy/postgres/converge_runtime_object_acls.sql >/dev/null

  printf 'GATE=migrate-%s EXIT=0\n' "$database"
done
unset DATABASE_URL MIGRATOR_DATABASE_URL

admin="postgresql://postgres:disposable_admin_pw@127.0.0.1:$port"
app="postgresql://rust_app:disposable_role_pw@127.0.0.1:$port"
export RUST_TEMPLATE_ADMIN_DATABASE_URL="$admin/postgres"
export RUST_OFFBOARDING_MARKET_ADMIN_DATABASE_URL="$admin/verdaxis_offboarding_market_test"
export RUST_OFFBOARDING_MARKET_DATABASE_URL="$app/verdaxis_offboarding_market_test"
export RUST_OFFBOARDING_MARKET_TEMPLATE_DATABASE=verdaxis_offboarding_market_test
export RUST_OFFBOARDING_REASSIGNMENT_ADMIN_DATABASE_URL="$admin/verdaxis_offboarding_reassignment_test"
export RUST_OFFBOARDING_REASSIGNMENT_DATABASE_URL="$app/verdaxis_offboarding_reassignment_test"
export RUST_OFFBOARDING_REASSIGNMENT_TEMPLATE_DATABASE=verdaxis_offboarding_reassignment_test
export RUST_MATCHING_ADMIN_DATABASE_URL="$admin/verdaxis_matching_test"
export RUST_MATCHING_DATABASE_URL="$app/verdaxis_matching_test"
export RUST_MATCHING_TEMPLATE_DATABASE=verdaxis_matching_test
export RUST_CONSERVATION_ADMIN_DATABASE_URL="$admin/verdaxis_effects_test"
export RUST_CONSERVATION_DATABASE_URL="$app/verdaxis_effects_test"
export RUST_CONSERVATION_TEMPLATE_DATABASE=verdaxis_effects_test
# The invalidation and lock targets create their own schemas here.
export RUST_TEST_MIGRATOR_DATABASE_URL="postgresql://verdaxis_migrator_staging:disposable_role_pw@127.0.0.1:$port/verdaxis_rejection_test"
export RUST_TEST_DATABASE_URL="$app/verdaxis_rejection_test"

log=$(mktemp "${TMPDIR:-/tmp}/offboarding-market.XXXXXX")
for entry in "${targets[@]}"; do
  read -r target expected <<<"$entry"
  started=$(date -u +%s)
  set +e
  # shellcheck disable=SC2086
  ${CARGO_RUNNER:-} cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read \
    --features contract-tests --test "$target" \
    -- --include-ignored --test-threads=1 2>&1 | tee "$log"
  cargo_status=${PIPESTATUS[0]}
  set -e
  printf 'GATE=%s EXIT=%s SECONDS=%s\n' "$target" "$cargo_status" "$(( $(date -u +%s) - started ))"
  if (( cargo_status != 0 )); then exit "$cargo_status"; fi
  # A green run that executed fewer cases is a false pass.
  grep -Eq "^test result: ok\. ${expected} passed; 0 failed; 0 ignored" "$log" \
    || { echo "$target: expected ${expected} executed cases" >&2; exit 1; }
done

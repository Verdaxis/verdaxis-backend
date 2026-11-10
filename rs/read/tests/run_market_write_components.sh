#!/usr/bin/env bash
# Market-write component cases on disposable PostgreSQL databases: cancel,
# commission, order contexts, write effects, match completion, post-only,
# stock movements and matchmaking suggestions.
#
# Starts one disposable container on a Docker-assigned loopback port. Five
# databases are migrated from the SQL baseline with the runtime ACL bundle and
# serve as templates that the tests clone per case; one more database holds the
# hand-made schemas of the stock and suggestion targets. Each target runs its
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

# Each test module of group_market_write and the number of cases it must run.
targets=(
  "market_write_pg_cancel 21"
  "market_write_pg_commission 17"
  "market_write_pg_contexts 17"
  "market_write_pg_effects 15"
  "market_write_pg_match_completion 20"
  "market_write_pg_post_only 5"
  "market_write_pg_stock 7"
  "market_write_pg_suggestions 8"
)

# Each migrated template and the variable prefix that the tests read for it.
# Commission and effects share the effects template; match completion and
# post-only share the matching template; contexts reads the support template.
templates=(
  "verdaxis_cancel_test RUST_CANCEL"
  "verdaxis_effects_test RUST_EFFECTS"
  "verdaxis_support_test RUST_SUPPORT"
  "verdaxis_matching_test RUST_MATCHING"
  "verdaxis_suggestions_test RUST_SUGGESTIONS"
)
# The stock and suggestion targets create their own schemas here.
scratch_database=verdaxis_market_scratch_test

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
# The stock expiry case counts statements with pg_stat_statements, which the
# server must preload.
container=$(docker run --platform linux/amd64 -d ${DOCKER_RUN_EXTRA:-} \
  -e POSTGRES_PASSWORD=disposable_admin_pw -p 127.0.0.1::5432 "$image" \
  postgres -c shared_preload_libraries=pg_stat_statements)
ready=0
for _ in $(seq 1 120); do
  if docker exec "$container" pg_isready -h 127.0.0.1 -U postgres >/dev/null 2>&1; then ready=1; break; fi
  sleep 1
done
[ "$ready" = 1 ] || { echo "database did not become ready" >&2; exit 2; }
port=$(docker port "$container" 5432/tcp | head -1)
port=${port##*:}

# The runtime and migrator roles use disposable credentials in this container.
docker exec -i "$container" psql -U postgres -v ON_ERROR_STOP=1 <<'SQL'
CREATE ROLE verdaxis_migrator_staging LOGIN NOINHERIT NOSUPERUSER PASSWORD 'disposable_role_pw';
CREATE ROLE rust_app LOGIN NOINHERIT NOSUPERUSER PASSWORD 'disposable_role_pw';
CREATE ROLE rust_backup NOLOGIN;
SQL
template_databases=()
for entry in "${templates[@]}"; do
  read -r database _ <<<"$entry"
  template_databases+=("$database")
done
for database in "${template_databases[@]}" "$scratch_database"; do
  docker exec "$container" psql -U postgres -v ON_ERROR_STOP=1 \
    -c "CREATE DATABASE $database OWNER verdaxis_migrator_staging" >/dev/null
  docker exec "$container" psql -U postgres -d "$database" -v ON_ERROR_STOP=1 \
    -c 'CREATE EXTENSION postgis; ALTER SCHEMA public OWNER TO verdaxis_migrator_staging; GRANT USAGE ON SCHEMA public TO rust_app' >/dev/null
done

for database in "${template_databases[@]}"; do
  # shellcheck disable=SC2086
  RUST_RUNNER_ENVIRONMENT=staging RUST_RUNNER_MIGRATOR_DATABASE_URL="postgresql://verdaxis_migrator_staging:disposable_role_pw@127.0.0.1:$port/$database" \
    ${CARGO_RUNNER:-} cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read \
    --features contract-tests --test group_phase10 \
    phase10_migrate_postgres::runner_database_adopts_to_the_baseline -- --ignored
  PGHOST=127.0.0.1 PGPORT="$port" PGDATABASE="$database" \
    PGUSER=verdaxis_migrator_staging PGPASSWORD=disposable_role_pw \
    psql -X --single-transaction -q -v ON_ERROR_STOP=1 \
    -v database_name="$database" -v app_role=rust_app \
    -v migrator_role=verdaxis_migrator_staging -v backup_role=rust_backup \
    -f deploy/postgres/converge_runtime_object_acls.sql >/dev/null

  printf 'GATE=migrate-%s EXIT=0\n' "$database"
done
unset DATABASE_URL MIGRATOR_DATABASE_URL
# The stock target resets and reads the statement statistics as the migrator.
docker exec -i "$container" psql -U postgres -d "$scratch_database" -v ON_ERROR_STOP=1 <<'SQL' >/dev/null
CREATE EXTENSION pg_stat_statements;
GRANT EXECUTE ON FUNCTION pg_stat_statements_reset TO verdaxis_migrator_staging;
SQL

admin="postgresql://postgres:disposable_admin_pw@127.0.0.1:$port"
app="postgresql://rust_app:disposable_role_pw@127.0.0.1:$port"
export RUST_TEMPLATE_ADMIN_DATABASE_URL="$admin/postgres"
for entry in "${templates[@]}"; do
  read -r database prefix <<<"$entry"
  export "${prefix}_ADMIN_DATABASE_URL=$admin/$database"
  export "${prefix}_DATABASE_URL=$app/$database"
  export "${prefix}_TEMPLATE_DATABASE=$database"
done
export RUST_TEST_MIGRATOR_DATABASE_URL="postgresql://verdaxis_migrator_staging:disposable_role_pw@127.0.0.1:$port/$scratch_database"
export RUST_TEST_DATABASE_URL="$app/$scratch_database"

log=$(mktemp "${TMPDIR:-/tmp}/market-write-components.XXXXXX")
# Every target runs, so one run reports all failing targets; the runner fails
# at the end if any target failed.
failed=()
for entry in "${targets[@]}"; do
  read -r target expected <<<"$entry"
  started=$(date -u +%s)
  set +e
  # shellcheck disable=SC2086
  ${CARGO_RUNNER:-} cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read \
    --features contract-tests --test group_market_write "$target::" \
    -- --include-ignored --test-threads=1 2>&1 | tee "$log"
  cargo_status=${PIPESTATUS[0]}
  set -e
  printf 'GATE=%s EXIT=%s SECONDS=%s\n' "$target" "$cargo_status" "$(( $(date -u +%s) - started ))"
  if (( cargo_status != 0 )); then
    failed+=("$target")
  elif ! grep -Eq "^test result: ok\. ${expected} passed; 0 failed; 0 ignored" "$log"; then
    # A green run that executed fewer cases is a false pass.
    echo "$target: expected ${expected} executed cases" >&2
    failed+=("$target")
  fi
done
if (( ${#failed[@]} > 0 )); then
  echo "failed targets: ${failed[*]}" >&2
  exit 1
fi

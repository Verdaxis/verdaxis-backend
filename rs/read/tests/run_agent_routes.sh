#!/usr/bin/env bash
# Exercise the agent HTTP routes with deployed app ACLs in a disposable database.
# Never point this runner at staging or production.
set -euo pipefail
image='postgis/postgis:17-3.6-alpine@sha256:49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272'
export SQLX_OFFLINE=true
RELEASE_SHA=$(git rev-parse HEAD)
export RELEASE_SHA
export ENVIRONMENT=test
export JWT_SECRET=test-secret-key-for-testing-minimum-32-chars

database=verdaxis_agent_routes_test
app_role=verdaxis_app_test
migrator_role=verdaxis_migrator_test
backup_role=verdaxis_backup_test
password=disposable_pw
container=''
run_log=''
cleanup() {
  local status=$?
  if [[ -n "$container" ]]; then
    if (( status != 0 )); then docker logs "$container" || true; fi
    docker rm -f -v "$container" >/dev/null || true
  fi
  if [[ -n "$run_log" ]]; then rm -f "$run_log"; fi
  exit "$status"
}
trap cleanup EXIT
container=$(docker run --platform linux/amd64 -d --rm -e POSTGRES_PASSWORD="$password" \
  -p 127.0.0.1::5432 "$image")
ready=0
for _ in {1..120}; do
  if docker exec "$container" pg_isready -h 127.0.0.1 -U postgres >/dev/null 2>&1; then ready=1; break; fi
  sleep 1
done
[[ "$ready" == 1 ]] || { echo 'database did not become ready' >&2; exit 2; }
port=$(docker port "$container" 5432/tcp)
port=${port##*:}
database_url() {
  printf 'postgresql://%s:%s@127.0.0.1:%s/%s' "$1" "$password" "$port" "$database"
}
docker exec "$container" psql -X -q -U postgres -v ON_ERROR_STOP=1 -c "CREATE DATABASE $database"
docker exec "$container" psql -X -q -U postgres -d "$database" -v ON_ERROR_STOP=1 -c 'CREATE EXTENSION IF NOT EXISTS postgis'
docker exec "$container" mkdir -p /tmp/role-policy
docker cp deploy/postgres/. "$container":/tmp/role-policy/ >/dev/null
apply_role_policy() {
  docker exec "$container" psql -X -q -U postgres -d "$database" -v ON_ERROR_STOP=1 \
    -v database_name="$database" -v app_role="$app_role" \
    -v migrator_role="$migrator_role" -v backup_role="$backup_role" \
    -f /tmp/role-policy/bootstrap_roles.sql >/dev/null
}
apply_role_policy
docker exec -i "$container" psql -X -q -U postgres -v ON_ERROR_STOP=1 --single-transaction <<SQL
ALTER ROLE $app_role PASSWORD '$password';
ALTER ROLE $migrator_role PASSWORD '$password';
ALTER ROLE $backup_role PASSWORD '$password';
SQL
RUST_RUNNER_ENVIRONMENT=staging RUST_RUNNER_MIGRATOR_DATABASE_URL="$(database_url "$migrator_role")" \
  cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read \
  --features contract-tests --test group_phase10 \
  phase10_migrate_postgres::runner_database_applies_the_baseline -- --ignored
apply_role_policy
docker exec "$container" psql -X -q -U postgres -d "$database" -v ON_ERROR_STOP=1 \
  -v database_name="$database" -v app_role="$app_role" \
  -v migrator_role="$migrator_role" -v backup_role="$backup_role" \
  -f /tmp/role-policy/validate_roles.sql >/dev/null

run_log=$(mktemp "${TMPDIR:-/tmp}/rust-agent-routes.XXXXXX")
set +e
AGENT_ROUTES_APP_DATABASE_URL="$(database_url "$app_role")" \
AGENT_ROUTES_ADMIN_DATABASE_URL="$(database_url postgres)" \
  cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read \
  --features contract-tests --test group_crud agent_routes_postgres:: \
  -- --ignored --test-threads=1 2>&1 | tee "$run_log"
status=${PIPESTATUS[0]}
set -e
if (( status != 0 )); then exit "$status"; fi
passed=$(awk '/^test result: /{sum += $4} END{print sum + 0}' "$run_log")
if (( passed != 13 )); then
  echo "agent routes: expected 13 passed tests, got $passed" >&2
  exit 1
fi
if grep -E '^test result: ' "$run_log" | grep -Ev ' 0 failed; 0 ignored;' >/dev/null; then
  echo 'agent routes: a test failed or was ignored' >&2
  exit 1
fi
printf 'agent routes: %s passed\n' "$passed"

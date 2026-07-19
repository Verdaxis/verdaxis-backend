#!/usr/bin/env bash
# Run the Product Analytics PostgreSQL correctness suite against a disposable
# postgis/postgis:17-3.6-alpine container (PostgreSQL 17 with the deployed
# PostGIS 3.6 line; the runner verifies the PostgreSQL/PostGIS series).
#
# Usage:
#   scripts/run_product_analytics_postgres_tests.sh [pytest paths...]
#
# Behaviour:
# - If PRODUCT_ANALYTICS_TEST_DATABASE_URL is already exported, no container
#   is started; the URL is validated (database name must end in
#   `_analytics_test`) and pytest runs against it directly.
# - Otherwise a uniquely named postgis container starts on a Docker-assigned
#   loopback port, `verdaxis_analytics_test` is created, pg_isready is
#   awaited, and the async URL is exported for the suite. The container is
#   removed on EXIT/INT/TERM.
#
# The invoking user needs Docker access and permission to read the backend
# .env (app settings import). Defaults target the repository venv.

set -euo pipefail

BACKEND_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PYTEST_BIN="${PYTEST_BIN:-$BACKEND_ROOT/venv/bin/pytest}"
IMAGE="postgis/postgis:17-3.6-alpine@sha256:49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272"
DB_NAME="verdaxis_analytics_test"
DB_PASSWORD="analytics-test"
APP_ROLE="verdaxis_app_test"
MIGRATOR_ROLE="verdaxis_migrator_test"
BACKUP_ROLE="verdaxis_backup_test"

# This harness is always a test environment. Callers may override these
# values, but a local run must not inherit unsafe production defaults.
export ENVIRONMENT="${ENVIRONMENT:-test}"
export RELEASE_SHA="${RELEASE_SHA:-test}"
export JWT_SECRET="${JWT_SECRET:-test-secret-key-that-is-at-least-32-characters-long}"

PYTEST_PATHS=("$@")
if [ ${#PYTEST_PATHS[@]} -eq 0 ]; then
  PYTEST_PATHS=("tests/postgres")
fi

validate_url() {
  local url="$1"
  local db_path="${url##*/}"
  db_path="${db_path%%\?*}"
  if [[ "$db_path" != *_analytics_test ]]; then
    echo "refusing externally supplied URL: database name '$db_path' must end with _analytics_test" >&2
    exit 2
  fi
}

apply_role_policy() {
  docker exec -i "$CONTAINER" psql -U postgres -d "$DB_NAME" \
    -v database_name="$DB_NAME" \
    -v app_role="$APP_ROLE" \
    -v migrator_role="$MIGRATOR_ROLE" \
    -v backup_role="$BACKUP_ROLE" \
    < "$BACKEND_ROOT/deploy/postgres/bootstrap_roles.sql"
}

if [ -n "${PRODUCT_ANALYTICS_TEST_DATABASE_URL:-}" ]; then
  validate_url "$PRODUCT_ANALYTICS_TEST_DATABASE_URL"
  echo "Using externally supplied PRODUCT_ANALYTICS_TEST_DATABASE_URL"
  export DATABASE_URL="${DATABASE_URL:-$PRODUCT_ANALYTICS_TEST_DATABASE_URL}"
else
  command -v docker >/dev/null || { echo "docker is required" >&2; exit 2; }
  CONTAINER="verdaxis-analytics-test-$$-$(date +%s)"

  cleanup() {
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  }
  trap cleanup EXIT INT TERM

  echo "Starting $IMAGE as $CONTAINER"
  docker run -d --name "$CONTAINER" \
    --label "com.docker.compose.project=verdaxis-postgres-test-$$" \
    -e POSTGRES_PASSWORD="$DB_PASSWORD" \
    -e POSTGRES_DB="$DB_NAME" \
    -p 127.0.0.1::5432 \
    "$IMAGE" >/dev/null

  PORT="$(docker port "$CONTAINER" 5432/tcp | head -1 | awk -F: '{print $NF}')"
  if [ -z "$PORT" ]; then
    echo "failed to determine mapped port" >&2
    exit 1
  fi

  echo "Waiting for PostgreSQL on 127.0.0.1:$PORT"
  for _ in $(seq 1 60); do
    if docker exec "$CONTAINER" pg_isready -U postgres -d "$DB_NAME" >/dev/null 2>&1 \
      && pg_isready -h 127.0.0.1 -p "$PORT" -U postgres -d "$DB_NAME" >/dev/null 2>&1; then
      READY=1
      break
    fi
    sleep 1
  done
  if [ -z "${READY:-}" ]; then
    echo "PostgreSQL did not become ready" >&2
    exit 1
  fi

  VERSION_ROW="$(docker exec "$CONTAINER" psql -U postgres -d "$DB_NAME" -qAtF '|' -c "CREATE EXTENSION IF NOT EXISTS postgis; SELECT current_setting('server_version_num'), PostGIS_Lib_Version();")"
  IFS='|' read -r POSTGRES_VERSION_NUM POSTGIS_VERSION <<<"$VERSION_ROW"
  if [[ "$POSTGRES_VERSION_NUM" -lt 170000 || "$POSTGRES_VERSION_NUM" -ge 180000 || "$POSTGIS_VERSION" != 3.6.* ]]; then
    echo "expected PostgreSQL 17/PostGIS 3.6; version attestation failed" >&2
    exit 1
  fi

  apply_role_policy
  docker exec "$CONTAINER" psql -U postgres -d "$DB_NAME" -v ON_ERROR_STOP=1 \
    -c "ALTER ROLE $APP_ROLE PASSWORD '$DB_PASSWORD'; ALTER ROLE $MIGRATOR_ROLE PASSWORD '$DB_PASSWORD'; ALTER ROLE $BACKUP_ROLE PASSWORD '$DB_PASSWORD';" >/dev/null

  export PRODUCT_ANALYTICS_TEST_DATABASE_URL="postgresql+asyncpg://${MIGRATOR_ROLE}:${DB_PASSWORD}@127.0.0.1:${PORT}/${DB_NAME}"
  export MIGRATOR_DATABASE_URL="postgresql+asyncpg://${MIGRATOR_ROLE}:${DB_PASSWORD}@127.0.0.1:${PORT}/${DB_NAME}"
  export DATABASE_URL="postgresql+asyncpg://${APP_ROLE}:${DB_PASSWORD}@127.0.0.1:${PORT}/${DB_NAME}"
  export BACKUP_DATABASE_URL="postgresql+asyncpg://${BACKUP_ROLE}:${DB_PASSWORD}@127.0.0.1:${PORT}/${DB_NAME}"
  export POSTGRES_ADMIN_TEST_DATABASE_URL="postgresql+asyncpg://postgres:${DB_PASSWORD}@127.0.0.1:${PORT}/${DB_NAME}"
fi

export RUNTIME_TEST_DATABASE_NAME="${RUNTIME_TEST_DATABASE_NAME:-$DB_NAME}"
export RUNTIME_TEST_APP_ROLE="${RUNTIME_TEST_APP_ROLE:-$APP_ROLE}"
export RUNTIME_TEST_MIGRATOR_ROLE="${RUNTIME_TEST_MIGRATOR_ROLE:-$MIGRATOR_ROLE}"
export RUNTIME_TEST_BACKUP_ROLE="${RUNTIME_TEST_BACKUP_ROLE:-$BACKUP_ROLE}"

cd "$BACKEND_ROOT"
./scripts/verify_migrations.sh
if [[ -n "${CONTAINER:-}" ]]; then
  apply_role_policy
  docker exec -i "$CONTAINER" psql -U postgres -d "$DB_NAME" \
    -v database_name="$DB_NAME" \
    -v app_role="$APP_ROLE" \
    -v migrator_role="$MIGRATOR_ROLE" \
    -v backup_role="$BACKUP_ROLE" \
    < "$BACKEND_ROOT/deploy/postgres/validate_roles.sql"
fi
PYTHONDONTWRITEBYTECODE=1 "$PYTEST_BIN" -p no:cacheprovider "${PYTEST_PATHS[@]}" -q

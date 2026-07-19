#!/usr/bin/env bash
# Run the Product Analytics PostgreSQL correctness suite against a disposable
# postgis/postgis:15-3.3 container (matching the deployed image).
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
IMAGE="postgis/postgis:15-3.3"
DB_NAME="verdaxis_analytics_test"
DB_PASSWORD="analytics-test"

# This harness is always a test environment. Callers may override these
# values, but a local run must not inherit unsafe production defaults.
export ENVIRONMENT="${ENVIRONMENT:-test}"
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

if [ -n "${PRODUCT_ANALYTICS_TEST_DATABASE_URL:-}" ]; then
  validate_url "$PRODUCT_ANALYTICS_TEST_DATABASE_URL"
  echo "Using externally supplied PRODUCT_ANALYTICS_TEST_DATABASE_URL"
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

  export PRODUCT_ANALYTICS_TEST_DATABASE_URL="postgresql+asyncpg://postgres:${DB_PASSWORD}@127.0.0.1:${PORT}/${DB_NAME}"
fi

cd "$BACKEND_ROOT"
DATABASE_URL="${PRODUCT_ANALYTICS_TEST_DATABASE_URL}" \
  ./scripts/verify_migrations.sh
PYTHONDONTWRITEBYTECODE=1 "$PYTEST_BIN" -p no:cacheprovider "${PYTEST_PATHS[@]}" -q

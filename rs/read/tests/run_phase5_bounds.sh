#!/usr/bin/env bash
# Phase 5 bound-matrix runner: the populated availability policy matrix and
# the producer result-limit matrix, which need their own disposable databases
# (the availability cases create a dedicated schema; the producer cases need
# the migrated schema and a writer role).
#
# Docker runs locally through Colima on the M1 test host. The container binds
# a Docker-assigned loopback port and is removed on exit. Never point this at
# staging or production.
set -euo pipefail
image='postgis/postgis:17-3.6-alpine@sha256:49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272'
test_home="${VERDAXIS_TEST_HOME:-/home/verdaxis-test}"

: "${RUST_TEST_PYTHON:?Set an absolute Python path with the project database dependencies}"
export RUST_TEST_PYTHON

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
alembic="$(dirname "$RUST_TEST_PYTHON")/alembic"

stamp() { echo "== $(date -u +%Y-%m-%dT%H:%M:%SZ) $*"; }

availability_container=''
producer_container=''
cleanup() {
  local status=$?
  if [[ -n "$availability_container" ]]; then
    docker rm -f -v "$availability_container" >/dev/null || true
  fi
  if [[ -n "$producer_container" ]]; then
    docker rm -f -v "$producer_container" >/dev/null || true
  fi
  exit "$status"
}
trap cleanup EXIT

echo "HOST=$(hostname -s) RUSTC=$(rustc -V 2>/dev/null)"
echo "TREE=$(git status --porcelain --untracked-files=no | wc -l | tr -d ' ') changed tracked file(s)"
export RELEASE_SHA=$(git rev-parse HEAD)
export SQLX_OFFLINE=true

# --- Availability: PostGIS, a dedicated database, and schema-creation rights
# for the schema the populated policy matrix builds for itself.
stamp availability-database
availability_container=$(docker run --platform linux/amd64 -d --rm -e POSTGRES_PASSWORD=disposable_admin_pw -p 127.0.0.1::5432 "$image")
for _ in {1..90}; do
  if docker exec "$availability_container" pg_isready -h 127.0.0.1 -U postgres >/dev/null 2>&1; then break; fi
  sleep 1
done
port=$(docker port "$availability_container" 5432/tcp)
port=${port##*:}
docker exec -i "$availability_container" psql -X -U postgres -v ON_ERROR_STOP=1 <<'SQL'
CREATE DATABASE availability_test;
\connect availability_test
CREATE EXTENSION postgis;
SQL

availability_log=$(mktemp "${test_home}/runs/phase5-availability.XXXXXX")
export AVAILABILITY_TEST_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/availability_test"
stamp availability-test
set +e
cargo test --manifest-path "$repo/rs/Cargo.toml" --locked -p verdaxis-read \
  --features contract-tests --test phase5_availability -- --ignored --test-threads=1 --nocapture 2>&1 | tee "$availability_log"
availability_status=${PIPESTATUS[0]}
set -e
echo "AVAILABILITY_TEST_EXIT=$availability_status"
grep -E '^test |^test result' "$availability_log" || true
if (( availability_status != 0 )); then
  rm -f "$availability_log"
  exit "$availability_status"
fi
if ! grep -Eq '^test result: ok\. [1-9][0-9]* passed' "$availability_log"; then
  echo "no populated availability case ran: refusing a green result" >&2
  rm -f "$availability_log"
  exit 1
fi
rm -f "$availability_log"
docker rm -f -v "$availability_container" >/dev/null
availability_container=''

# --- Producers: the migrated schema, a writer role and the app role.
stamp producer-database
producer_container=$(docker run --platform linux/amd64 -d --rm -e POSTGRES_PASSWORD=disposable_admin_pw -p 127.0.0.1::5432 "$image")
for _ in {1..90}; do
  if docker exec "$producer_container" pg_isready -h 127.0.0.1 -U postgres >/dev/null 2>&1; then break; fi
  sleep 1
done
port=$(docker port "$producer_container" 5432/tcp)
port=${port##*:}
docker exec -i "$producer_container" psql -U postgres -v ON_ERROR_STOP=1 <<'SQL'
CREATE ROLE phase5_bounds_writer LOGIN NOINHERIT NOSUPERUSER PASSWORD 'disposable_writer_pw';
CREATE ROLE phase5_bounds_app LOGIN NOINHERIT PASSWORD 'disposable_app_pw';
CREATE DATABASE producer_bounds_test OWNER phase5_bounds_writer;
\connect producer_bounds_test
CREATE EXTENSION IF NOT EXISTS postgis;
SQL
writer="postgresql+asyncpg://phase5_bounds_writer:disposable_writer_pw@127.0.0.1:$port/producer_bounds_test"
export DATABASE_URL="$writer"
export MIGRATOR_DATABASE_URL="$writer"
export ENVIRONMENT=test
export JWT_SECRET='test-secret-key-for-testing-minimum-32-chars'
stamp producer-migrate
PYTHONDONTWRITEBYTECODE=1 PYTHONPATH="$repo" "$alembic" upgrade head
docker exec -i "$producer_container" psql -q -U postgres -d producer_bounds_test -v ON_ERROR_STOP=1 -c \
  "GRANT USAGE ON SCHEMA public TO phase5_bounds_app; GRANT SELECT ON ALL TABLES IN SCHEMA public TO phase5_bounds_app;"

producer_log=$(mktemp "${test_home}/runs/phase5-producers.XXXXXX")
export PRODUCER_TEST_APP_URL="postgres://phase5_bounds_app:disposable_app_pw@127.0.0.1:$port/producer_bounds_test"
export PRODUCER_TEST_WRITER_URL="postgres://phase5_bounds_writer:disposable_writer_pw@127.0.0.1:$port/producer_bounds_test"
stamp producer-test
set +e
cargo test --manifest-path "$repo/rs/Cargo.toml" --locked -p verdaxis-read \
  --features contract-tests --test phase5_producers -- --ignored --test-threads=1 --nocapture 2>&1 | tee "$producer_log"
producer_status=${PIPESTATUS[0]}
set -e
echo "PRODUCER_TEST_EXIT=$producer_status"
grep -E '^test |^test result' "$producer_log" || true
if (( producer_status != 0 )); then
  rm -f "$producer_log"
  exit "$producer_status"
fi
if ! grep -Eq '^test result: ok\. [1-9][0-9]* passed' "$producer_log"; then
  echo "no populated producer case ran: refusing a green result" >&2
  rm -f "$producer_log"
  exit 1
fi
rm -f "$producer_log"
stamp done
echo GATES_DONE

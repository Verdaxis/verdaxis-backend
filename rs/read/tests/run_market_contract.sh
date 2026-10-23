#!/usr/bin/env bash
# Task 6: compare the shared visibility and trade-evidence SQL with the Python
# oracle on one disposable PostgreSQL container, then run the compliance
# overlay's populated-fixture test on that same seeded database after the
# overlay_contract_fixture.py matrix is added.
#
# Docker runs locally through Colima. The container binds a Docker-assigned
# loopback port, sets the oracle time zone, and is removed on exit. Never point
# this at staging or production.
set -euo pipefail
image='postgis/postgis:17-3.6-alpine@sha256:49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272'
db='verdaxis_market_test'
clock='2026-10-15T12:00:00+00:00'

# Fail before starting a container when the caller has not pointed the suite at
# the interpreter that carries the project's database dependencies.
: "${RUST_TEST_PYTHON:?Set an absolute Python path with the project database dependencies}"
export RUST_TEST_PYTHON

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
alembic="$(dirname "$RUST_TEST_PYTHON")/alembic"

container=''
log=''
expected=''
overlay_manifest=''
cleanup() {
  # An EXIT trap replaces the script's exit status with the status of its own
  # last command. Capture the real status first and restore it after the
  # container and scratch files are gone, so no failure is reported as success.
  local status=$?
  if [[ -n "$container" ]]; then
    docker logs "$container" || true
    # -v removes the anonymous data volume; a forced remove skips the --rm cleanup.
    docker rm -f -v "$container" >/dev/null || true
  fi
  if [[ -n "$log" ]]; then rm -f "$log"; fi
  if [[ -n "$expected" ]]; then rm -f "$expected"; fi
  if [[ -n "$overlay_manifest" ]]; then rm -f "$overlay_manifest"; fi
  exit "$status"
}
trap cleanup EXIT

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required" >&2
  exit 2
fi

expected=$(mktemp "${TMPDIR:-/tmp}/rust-market-expected.XXXXXX")
container=$(docker run --platform linux/amd64 -d --rm -e POSTGRES_PASSWORD=disposable_admin_pw -p 127.0.0.1::5432 "$image")
for i in {1..90}; do
  if docker exec "$container" pg_isready -h 127.0.0.1 -U postgres >/dev/null 2>&1; then break; fi
  sleep 1
done
port=$(docker port "$container" 5432/tcp)
port=${port##*:}
docker exec -i "$container" psql -U postgres -v ON_ERROR_STOP=1 <<SQL
CREATE ROLE rust_market_writer LOGIN NOINHERIT NOSUPERUSER PASSWORD 'disposable_writer_pw';
CREATE ROLE rust_app LOGIN NOINHERIT PASSWORD 'disposable_app_pw';
CREATE DATABASE $db OWNER rust_market_writer;
\connect $db
CREATE EXTENSION IF NOT EXISTS postgis;
ALTER DATABASE $db SET TimeZone TO 'Asia/Singapore';
SQL

release_sha=$(git -C "$repo" rev-parse HEAD)
writer="postgresql+asyncpg://rust_market_writer:disposable_writer_pw@127.0.0.1:$port/$db"
export ENVIRONMENT=test
export RELEASE_SHA="$release_sha"
export JWT_SECRET='test-secret-key-for-testing-minimum-32-chars'
export DATABASE_URL="$writer"
export MIGRATOR_DATABASE_URL="$writer"
export RUST_MARKET_WRITER_URL="$writer"
export RUST_MARKET_EXPECTED_DATABASE="$db"
export RUST_MARKET_TEST_CLOCK="$clock"
export RUST_MARKET_TEST_EXPECTED="$expected"
export RUST_MARKET_TEST_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/$db"

(
  cd "$repo"
  PYTHONDONTWRITEBYTECODE=1 PYTHONPATH="$repo" "$alembic" upgrade head
  # The Rust reads use the ordinary runtime app grants: read-only, no owner.
  PYTHONDONTWRITEBYTECODE=1 PYTHONPATH="$repo" "$RUST_TEST_PYTHON" \
    rs/read/tests/market_contract_oracle.py
)
docker exec -i "$container" psql -q -U postgres -d "$db" -v ON_ERROR_STOP=1 -c \
  "GRANT USAGE ON SCHEMA public TO rust_app; GRANT SELECT ON ALL TABLES IN SCHEMA public TO rust_app;"

log=$(mktemp "${TMPDIR:-/tmp}/rust-market-tests.XXXXXX")
set +e
cargo test --manifest-path "$repo/rs/Cargo.toml" --locked -p verdaxis-read \
  --features contract-tests --test market_contract_postgres -- --test-threads=1 2>&1 | tee "$log"
cargo_status=${PIPESTATUS[0]}
set -e
if (( cargo_status != 0 )); then
  exit "$cargo_status"
fi
if ! grep -Eq '^test result: ok\. [1-9][0-9]* passed' "$log"; then
  echo "no Rust test ran: refusing a green result" >&2
  exit 1
fi

# Phase 5 lane O: the compliance overlay's populated-fixture test needs the
# oracle-seeded market database above, plus unequal tenant vessel counts and
# explicit visible/hidden orders. Its fixture must run after the market
# contract tests: it seeds another public ASK clone, which would change the
# exact visibility sets those tests assert. run_postgres.sh skips this test by
# name for the same reason.
overlay_manifest=$(mktemp "${TMPDIR:-/tmp}/rust-overlay-cases.XXXXXX")
export RUST_OVERLAY_TEST_CASES="$overlay_manifest"
(
  cd "$repo"
  PYTHONDONTWRITEBYTECODE=1 PYTHONPATH="$repo" "$RUST_TEST_PYTHON" \
    rs/read/tests/overlay_contract_fixture.py
)

rm -f "$log"
log=$(mktemp "${TMPDIR:-/tmp}/rust-overlay-tests.XXXXXX")
set +e
cargo test --manifest-path "$repo/rs/Cargo.toml" --locked -p verdaxis-read \
  --features contract-tests --test phase5_compliance_overlay -- --test-threads=1 2>&1 | tee "$log"
cargo_status=${PIPESTATUS[0]}
set -e
rm -f "$overlay_manifest"
if (( cargo_status != 0 )); then
  exit "$cargo_status"
fi
if ! grep -Eq '^test result: ok\. [1-9][0-9]* passed' "$log"; then
  echo "no Rust overlay test ran: refusing a green result" >&2
  exit 1
fi

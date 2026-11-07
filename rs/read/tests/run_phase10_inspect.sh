#!/usr/bin/env bash
# Phase 10 Task 23 runner: the read-only inspection tools against a migrated
# disposable database with fixed expected report values.
#
# Docker runs locally. The container binds a Docker-assigned loopback port and
# is removed on exit. Never point this at staging or production.
set -euo pipefail
image='postgis/postgis:17-3.6-alpine@sha256:49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272'


repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
fixtures="$repo/rs/read/tests/fixtures"

container=''
log=''
cleanup() {
  # Keep the real exit status: the last cleanup command must not replace it.
  local status=$?
  if [[ -n "$container" ]]; then
    docker rm -f -v "$container" >/dev/null || true
  fi
  if [[ -n "$log" ]]; then rm -f "$log"; fi
  exit "$status"
}
trap cleanup EXIT

stamp() { echo "== $(date -u +%Y-%m-%dT%H:%M:%SZ) $*"; }

container=$(docker run --platform linux/amd64 -d --rm -e POSTGRES_PASSWORD=disposable_admin_pw -p 127.0.0.1::5432 "$image")
for _ in {1..90}; do
  if docker exec "$container" pg_isready -h 127.0.0.1 -U postgres >/dev/null 2>&1; then break; fi
  sleep 1
done
port=$(docker port "$container" 5432/tcp | head -1)
port=${port##*:}

# The owner can write (it migrates and loads the fixtures, and proves that the
# inspection transaction, not a missing privilege, refuses a write). The reader
# holds SELECT only.
docker exec -i "$container" psql -X -q -U postgres -v ON_ERROR_STOP=1 <<'SQL'
CREATE ROLE inspect_owner LOGIN NOINHERIT PASSWORD 'disposable_owner_pw';
CREATE ROLE inspect_reader LOGIN NOINHERIT PASSWORD 'disposable_reader_pw';
CREATE DATABASE verdaxis_inspect_test OWNER inspect_owner;
CREATE DATABASE verdaxis_inspect_clean_test OWNER inspect_owner;
SQL
for database in verdaxis_inspect_test verdaxis_inspect_clean_test; do
  docker exec -i "$container" psql -X -q -U postgres -d "$database" -v ON_ERROR_STOP=1 \
    -c 'CREATE EXTENSION IF NOT EXISTS postgis'
done

export ENVIRONMENT=test
export RELEASE_SHA=test
export JWT_SECRET='test-secret-key-for-testing-minimum-32-chars'

load() { # database fixture-file
  docker exec -i -e PGPASSWORD=disposable_owner_pw "$container" \
    psql -X -q -h 127.0.0.1 -U inspect_owner -d "$1" -v ON_ERROR_STOP=1 < "$fixtures/$2"
}

stamp migrate
for database in verdaxis_inspect_test verdaxis_inspect_clean_test; do
  docker exec -i "$container" psql -X -q -U inspect_owner -d "$database" \
    -v ON_ERROR_STOP=1 < "$repo/rs/read/migrations/1_baseline.sql" >/dev/null
done

stamp fixtures
load verdaxis_inspect_test phase10_inspect_seed.sql
load verdaxis_inspect_test phase10_inspect_market_seed.sql
load verdaxis_inspect_test phase10_inspect_write_probe.sql
load verdaxis_inspect_clean_test phase10_inspect_clean_seed.sql
for database in verdaxis_inspect_test verdaxis_inspect_clean_test; do
  docker exec -i "$container" psql -X -q -U postgres -d "$database" -v ON_ERROR_STOP=1 \
    -c 'GRANT USAGE ON SCHEMA public TO inspect_reader; GRANT SELECT ON ALL TABLES IN SCHEMA public TO inspect_reader'
done

export RUST_INSPECT_TEST_DATABASE_URL="postgres://inspect_reader:disposable_reader_pw@127.0.0.1:$port/verdaxis_inspect_test"
export RUST_INSPECT_CLEAN_DATABASE_URL="postgres://inspect_reader:disposable_reader_pw@127.0.0.1:$port/verdaxis_inspect_clean_test"
export RUST_INSPECT_OWNER_DATABASE_URL="postgres://inspect_owner:disposable_owner_pw@127.0.0.1:$port/verdaxis_inspect_test"
export SQLX_OFFLINE=true
RELEASE_SHA=$(git -C "$repo" rev-parse HEAD)
export RELEASE_SHA

log=$(mktemp "${TMPDIR:-/tmp}/phase10-inspect.XXXXXX")
stamp test
set +e
cargo test --manifest-path "$repo/rs/Cargo.toml" --locked -p verdaxis-read \
  --features contract-tests --test group_phase10 \
  -- phase10_inspect:: phase10_inspect_postgres:: --test-threads=1 2>&1 | tee "$log"
cargo_status=${PIPESTATUS[0]}
set -e
if (( cargo_status != 0 )); then
  exit "$cargo_status"
fi
# A green run that executed no PostgreSQL case is a false pass; refuse it.
if ! grep -Eq '^test phase10_inspect_postgres::inspect_blockers_matches_fixed_counts \.\.\. ok' "$log"; then
  echo "the fixed-count inspection case did not run: refusing a green result" >&2
  exit 1
fi
# The module filters leave out only the other modules of the group binary, so
# a filtered-out count is expected; an ignored case is not.
if grep -Eq '[1-9][0-9]* ignored' "$log"; then
  echo "a required case was ignored: refusing a green result" >&2
  exit 1
fi
stamp finished

#!/usr/bin/env bash
# Run fixed Rust market assertions and the independent SQL overlay matrix.
# The database is disposable and binds only an assigned loopback port.
set -euo pipefail
image='postgis/postgis:17-3.6-alpine@sha256:49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272'
db='verdaxis_market_test'
clock='2026-10-15T12:00:00+00:00'
python=${RUST_TEST_PYTHON:-python3}
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
container=''
log=''
overlay_manifest=''
cleanup() {
  local status=$?
  if [[ -n "$container" ]]; then
    docker logs "$container" || true
    docker rm -f -v "$container" >/dev/null || true
  fi
  if [[ -n "$log" ]]; then rm -f "$log"; fi
  if [[ -n "$overlay_manifest" ]]; then rm -f "$overlay_manifest"; fi
  exit "$status"
}
trap cleanup EXIT

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required" >&2
  exit 2
fi
container=$(docker run --platform linux/amd64 -d --rm -e POSTGRES_PASSWORD=disposable_admin_pw -p 127.0.0.1::5432 "$image")
for _ in {1..90}; do
  if docker exec "$container" pg_isready -h 127.0.0.1 -U postgres >/dev/null 2>&1; then break; fi
  sleep 1
done
port=$(docker port "$container" 5432/tcp)
port=${port##*:}
docker exec -i "$container" psql -U postgres -v ON_ERROR_STOP=1 <<SQL
CREATE ROLE verdaxis_migrator_staging LOGIN NOINHERIT NOSUPERUSER PASSWORD 'disposable_writer_pw';
CREATE ROLE rust_app LOGIN NOINHERIT PASSWORD 'disposable_app_pw';
CREATE DATABASE $db OWNER verdaxis_migrator_staging;
\connect $db
CREATE EXTENSION IF NOT EXISTS postgis;
ALTER DATABASE $db SET TimeZone TO 'Asia/Singapore';
SQL
export RELEASE_SHA
RELEASE_SHA=$(git -C "$repo" rev-parse HEAD)
export SQLX_OFFLINE=true
writer="postgresql://verdaxis_migrator_staging:disposable_writer_pw@127.0.0.1:$port/$db"
export RUST_MARKET_WRITER_URL="$writer"
export RUST_MARKET_EXPECTED_DATABASE="$db"
export RUST_MARKET_TEST_CLOCK="$clock"
export RUST_MARKET_TEST_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/$db"
export RUST_TEST_POSTGRES_CONTAINER="$container"

# Initialize the empty database with the native baseline before granting reads.
RUST_RUNNER_ENVIRONMENT=staging RUST_RUNNER_MIGRATOR_DATABASE_URL="$writer" \
  cargo test --manifest-path "$repo/rs/Cargo.toml" --locked -p verdaxis-read \
  --features contract-tests --test group_phase10 \
  phase10_migrate_postgres::runner_database_applies_the_baseline -- --ignored
PYTHONDONTWRITEBYTECODE=1 "$python" "$repo/rs/read/tests/market_contract_fixture.py"
docker exec -i "$container" psql -q -U postgres -d "$db" -v ON_ERROR_STOP=1 -c \
  "GRANT USAGE ON SCHEMA public TO rust_app; GRANT SELECT ON ALL TABLES IN SCHEMA public TO rust_app;"

log=$(mktemp "${TMPDIR:-/tmp}/rust-market-tests.XXXXXX")
set +e
# An optional list of test module filters permits focused SQL checks on the
# same matrix.
targets=${RUST_MARKET_TARGETS:-market_contract_postgres:: meaning_curve_queries:: meaning_orderbook_queries::}
# shellcheck disable=SC2086
cargo test --manifest-path "$repo/rs/Cargo.toml" --locked -p verdaxis-read \
  --features contract-tests --test group_market_reads -- $targets --test-threads=1 2>&1 | tee "$log"
cargo_status=${PIPESTATUS[0]}
set -e
if (( cargo_status != 0 )); then
  exit "$cargo_status"
fi
if ! grep -Eq '^test result: ok\. [1-9][0-9]* passed' "$log"; then
  echo "no Rust test ran: refusing a green result" >&2
  exit 1
fi

# Add overlay rows only after the exact market sets have been checked.
overlay_manifest=$(mktemp "${TMPDIR:-/tmp}/rust-overlay-cases.XXXXXX")
export RUST_OVERLAY_TEST_CASES="$overlay_manifest"
export RUST_OVERLAY_REQUIRED=1
PYTHONDONTWRITEBYTECODE=1 "$python" "$repo/rs/read/tests/overlay_contract_fixture.py"

rm -f "$log"
log=$(mktemp "${TMPDIR:-/tmp}/rust-overlay-tests.XXXXXX")
set +e
cargo test --manifest-path "$repo/rs/Cargo.toml" --locked -p verdaxis-read \
  --features contract-tests --test phase5_compliance_overlay -- --test-threads=1 2>&1 | tee "$log"
cargo_status=${PIPESTATUS[0]}
set -e
if (( cargo_status != 0 )); then
  exit "$cargo_status"
fi
if ! grep -Eq '^test result: ok\. [1-9][0-9]* passed' "$log"; then
  echo "no Rust overlay test ran: refusing a green result" >&2
  exit 1
fi

# The KYC execution gate on the seeded market with the overlay rows. This case
# changes KYC state as the schema owner, so it runs last.
rm -f "$log"
log=$(mktemp "${TMPDIR:-/tmp}/rust-kyc-market-tests.XXXXXX")
set +e
RUST_MARKET_TEST_MIGRATOR_URL="$writer" RUST_CONTRACT_CLOCK='2026-10-15T12:00:00Z' \
  cargo test --manifest-path "$repo/rs/Cargo.toml" --locked -p verdaxis-read \
  --features contract-tests --test phase4_kyc_policy -- --ignored --exact postgres::p4_t9_c01 \
  --test-threads=1 2>&1 | tee "$log"
cargo_status=${PIPESTATUS[0]}
set -e
if (( cargo_status != 0 )); then
  exit "$cargo_status"
fi
if ! grep -Eq '^test result: ok\. 1 passed; 0 failed' "$log"; then
  echo "the KYC market case did not run: refusing a green result" >&2
  exit 1
fi

#!/usr/bin/env bash
# Run the whole Rust workspace against a disposable PostgreSQL container.
#
# Docker runs locally through Colima. The container binds a Docker-assigned
# loopback port and is removed on exit. Never point this at staging or prod.
set -euo pipefail
image='postgis/postgis:17-3.6-alpine@sha256:49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272'

# Fail before starting a container when the caller has not pointed the suite at
# the interpreter that carries the project's database dependencies.
: "${RUST_TEST_PYTHON:?Set an absolute Python path with the project database dependencies}"
export RUST_TEST_PYTHON

container=''
log=''
cleanup() {
  # An EXIT trap replaces the script's exit status with the status of its own
  # last command. Capture the real status first and restore it after the
  # container and scratch log are gone, so no failure is reported as success.
  local status=$?
  if [[ -n "$container" ]]; then
    docker logs "$container" || true
    docker rm -f "$container" >/dev/null || true
  fi
  if [[ -n "$log" ]]; then rm -f "$log"; fi
  exit "$status"
}
trap cleanup EXIT

container=$(docker run --platform linux/amd64 -d --rm -e POSTGRES_PASSWORD=disposable_admin_pw -p 127.0.0.1::5432 "$image")
for i in {1..90}; do
  if docker exec "$container" pg_isready -h 127.0.0.1 -U postgres >/dev/null 2>&1; then break; fi
  sleep 1
done
port=$(docker port "$container" 5432/tcp)
port=${port##*:}
docker exec -i "$container" psql -U postgres -v ON_ERROR_STOP=1 <<'SQL'
CREATE ROLE rust_app LOGIN NOINHERIT PASSWORD 'disposable_app_pw';
CREATE ROLE rust_member LOGIN NOINHERIT PASSWORD 'disposable_member_pw';
CREATE ROLE rust_group NOLOGIN;
GRANT rust_group TO rust_member;
CREATE ROLE rust_migrator LOGIN NOINHERIT PASSWORD 'disposable_migrator_pw';
CREATE DATABASE verdaxis_rust_test OWNER rust_migrator;
\connect verdaxis_rust_test
REVOKE ALL ON SCHEMA public FROM PUBLIC;
GRANT USAGE ON SCHEMA public TO rust_app, rust_member;
SET ROLE rust_migrator;
CREATE TABLE alembic_version(version_num varchar(64));
INSERT INTO alembic_version VALUES ('kycg_20261014_staging');
SQL
export RUST_TEST_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_rust_test"
export RUST_TEST_MEMBER_DATABASE_URL="postgres://rust_member:disposable_member_pw@127.0.0.1:$port/verdaxis_rust_test"
export RUST_TEST_MIGRATOR_DATABASE_URL="postgres://rust_migrator:disposable_migrator_pw@127.0.0.1:$port/verdaxis_rust_test"
export RELEASE_SHA=$(git rev-parse HEAD)

log=$(mktemp "${TMPDIR:-/tmp}/rust-postgres-tests.XXXXXX")
set +e
# The Task 6 visibility and trade-evidence matrices need the seeded fixture
# catalog and the Python oracle, which this database does not carry. They are
# named here rather than silently skipped: rs/read/tests/run_market_contract.sh
# runs them against their own disposable database.
cargo test --manifest-path rs/Cargo.toml --locked --workspace --features contract-tests \
  -- --test-threads=1 --skip public_visibility_matrix_matches_python \
  --skip trade_evidence_matrix_matches_python 2>&1 | tee "$log"
cargo_status=${PIPESTATUS[0]}
set -e
if (( cargo_status != 0 )); then
  exit "$cargo_status"
fi
# A green cargo run that executed no test is a false pass; refuse it.
if ! grep -Eq '^test result: ok\. [1-9][0-9]* passed' "$log"; then
  echo "no Rust test ran: refusing a green result" >&2
  exit 1
fi

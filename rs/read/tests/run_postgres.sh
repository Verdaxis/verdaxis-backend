#!/usr/bin/env bash
# Run the whole Rust workspace against a disposable PostgreSQL container, then
# the migrated market-write component targets on a second migrated database.
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
    # -v removes the anonymous data volume; a forced remove skips the --rm cleanup.
    docker rm -f -v "$container" >/dev/null || true
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
CREATE ROLE rust_backup NOLOGIN;
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
# Settings import requirements for the alembic migration below.
export ENVIRONMENT="${ENVIRONMENT:-test}"
export JWT_SECRET="${JWT_SECRET:-test-secret-key-for-testing-minimum-32-chars}"

# The market-write component suites need the fully migrated schema and the
# converged runtime grants, not the minimal workspace fixture above. Two
# databases share one container; each target receives its own runtime and
# admin URLs on the same numeric-loopback endpoint.
market_image_role_setup() {
  local database=$1
  docker exec -i "$container" psql -U postgres -v ON_ERROR_STOP=1 <<SQL
CREATE DATABASE $database OWNER rust_migrator;
\connect $database
CREATE EXTENSION IF NOT EXISTS postgis;
ALTER SCHEMA public OWNER TO rust_migrator;
GRANT USAGE ON SCHEMA public TO rust_app;
SQL
}
for database in verdaxis_trades_test verdaxis_create_test verdaxis_amend_test verdaxis_confirm_test verdaxis_publish_test verdaxis_settlement_test verdaxis_freeze_enforcement_test; do
  market_image_role_setup "$database"
done
export RUST_TRADES_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/verdaxis_trades_test"
export RUST_TRADES_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_trades_test"
export RUST_CREATE_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/verdaxis_create_test"
export RUST_CREATE_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_create_test"
export RUST_AMEND_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/verdaxis_amend_test"
export RUST_AMEND_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_amend_test"
export RUST_CONFIRM_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/verdaxis_confirm_test"
export RUST_CONFIRM_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_confirm_test"
export RUST_FREEZE_ENFORCEMENT_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/verdaxis_freeze_enforcement_test"
export RUST_FREEZE_ENFORCEMENT_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_freeze_enforcement_test"
export RUST_SETTLEMENT_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/verdaxis_settlement_test"
export RUST_SETTLEMENT_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_settlement_test"
export RUST_PUBLISH_ADMIN_DATABASE_URL="postgres://postgres:disposable_admin_pw@127.0.0.1:$port/verdaxis_publish_test"
export RUST_PUBLISH_DATABASE_URL="postgres://rust_app:disposable_app_pw@127.0.0.1:$port/verdaxis_publish_test"
for database in verdaxis_trades_test verdaxis_create_test verdaxis_amend_test verdaxis_confirm_test verdaxis_publish_test verdaxis_settlement_test verdaxis_freeze_enforcement_test; do
  export PGHOST=127.0.0.1 PGPORT="$port" PGUSER=rust_migrator \
    PGPASSWORD=disposable_migrator_pw PGDATABASE="$database"
  export DATABASE_URL="postgresql+asyncpg://rust_migrator:disposable_migrator_pw@127.0.0.1:$port/$database" \
    MIGRATOR_DATABASE_URL="postgresql+asyncpg://rust_migrator:disposable_migrator_pw@127.0.0.1:$port/$database"
  "$RUST_TEST_PYTHON" -m alembic upgrade p89_20261022_trade_freeze
  psql -X --single-transaction -v ON_ERROR_STOP=1 \
    -v database_name="$database" -v app_role=rust_app \
    -v migrator_role=rust_migrator -v backup_role=rust_backup \
    -f deploy/postgres/converge_runtime_object_acls.sql
done
unset DATABASE_URL MIGRATOR_DATABASE_URL PGHOST PGPORT PGUSER PGPASSWORD PGDATABASE

log=$(mktemp "${TMPDIR:-/tmp}/rust-postgres-tests.XXXXXX")
set +e
# The Task 6 visibility, trade-evidence and market-policy matrices need the
# seeded fixture catalog and the Python oracle, which this database does not
# carry. They are named here rather than silently skipped:
# rs/read/tests/run_market_contract.sh runs them against their own disposable
# database.
cargo test --manifest-path rs/Cargo.toml --locked --workspace --features contract-tests \
  -- --test-threads=1 --skip public_visibility_matrix_matches_python \
  --skip trade_evidence_matrix_matches_python \
  --skip benchmark_key_first_matches_key_join_last \
  --skip biofuel_supplier_metadata_clause_hides_only_failing_asks \
  --skip eligibility_sets_match_legacy_for_the_curve_relation \
  --skip eligibility_sets_match_legacy_predicate_for_every_order 2>&1 | tee "$log"
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

# The market-write component suites mark their database cases explicitly;
# execute each named target on its own migrated disposable database.
market_log=$(mktemp "${TMPDIR:-/tmp}/rust-postgres-market.XXXXXX")
set +e
cargo test --manifest-path rs/Cargo.toml --locked -p verdaxis-read --features contract-tests \
  --test market_write_pg_trades --test market_write_pg_create \
  --test market_write_pg_amend --test market_write_pg_confirm \
  --test market_write_pg_publish --test market_write_pg_settlement \
  --test market_write_pg_freeze_enforcement \
  -- --include-ignored --test-threads=1 2>&1 | tee "$market_log"
market_status=${PIPESTATUS[0]}
set -e
if (( market_status != 0 )); then
  exit "$market_status"
fi
if ! grep -Eq '^test result: ok\. [1-9][0-9]* passed' "$market_log"; then
  echo "no market-write test ran: refusing a green result" >&2
  exit 1
fi

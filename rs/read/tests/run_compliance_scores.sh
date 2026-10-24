#!/usr/bin/env bash
# Task 9 gate: fmt, both clippy configurations, the offline workspace, cargo
# audit, and the compliance score suite against one disposable PostgreSQL
# container migrated with the accepted literal chain.
#
# Docker runs locally through Colima on the M1 test host. The container binds a
# Docker-assigned loopback port and is removed on exit. Never point this at
# staging or production.
set -euo pipefail
image='postgis/postgis:17-3.6-alpine@sha256:49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272'
db='verdaxis_compliance_test'
test_home="${VERDAXIS_TEST_HOME:-/home/verdaxis-test}"

: "${RUST_TEST_PYTHON:?Set an absolute Python path with the project database dependencies}"
export RUST_TEST_PYTHON

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
alembic="$(dirname "$RUST_TEST_PYTHON")/alembic"

stamp() { echo "== $(date -u +%Y-%m-%dT%H:%M:%SZ) $*"; }

container=''
cleanup() {
  local status=$?
  if [[ -n "$container" ]]; then
    docker logs "$container" || true
    docker rm -f -v "$container" >/dev/null || true
  fi
  exit "$status"
}
trap cleanup EXIT

cd "$repo"
echo "GATE_SHA=$(git rev-parse HEAD)"
echo "HOST=$(hostname -s) RUSTC=$(rustc -V 2>/dev/null)"
echo "TREE=$(git status --porcelain --untracked-files=no | wc -l | tr -d ' ') changed tracked file(s)"
# The build script embeds the release identity; every cargo command needs it.
export RELEASE_SHA=$(git rev-parse HEAD)

stamp fmt
cargo fmt --manifest-path rs/Cargo.toml --all -- --check
echo "FMT_EXIT=$?"

stamp clippy-workspace
SQLX_OFFLINE=true cargo clippy --manifest-path rs/Cargo.toml --locked --workspace --all-targets -- -D warnings
echo "CLIPPY_EXIT=$?"

stamp clippy-contract
SQLX_OFFLINE=true cargo clippy --manifest-path rs/Cargo.toml --locked -p verdaxis-read \
  --features contract-tests --all-targets -- -D warnings
echo "CLIPPY_CONTRACT_EXIT=$?"

stamp offline-workspace
SQLX_OFFLINE=true cargo test --manifest-path rs/Cargo.toml --locked --workspace
echo "WORKSPACE_EXIT=$?"

stamp audit
set +e
(cd rs && cargo audit)
echo "AUDIT_EXIT=$?"
set -e

stamp postgres
container=$(docker run --platform linux/amd64 -d --rm -e POSTGRES_PASSWORD=disposable_admin_pw -p 127.0.0.1::5432 "$image")
for _ in {1..90}; do
  if docker exec "$container" pg_isready -h 127.0.0.1 -U postgres >/dev/null 2>&1; then break; fi
  sleep 1
done
port=$(docker port "$container" 5432/tcp)
port=${port##*:}
docker exec -i "$container" psql -U postgres -v ON_ERROR_STOP=1 <<SQL
CREATE ROLE task9_writer LOGIN NOINHERIT NOSUPERUSER PASSWORD 'disposable_writer_pw';
CREATE ROLE task9_app LOGIN NOINHERIT PASSWORD 'disposable_app_pw';
CREATE DATABASE $db OWNER task9_writer;
\connect $db
CREATE EXTENSION IF NOT EXISTS postgis;
ALTER DATABASE $db SET TimeZone TO 'UTC';
SQL

release_sha=$(git rev-parse HEAD)
writer="postgresql+asyncpg://task9_writer:disposable_writer_pw@127.0.0.1:$port/$db"
export ENVIRONMENT=test
export RELEASE_SHA="$release_sha"
export JWT_SECRET='test-secret-key-for-testing-minimum-32-chars'
export DATABASE_URL="$writer"
export MIGRATOR_DATABASE_URL="$writer"

stamp migrate
PYTHONDONTWRITEBYTECODE=1 PYTHONPATH="$repo" "$alembic" upgrade head
echo "MIGRATE_EXIT=$?"

# The runtime role holds read access to the compliance relations and the
# current-account update lock, as the deployed app role does.
docker exec -i "$container" psql -q -U postgres -d "$db" -v ON_ERROR_STOP=1 -c \
  "GRANT USAGE ON SCHEMA public TO task9_app; GRANT SELECT ON ALL TABLES IN SCHEMA public TO task9_app; GRANT UPDATE ON users TO task9_app;"

export RUST_COMPLIANCE_TEST_DATABASE_URL="postgres://task9_app:disposable_app_pw@127.0.0.1:$port/$db"
export RUST_COMPLIANCE_TEST_WRITER_URL="postgres://task9_writer:disposable_writer_pw@127.0.0.1:$port/$db"

log=$(mktemp "${test_home}/runs/task9-tests.XXXXXX")
stamp cargo-test
set +e
cargo test --manifest-path "$repo/rs/Cargo.toml" --locked -p verdaxis-read \
  --features contract-tests --test phase5_compliance_scores -- --test-threads=1 --nocapture 2>&1 | tee "$log"
cargo_status=${PIPESTATUS[0]}
set -e
echo "SCORES_TEST_EXIT=$cargo_status"
grep -E '^test |^test result' "$log" || true
if (( cargo_status != 0 )); then
  rm -f "$log"
  exit "$cargo_status"
fi
if ! grep -Eq '^test result: ok\. [1-9][0-9]* passed' "$log"; then
  echo "no compliance test ran: refusing a green result" >&2
  rm -f "$log"
  exit 1
fi
if grep -q 'compliance score database matrix not run' "$log"; then
  echo "database matrix did not run: refusing a green result" >&2
  rm -f "$log"
  exit 1
fi
rm -f "$log"
stamp done
echo GATES_DONE

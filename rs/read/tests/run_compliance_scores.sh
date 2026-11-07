#!/usr/bin/env bash
# Compliance score and cross-group suites against one disposable PostgreSQL
# container initialized from the frozen SQL baseline. Formatting, both clippy
# configurations, the offline workspace and advisories have separate CI gates.
#
# Docker runs locally through Colima on the M1 test host. The container binds a
# Docker-assigned loopback port and is removed on exit. Never point this at
# staging or production.
set -euo pipefail
image='postgis/postgis:17-3.6-alpine@sha256:49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272'
db='verdaxis_compliance_test'

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"

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
RELEASE_SHA=$(git rev-parse HEAD)
export RELEASE_SHA

stamp postgres
container=$(docker run --platform linux/amd64 -d --rm -e POSTGRES_PASSWORD=disposable_admin_pw -p 127.0.0.1::5432 "$image")
for _ in {1..90}; do
  if docker exec "$container" pg_isready -h 127.0.0.1 -U postgres >/dev/null 2>&1; then break; fi
  sleep 1
done
port=$(docker port "$container" 5432/tcp)
port=${port##*:}
docker exec -i "$container" psql -U postgres -v ON_ERROR_STOP=1 <<SQL
CREATE ROLE verdaxis_migrator_staging LOGIN NOINHERIT NOSUPERUSER PASSWORD 'disposable_writer_pw';
CREATE ROLE task9_app LOGIN NOINHERIT PASSWORD 'disposable_app_pw';
-- Deployed app sessions carry a short lock deadline; an unscoped route
-- inherits it instead of setting a transaction-local one.
ALTER ROLE task9_app SET lock_timeout = '150ms';
CREATE DATABASE $db OWNER verdaxis_migrator_staging;
\connect $db
CREATE EXTENSION IF NOT EXISTS postgis;
ALTER SCHEMA public OWNER TO verdaxis_migrator_staging;
ALTER DATABASE $db SET TimeZone TO 'UTC';
SQL

release_sha=$(git rev-parse HEAD)
export RELEASE_SHA="$release_sha"

stamp migrate
RUST_RUNNER_ENVIRONMENT=staging RUST_RUNNER_MIGRATOR_DATABASE_URL="postgresql://verdaxis_migrator_staging:disposable_writer_pw@127.0.0.1:$port/$db" \
  cargo test --manifest-path "$repo/rs/Cargo.toml" --locked -p verdaxis-read \
  --features contract-tests --test phase10_migrate_postgres \
  runner_database_adopts_to_the_baseline -- --ignored
echo "MIGRATE_EXIT=$?"

# The runtime role holds read access to the compliance relations and the
# current-account update lock, as the deployed app role does.
docker exec -i "$container" psql -q -U postgres -d "$db" -v ON_ERROR_STOP=1 -c \
  "GRANT USAGE ON SCHEMA public TO task9_app; GRANT SELECT ON ALL TABLES IN SCHEMA public TO task9_app; GRANT UPDATE ON users TO task9_app;"

export RUST_COMPLIANCE_TEST_DATABASE_URL="postgres://task9_app:disposable_app_pw@127.0.0.1:$port/$db"
export RUST_COMPLIANCE_TEST_WRITER_URL="postgres://verdaxis_migrator_staging:disposable_writer_pw@127.0.0.1:$port/$db"
# This runner supplies the database, so a missing URL is a failure, not a skip.
export RUST_COMPLIANCE_REQUIRED=1

scores_log=$(mktemp "${TMPDIR:-/tmp}/compliance-scores.XXXXXX")
stamp scores-test
set +e
cargo test --manifest-path "$repo/rs/Cargo.toml" --locked -p verdaxis-read \
  --features contract-tests --test phase5_compliance_scores -- --test-threads=1 --nocapture 2>&1 | tee "$scores_log"
scores_status=${PIPESTATUS[0]}
echo "SCORES_TEST_EXIT=$scores_status"
grep -E '^test result' "$scores_log" || true
task12_log=$(mktemp "${TMPDIR:-/tmp}/compliance-cross-group.XXXXXX")
stamp task12-test
cargo test --manifest-path "$repo/rs/Cargo.toml" --locked -p verdaxis-read \
  --features contract-tests --test phase5_postgres -- --test-threads=1 --nocapture 2>&1 | tee "$task12_log"
task12_status=${PIPESTATUS[0]}
set -e
echo "TASK12_TEST_EXIT=$task12_status"
grep -E '^test |^test result' "$task12_log" || true
# Each part keeps every guard against its own log: the exit code, a green run
# that executed no test, and the not-run marker the part's tests print when
# their disposable URLs are missing. Each log is removed only after its own
# checks.
if (( scores_status != 0 )); then
  echo "scores matrix failed: $scores_status" >&2
  rm -f "$scores_log" "$task12_log"
  exit "$scores_status"
fi
if ! grep -Eq '^test result: ok\. [1-9][0-9]* passed' "$scores_log"; then
  echo "no compliance score test ran: refusing a green result" >&2
  rm -f "$scores_log" "$task12_log"
  exit 1
fi
if grep -q 'compliance score database matrix not run' "$scores_log"; then
  echo "compliance score database matrix did not run: refusing a green result" >&2
  rm -f "$scores_log" "$task12_log"
  exit 1
fi
rm -f "$scores_log"
if (( task12_status != 0 )); then
  echo "cross-group matrix failed: $task12_status" >&2
  rm -f "$task12_log"
  exit "$task12_status"
fi
if ! grep -Eq '^test result: ok\. [1-9][0-9]* passed' "$task12_log"; then
  echo "no cross-group test ran: refusing a green result" >&2
  rm -f "$task12_log"
  exit 1
fi
if grep -q 'cross-group matrix not run' "$task12_log"; then
  echo "cross-group database matrix did not run: refusing a green result" >&2
  rm -f "$task12_log"
  exit 1
fi
rm -f "$task12_log"
stamp "done"
echo GATES_DONE

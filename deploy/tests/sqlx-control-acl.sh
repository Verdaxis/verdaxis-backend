#!/bin/bash
# Phase 10 Task 6: the sqlx history (_sqlx_migrations) is migration control.
# The existing ACL convergence already governs it like any table without an app
# policy entry; this test keeps that true. Against a disposable PostgreSQL (the
# pinned image of rs/read/tests/run_postgres.sh) with the production role names,
# prove that ACL convergence
#   - denies the application role any access to the history,
#   - converges twice to the same ACLs and leaves writes to the migrator only,
#   - lets the backup role dump and restore the history but not write it,
#   - refuses a history that another role owns.
# Usage: bash deploy/tests/sqlx-control-acl.sh (from the repository root)
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cd "$ROOT"
IMAGE=$(sed -n "s/^image='\(postgis\/postgis:[^']*\)'$/\1/p" rs/read/tests/run_postgres.sh)
[[ -n $IMAGE ]] || { echo "the pinned PostGIS image was not found" >&2; exit 1; }

container=$(docker run --platform linux/amd64 -d --rm -e POSTGRES_PASSWORD=disposable_admin_pw -p 127.0.0.1::5432 "$IMAGE")
trap 'docker rm -f -v "$container" > /dev/null 2>&1 || true' EXIT
port=$(docker port "$container" 5432/tcp | sed -n 's/^127\.0\.0\.1://p' | head -n 1)
for _ in $(seq 1 120); do
  docker exec "$container" pg_isready -h 127.0.0.1 -U postgres -q 2> /dev/null && break
  sleep 1
done

failures=0
pass() { echo "  ok   $1"; }
fail() {
  echo "  FAIL $1" >&2
  failures=$((failures + 1))
}
# psql inside the container: its client matches the server and pg_dump.
as_role() {
  local role=$1 database=$2
  shift 2
  docker exec -i "$container" psql -U "$role" -d "$database" -X -q -v ON_ERROR_STOP=1 "$@"
}
denied() {
  local role=$1 database=$2 statement=$3
  local output
  output=$(as_role "$role" "$database" -c "$statement" 2>&1) && return 1
  grep -q 'permission denied' <<< "$output"
}

as_role postgres postgres <<'SQL'
CREATE ROLE verdaxis_migrator LOGIN NOINHERIT PASSWORD 'disposable_migrator_pw';
CREATE ROLE verdaxis_app LOGIN NOINHERIT PASSWORD 'disposable_app_pw';
CREATE ROLE verdaxis_backup LOGIN NOINHERIT PASSWORD 'disposable_backup_pw';
SQL
make_database() {
  local database=$1
  as_role postgres postgres -c "CREATE DATABASE $database OWNER verdaxis_migrator"
  as_role postgres "$database" <<'SQL'
CREATE EXTENSION IF NOT EXISTS postgis;
ALTER SCHEMA public OWNER TO verdaxis_migrator;
GRANT USAGE ON SCHEMA public TO verdaxis_app, verdaxis_backup;
SQL
}
# An adopted database: the baseline schema and the sqlx history with version 1.
adopted_database() {
  local database=$1
  make_database "$database"
  as_role verdaxis_migrator "$database" < rs/read/migrations/1_baseline.sql > /dev/null
  as_role verdaxis_migrator "$database" <<'SQL'
CREATE TABLE public._sqlx_migrations (
    version BIGINT PRIMARY KEY, description TEXT NOT NULL,
    installed_on TIMESTAMPTZ NOT NULL DEFAULT now(), success BOOLEAN NOT NULL,
    checksum BYTEA NOT NULL, execution_time BIGINT NOT NULL);
INSERT INTO public._sqlx_migrations (version, description, success, checksum, execution_time)
VALUES (1, 'baseline', true, '\x00', 0);
SQL
}
# The repository's ACL convergence, run by this Mac's psql as the administrator
# so that its \i of app_acl_policy.sql resolves inside the checkout.
converge() {
  local database=$1
  PGPASSWORD=disposable_admin_pw psql -X -q -h 127.0.0.1 -p "$port" -U postgres -d "$database" \
    --single-transaction -v ON_ERROR_STOP=1 \
    -v database_name="$database" -v app_role=verdaxis_app \
    -v migrator_role=verdaxis_migrator -v backup_role=verdaxis_backup \
    -f deploy/postgres/converge_runtime_object_acls.sql > /dev/null
}
acl_snapshot() {
  as_role postgres "$1" -tA -c "SELECT string_agg(relname || '=' || COALESCE(relacl::text, '-'), E'\n' ORDER BY relname)
    FROM pg_class WHERE relnamespace = 'public'::regnamespace AND relkind IN ('r', 'p', 'S')"
}

echo "== sqlx history ACL"
adopted_database verdaxis_acl_test
converge verdaxis_acl_test

# app_role_is_denied_sqlx_control_table
if denied verdaxis_app verdaxis_acl_test 'SELECT count(*) FROM public._sqlx_migrations' \
  && denied verdaxis_app verdaxis_acl_test "INSERT INTO public._sqlx_migrations VALUES (9, 'x', now(), true, '\x00', 0)"; then
  pass "app_role_is_denied_sqlx_control_table"
else
  fail "app_role_is_denied_sqlx_control_table"
fi

# sqlx_control_acl_converges_twice_with_migrator_only_writes
first=$(acl_snapshot verdaxis_acl_test)
converge verdaxis_acl_test
second=$(acl_snapshot verdaxis_acl_test)
if [[ $first == "$second" ]] \
  && as_role verdaxis_migrator verdaxis_acl_test -c "INSERT INTO public._sqlx_migrations VALUES (9, 'probe', now(), true, '\x00', 0); DELETE FROM public._sqlx_migrations WHERE version = 9" > /dev/null \
  && denied verdaxis_backup verdaxis_acl_test "INSERT INTO public._sqlx_migrations VALUES (9, 'x', now(), true, '\x00', 0)"; then
  pass "sqlx_control_acl_converges_twice_with_migrator_only_writes"
else
  fail "sqlx_control_acl_converges_twice_with_migrator_only_writes"
fi

# backup_reader_can_dump_and_restore_sqlx_history_but_cannot_write_it
dumped=$(docker exec "$container" pg_dump -U verdaxis_backup --no-owner --no-privileges \
  --table=public._sqlx_migrations verdaxis_acl_test 2> /dev/null || true)
make_database verdaxis_acl_restore
restored=""
if [[ -n $dumped ]] && as_role postgres verdaxis_acl_restore <<< "$dumped" > /dev/null 2>&1; then
  restored=$(as_role postgres verdaxis_acl_restore -tA -c "SELECT version || '/' || description || '/' || success FROM public._sqlx_migrations ORDER BY version")
fi
if [[ $restored == "1/baseline/true" ]] \
  && denied verdaxis_backup verdaxis_acl_test 'UPDATE public._sqlx_migrations SET success = false' \
  && denied verdaxis_backup verdaxis_acl_test 'DELETE FROM public._sqlx_migrations'; then
  pass "backup_reader_can_dump_and_restore_sqlx_history_but_cannot_write_it"
else
  fail "backup_reader_can_dump_and_restore_sqlx_history_but_cannot_write_it"
fi

# A leftover Alembic history table is governed like any unlisted table: the
# application role is denied it, and the backup role may read it for a dump
# but never write it.
# The migrator owns it, as bootstrap_roles.sql made it own the retained table.
adopted_database verdaxis_acl_alembic
as_role verdaxis_migrator verdaxis_acl_alembic <<'SQL' > /dev/null
CREATE TABLE public.alembic_version (version_num varchar(64) NOT NULL);
INSERT INTO public.alembic_version VALUES ('ob_20261031_offboarding');
SQL
converge verdaxis_acl_alembic
converge verdaxis_acl_alembic
if denied verdaxis_app verdaxis_acl_alembic 'SELECT count(*) FROM public.alembic_version' \
  && denied verdaxis_backup verdaxis_acl_alembic "UPDATE public.alembic_version SET version_num = 'x'" \
  && denied verdaxis_backup verdaxis_acl_alembic 'DELETE FROM public.alembic_version'; then
  pass "leftover_alembic_history_is_denied_to_the_app_and_read_only_to_backup"
else
  fail "leftover_alembic_history_is_denied_to_the_app_and_read_only_to_backup"
fi

# A history owned by another role is refused by convergence.
adopted_database verdaxis_acl_owner
as_role postgres verdaxis_acl_owner -c 'ALTER TABLE public._sqlx_migrations OWNER TO verdaxis_app' > /dev/null
if converge verdaxis_acl_owner 2> /dev/null; then
  fail "convergence_refuses_a_history_owned_by_another_role"
else
  pass "convergence_refuses_a_history_owned_by_another_role"
fi

if ((failures > 0)); then
  echo "SQLX_CONTROL_ACL=failed ($failures)" >&2
  exit 1
fi
echo "SQLX_CONTROL_ACL=ok"

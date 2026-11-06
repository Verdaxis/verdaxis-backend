#!/usr/bin/env bash
# Run inside a disposable Debian container with PostgreSQL server and Python 3.
# The caller owns the container; never run this fixture on an installed host.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
[[ ${VERDAXIS_DISPOSABLE_READER_TEST:-} == 1 && $(id -u) == 0 ]] || exit 2
pg_ctlcluster 15 main start
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
helper=$ROOT/deploy/scripts/provision-bundle-reader.sh
check() { "$helper" --environment "$1" --check > "$work/output" 2>&1; }
admin() { runuser -u postgres -- psql -XqAt -v ON_ERROR_STOP=1 "$@"; }
admin -c "ALTER SYSTEM SET log_statement = 'all'"
admin -c 'SELECT pg_reload_conf()' > /dev/null
for environment in production staging; do
    role=verdaxis_bundle_reader
    database=verdaxis_prod
    if [[ $environment == staging ]]; then
        role=${role}_staging
        database=verdaxis_staging
    fi
    service=/etc/verdaxis-bundle-readers/$environment-bundle-reader.pg_service.conf
    admin -c "CREATE DATABASE $database"
    if check "$environment"; then echo 'missing reader accepted' >&2; exit 1; fi
    "$helper" --environment "$environment" > "$work/output" 2>&1
    [[ $(stat -c '%u:%a' "$service") == 0:600 ]]
    check "$environment"
    cp "$service" "$work/service"
    verifier=$(admin -c "SELECT rolpassword FROM pg_authid WHERE rolname='$role'")
    "$helper" --environment "$environment" > "$work/output" 2>&1
    cmp "$service" "$work/service"
    [[ $(admin -c "SELECT rolpassword FROM pg_authid WHERE rolname='$role'") == "$verifier" ]]
    [[ $(admin -c "SELECT rolcanlogin AND NOT (rolsuper OR rolinherit OR rolcreatedb OR rolcreaterole OR rolreplication OR rolbypassrls) FROM pg_roles WHERE rolname='$role'") == t ]]
    [[ $(admin -c "SELECT count(*) FROM pg_auth_members WHERE member=(SELECT oid FROM pg_roles WHERE rolname='$role') OR roleid=(SELECT oid FROM pg_roles WHERE rolname='$role')") == 0 ]]
    [[ $(admin -d "$database" -c "SELECT count(*) FROM pg_shdepend WHERE refobjid=(SELECT oid FROM pg_roles WHERE rolname='$role') AND deptype IN ('a','o')") == 0 ]]
    password=$(grep '^password=' "$service" | cut -d= -f2-)
    if grep -qF "$password" "$work/output"; then echo 'password reached output' >&2; exit 1; fi
    sed -i 's/^password=.*/password=0000000000000000000000000000000000000000000000000000000000000000/' "$service"
    if check "$environment"; then echo 'wrong password accepted' >&2; exit 1; fi
    if "$helper" --environment "$environment" > "$work/output" 2>&1; then echo 'wrong password silently rotated' >&2; exit 1; fi
    cp "$work/service" "$service"
    chmod 0640 "$service"
    if check "$environment"; then echo 'public service accepted' >&2; exit 1; fi
    chmod 0600 "$service"
    mv "$service" "$work/saved"
    if check "$environment"; then echo 'missing service accepted' >&2; exit 1; fi
    if "$helper" --environment "$environment" > "$work/output" 2>&1; then echo 'existing role without service rotated' >&2; exit 1; fi
    mv "$work/saved" "$service"
    admin -c "ALTER ROLE $role CREATEDB"
    if check "$environment"; then echo 'unsafe role accepted' >&2; exit 1; fi
    if "$helper" --environment "$environment" > "$work/output" 2>&1; then echo 'unsafe role provisioned' >&2; exit 1; fi
    admin -c "ALTER ROLE $role NOCREATEDB"
    check "$environment"
    mv "$service" "$work/saved"
    ln -s "$work/saved" "$service"
    if check "$environment"; then echo 'symlink service accepted' >&2; exit 1; fi
    rm "$service"
    mv "$work/saved" "$service"
    admin -c "DROP ROLE $role"
    if check "$environment"; then echo 'absent role accepted with service' >&2; exit 1; fi
    "$helper" --environment "$environment" > "$work/output" 2>&1
    cmp "$service" "$work/service"
    check "$environment"
    if runuser -u nobody -- "$helper" --environment "$environment" > "$work/output" 2>&1; then
        echo 'non-root provisioning accepted' >&2; exit 1
    fi
    if grep -qF "$password" /var/log/postgresql/postgresql-15-main.log; then
        echo 'password reached database log' >&2; exit 1
    fi
    endpoint_sql=$(grep -F "SELECT 'DATABASE:'" "$ROOT/deploy/scripts/pre-m-bundle.sh" | cut -d '"' -f2)
    endpoint=$(printf '%b' "$endpoint_sql" | env -i PATH=/usr/bin:/bin PGSERVICEFILE="$service" PGSERVICE=pre_m_reader psql -XqAt)
    [[ $endpoint == "DATABASE:$database:$role:127.0.0.1:5432" ]] || {
        echo "pre_m_catalog_endpoint_identity: unexpected $endpoint" >&2; exit 1;
    }
    echo "ok bundle-reader $environment provisioning and preflight"
done

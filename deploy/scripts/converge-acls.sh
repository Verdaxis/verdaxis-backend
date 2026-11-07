#!/usr/bin/env bash
# Converge the committed runtime object and column ACL policy as the object
# owner (Phase 10 Task 7). The deploy runs it after `migrate` and before any
# restart. It replaces scripts/converge_runtime_acls.py.
#
# This helper checks its inputs and runs the verified release binary's
# `acl converge`, which reads the two database URLs from the deployed .env,
# checks both roles and the endpoint, and runs psql as the migrator. The
# migrator password never passes through this shell.
#
# Usage: converge-acls.sh --binary <verified verdaxis-read> --environment <env>
#          --release-sha <40 hex> --env-file <deployed .env> --bundle-root <dir>
set -euo pipefail
export LC_ALL=C

fail() {
    printf 'runtime ACL convergence refused: %s\n' "$1" >&2
    exit 1
}

binary=''
environment=''
release_sha=''
env_file=''
bundle_root=''
while (($#)); do
    (($# >= 2)) || fail 'an option has no value'
    case "$1" in
        --binary) binary=$2 ;;
        --environment) environment=$2 ;;
        --release-sha) release_sha=$2 ;;
        --env-file) env_file=$2 ;;
        --bundle-root) bundle_root=$2 ;;
        *) fail 'unknown option' ;;
    esac
    shift 2
done

[[ "$environment" == production || "$environment" == staging ]] \
    || fail 'the environment must be production or staging'
[[ "$release_sha" =~ ^[0-9a-f]{40}$ ]] || fail 'a full lowercase release SHA is required'
for path in "$binary" "$env_file" "$bundle_root"; do
    [[ "$path" == /* ]] || fail 'every path must be absolute'
done
[[ -f "$binary" && ! -L "$binary" && -x "$binary" ]] \
    || fail 'the release binary must be a regular executable file'
[[ -f "$env_file" && ! -L "$env_file" ]] \
    || fail 'the deployed environment file must be a regular file'
[[ -d "$bundle_root" && ! -L "$bundle_root" ]] \
    || fail 'the ACL bundle root must be a real directory'
for relative in deploy/postgres/app_acl_policy.sql deploy/postgres/converge_runtime_object_acls.sql; do
    [[ -f "$bundle_root/$relative" && ! -L "$bundle_root/$relative" ]] \
        || fail "the ACL bundle is missing a regular $relative"
done

if ! "$binary" acl converge --environment "$environment" --release-sha "$release_sha" \
    --env-file "$env_file" --bundle-root "$bundle_root" --apply; then
    fail 'owner-executable runtime ACL convergence failed'
fi
printf 'runtime ACL convergence complete: %s\n' "$environment"

#!/usr/bin/env bash
# Phase 10 Task 10: the unit installer without Python.
#
# The installer's KYC key-source check moved from inline Python to shell. This
# contract runs those exact installer lines (between the BEGIN/END markers of
# kyc-key-source-preflight) with `sudo` replaced by a fake that answers `stat`
# and `test` from a table, so no host path is read. It replaces, case for case,
# tests/unit/test_kyc_key_source_preflight.py:
#   test_source_metadata_policy (12 problems x 2 environments)
#     -> kyc_source_metadata_policy/<environment>/<problem>
#   test_absent_source_is_refused_with_the_placeholder_command
#     -> kyc_absent_source_is_refused_with_the_placeholder_command
#   test_missing_etc_verdaxis_is_refused_with_the_placeholder_command
#     -> kyc_missing_etc_verdaxis_is_refused_with_the_placeholder_command
#   test_preflight_precedes_dry_run_and_mutation
#     -> kyc_preflight_precedes_dry_run_and_mutation
#   test_preflight_never_reads_key_contents
#     -> kyc_preflight_never_reads_key_contents
# The unit bytes themselves are checked by rs/read/tests/phase10_units.rs.
#
# Usage: bash deploy/tests/units-contract.sh (from any directory)
set -euo pipefail
export LC_ALL=C
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
INSTALLER="$ROOT/scripts/install_systemd_units.sh"
PASSED=0

fail() {
    printf 'FAIL %s: %s\n' "$1" "$2" >&2
    exit 1
}

pass() {
    PASSED=$((PASSED + 1))
    printf 'ok %s\n' "$1"
}

PREFLIGHT=$(sed -n '/^# BEGIN kyc-key-source-preflight/,/^# END kyc-key-source-preflight/p' "$INSTALLER")
[[ -n "$PREFLIGHT" ]] || fail setup 'installer has no kyc-key-source-preflight block'

# Metadata of one path for the selected problem, in the installer's stat
# formats: '%F:%u:%a' for a directory, '%F:%u:%g:%a' for the key source.
# Prints nothing and fails when the path is absent.
fake_metadata() {
    local path=$1
    if [[ "$path" == "$TARGET" ]]; then
        case $PROBLEM in
            missing|missing_parent) return 1 ;;
            mode) echo 'regular file:0:0:644' ;;
            owner) echo 'regular file:1:0:400' ;;
            group) echo 'regular file:0:1:400' ;;
            symlink) echo 'symbolic link:0:0:777' ;;
            fifo) echo 'fifo:0:0:400' ;;
            directory) echo 'directory:0:0:400' ;;
            *) echo 'regular empty file:0:0:400' ;;
        esac
        return 0
    fi
    case $path in
        /|/etc) echo 'directory:0:755' ;;
        /etc/verdaxis)
            case $PROBLEM in
                missing_parent) return 1 ;;
                parent_write) echo 'directory:0:775' ;;
                parent_owner) echo 'directory:1000:755' ;;
                parent_symlink) echo 'symbolic link:0:777' ;;
                *) echo 'directory:0:755' ;;
            esac
            ;;
        *) fail "$CASE" "unexpected path $path" ;;
    esac
}

# The installer calls `sudo stat -c <format> -- <path>` and
# `sudo test -e|-L <path>`; anything else is a contract break.
sudo() {
    case "$1 $2" in
        'stat -c')
            [[ "$4" == -- ]] || fail "$CASE" "unexpected stat call: $*"
            fake_metadata "$5"
            ;;
        'test -e')
            fake_metadata "$3" > /dev/null
            ;;
        'test -L')
            [[ "$(fake_metadata "$3")" == 'symbolic link:'* ]]
            ;;
        *) fail "$CASE" "unexpected sudo call: $*" ;;
    esac
}

# Run the installer's preflight lines for one environment and problem.
# Sets STATUS and MESSAGE.
run_preflight() {
    local environment=$1
    PROBLEM=$2
    case $environment in
        production) TARGET=/etc/verdaxis/kyc-document-keys.json ;;
        staging) TARGET=/etc/verdaxis/kyc-document-keys-staging.json ;;
    esac
    STATUS=0
    # The block reads KYC_KEY_SOURCE, which the installer sets per environment.
    # shellcheck disable=SC2034
    MESSAGE=$(
        KYC_KEY_SOURCE=$TARGET
        eval "$PREFLIGHT" 2>&1
    ) || STATUS=$?
}

for environment in staging production; do
    for problem in none missing missing_parent mode owner group symlink fifo directory \
        parent_write parent_owner parent_symlink; do
        CASE="kyc_source_metadata_policy/$environment/$problem"
        run_preflight "$environment" "$problem"
        if [[ "$problem" == none ]]; then
            # A present root:root 0400 regular file passes, empty or not.
            [[ "$STATUS" == 0 && -z "$MESSAGE" ]] || fail "$CASE" "refused: $MESSAGE"
        else
            [[ "$STATUS" == 1 ]] || fail "$CASE" "exit $STATUS"
            case $problem in
                missing|missing_parent)
                    [[ "$MESSAGE" == *"$TARGET"* \
                        && "$MESSAGE" == *"printf '' > $TARGET"* \
                        && "$MESSAGE" == *0400* ]] || fail "$CASE" "$MESSAGE"
                    ;;
                *)
                    [[ "$MESSAGE" == *'KYC key source metadata is unsafe'* ]] \
                        || fail "$CASE" "$MESSAGE"
                    ;;
            esac
        fi
        pass "$CASE"
    done
done

CASE=kyc_absent_source_is_refused_with_the_placeholder_command
run_preflight production missing
[[ "$STATUS" == 1 \
    && "$MESSAGE" == *'/etc/verdaxis/kyc-document-keys.json'* \
    && "$MESSAGE" == *'243/CREDENTIALS'* \
    && "$MESSAGE" == *"printf '' > /etc/verdaxis/kyc-document-keys.json"* \
    && "$MESSAGE" == *'chown root:root'* && "$MESSAGE" == *0400* ]] || fail "$CASE" "$MESSAGE"
pass "$CASE"

CASE=kyc_missing_etc_verdaxis_is_refused_with_the_placeholder_command
run_preflight staging missing_parent
[[ "$STATUS" == 1 && "$MESSAGE" == *'/etc/verdaxis/kyc-document-keys-staging.json'* ]] \
    || fail "$CASE" "$MESSAGE"
pass "$CASE"

CASE=kyc_preflight_precedes_dry_run_and_mutation
installer=$(< "$INSTALLER")
before_preflight=${installer%%'# BEGIN kyc-key-source-preflight'*}
# The installer's literal text, not an expansion.
# shellcheck disable=SC2016
before_dry_run=${installer%%'if [[ "$MODE" == "dry-run" ]]'*}
before_mutation=${installer%%'PENDING_DIR="/var/lib'*}
(( ${#before_preflight} < ${#before_dry_run} && ${#before_preflight} < ${#before_mutation} )) \
    || fail "$CASE" 'the preflight must run before the dry-run exit and any mutation'
[[ "$installer" != *python* ]] || fail "$CASE" 'the installer must not run Python'
bash -n "$INSTALLER" || fail "$CASE" 'installer syntax'
pass "$CASE"

CASE=kyc_preflight_never_reads_key_contents
# Metadata only: the block names no reader of file contents. The one `cat`
# writes the help text to stderr from a here-document.
offending=$(grep -v '^ *#' <<< "$PREFLIGHT" \
    | grep -E '(^|[^a-z_])(cat|head|tail|dd|od|xxd|sha256sum|base64|jq) |(^|[^<])< *"\$' \
    | grep -v '^ *cat >&2 <<KYC_KEY_SOURCE_MISSING_HELP$' || true)
[[ -z "$offending" ]] || fail "$CASE" "reads contents: $offending"
pass "$CASE"

printf 'units-contract: %d cases passed\n' "$PASSED"

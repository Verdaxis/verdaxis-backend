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

# Use a release-shaped fixture, never the installed host or a live release.
mkdir -p "$ROOT/work"
WORK=$(mktemp -d "$ROOT/work/units-contract.XXXXXXXX")
WORK=$(cd "$WORK" && pwd -P)
trap 'rm -rf -- "$WORK"' EXIT
mkdir -p "$WORK/release/deploy/scripts" "$WORK/release/deploy/monitor" "$WORK/release/deploy/external_monitor/systemd" "$WORK/release/.runtime-rust"
cp "$ROOT/deploy/scripts/install-host-automation.sh" "$WORK/release/deploy/scripts/"
while IFS= read -r source; do
    cp "$ROOT/deploy/monitor/$source" "$WORK/release/deploy/monitor/$source"
done < <(jq -r '.artifacts[].source' "$ROOT/deploy/monitor/artifact-manifest.json")
printf '#!/bin/sh\nexit 0\n' > "$WORK/release/.runtime-rust/verdaxis-read"
BINARY_DIGEST=$(shasum -a 256 "$WORK/release/.runtime-rust/verdaxis-read")
BINARY_DIGEST=${BINARY_DIGEST%% *}
jq --arg digest "$BINARY_DIGEST" '.binary.sha256=$digest' "$ROOT/deploy/monitor/artifact-manifest.json" > "$WORK/release/deploy/monitor/artifact-manifest.json"
HOST_INSTALLER="$WORK/release/deploy/scripts/install-host-automation.sh"
# macOS has shasum; the Linux installer uses the same digest algorithm.
mkdir "$WORK/tools"
printf '#!/bin/sh\nexec shasum -a 256 "$@"\n' > "$WORK/tools/sha256sum"
chmod +x "$WORK/tools/sha256sum"
export PATH="$WORK/tools:$PATH"

installer_refuses_a_manifest_digest_mismatch() {
    local source target
    bash "$HOST_INSTALLER" --root "$WORK/untouched" --dry-run >/dev/null
    while IFS= read -r source; do
        if [[ $source == .runtime-rust/* ]]; then target="$WORK/release/$source"; else target="$WORK/release/deploy/monitor/$source"; fi
        cp "$target" "$WORK/original"
        printf x >> "$target"
        if bash "$HOST_INSTALLER" --root "$WORK/untouched" --dry-run >"$WORK/refusal.log" 2>&1; then
            fail "$CASE" "accepted changed byte: $source"
        fi
        [[ ! -e "$WORK/untouched" ]] || fail "$CASE" 'wrote before digest verification'
        cp "$WORK/original" "$target"
    done < <(jq -r '.binary.source, .artifacts[].source' "$WORK/release/deploy/monitor/artifact-manifest.json")
}
CASE=installer_refuses_a_manifest_digest_mismatch
installer_refuses_a_manifest_digest_mismatch
pass "$CASE"

installer_restart_and_revert_preserve_admitted_alert_state() {
    command -v docker >/dev/null || fail "$CASE" 'Docker is required'
    # A real previous native release is required. Task 10a did not contain
    # this installer; copying the candidate would not prove forward revert.
    if [[ -z ${VERDAXIS_PREVIOUS_HOST_RELEASE:-} || ! -f ${VERDAXIS_PREVIOUS_HOST_RELEASE}/deploy/scripts/install-host-automation.sh ]]; then
        printf 'FAIL %s: VERDAXIS_PREVIOUS_HOST_RELEASE must name a prior native release pack\n' "$CASE" >&2
        return 1
    fi
    cp -R "$VERDAXIS_PREVIOUS_HOST_RELEASE" "$WORK/previous-release"
    docker run --rm -v "$WORK:/fixture" debian:bookworm-slim /bin/bash -euo pipefail -c '
        apt-get update -qq >/dev/null
        apt-get install -y -qq systemd acl jq >/dev/null
        groupadd verdaxis-prod; useradd -g verdaxis-prod verdaxis-prod
        bash /fixture/release/deploy/scripts/install-host-automation.sh
        entry=/var/lib/verdaxis-health/alert-handoff/1.json
        cursor=/var/lib/verdaxis-health/health.sequence.json
        lock=/var/lib/verdaxis-health/.health.journal.lock
        printf "{\"schema_version\":1,\"sequence\":1,\"state\":\"failed\"}\n" > "$entry"
        chown verdaxis-health:verdaxis-health "$entry"
        setfacl -m u::rw-,u:verdaxis-monitor-alert:r--,g::---,m::r--,o::--- "$entry"
        printf "{\"sequence\":1}\n" > "$cursor"
        chown verdaxis-health:verdaxis-health "$cursor"; chmod 0600 "$cursor"
        sha256sum "$entry" "$cursor" > /tmp/state.sums
        inode=$(stat -c %i "$lock")
        bash /fixture/release/deploy/scripts/install-host-automation.sh
        bash /fixture/previous-release/deploy/scripts/install-host-automation.sh
        sha256sum --check /tmp/state.sums
        test "$(stat -c %i "$lock")" = "$inode"
        test "$(stat -c %U:%G:%a /usr/local/libexec/verdaxis/verdaxis-read)" = root:root:755
        test "$(stat -c %U:%a /var/lib/verdaxis-render)" = verdaxis-render:700
    '
}
CASE=installer_restart_and_revert_preserve_admitted_alert_state
INSTALLER_FAILURES=0
if installer_restart_and_revert_preserve_admitted_alert_state; then pass "$CASE"; else INSTALLER_FAILURES=$((INSTALLER_FAILURES + 1)); fi

installer_enforces_per_check_handoff_acl_and_identity_boundary() {
    # Install into a disposable target first; the installer checks exact ACLs.
    docker run --rm -v "$WORK:/fixture" debian:bookworm-slim /bin/bash -euo pipefail -c '
        apt-get update -qq >/dev/null
        apt-get install -y -qq systemd acl jq >/dev/null
        groupadd verdaxis-prod; useradd -g verdaxis-prod verdaxis-prod
        bash /fixture/release/deploy/scripts/install-host-automation.sh
    ' || return 1
    # Missing binary, identity, systemd or ACL support fails; never skip.
    bash "$ROOT/deploy/tests/alert-handoff-contract.sh" --disposable-identities
}
CASE=installer_enforces_per_check_handoff_acl_and_identity_boundary
if installer_enforces_per_check_handoff_acl_and_identity_boundary; then pass "$CASE"; else INSTALLER_FAILURES=$((INSTALLER_FAILURES + 1)); fi
printf 'units-contract: %d cases passed; %d installer cases failed\n' "$PASSED" "$INSTALLER_FAILURES"
(( INSTALLER_FAILURES == 0 ))

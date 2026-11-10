#!/usr/bin/env bash
# Install only. Activation and Python/native fencing belong to the operator.
set -euo pipefail
export LC_ALL=C
SOURCE_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
MANIFEST="$SOURCE_ROOT/deploy/monitor/artifact-manifest.json"
ROOT=""
DRY_RUN=0
BINARY="$SOURCE_ROOT/.runtime-rust/verdaxis-read"
BINARY_SHA256=""
DEPLOY_ENVIRONMENT=""
HOST_SCOPE="environment"
fail() { printf 'host automation: %s\n' "$*" >&2; exit 1; }
while (( $# )); do
    case $1 in
        --scope) [[ $# -ge 2 && $2 =~ ^(environment|host)$ ]] || fail 'invalid scope'; HOST_SCOPE=$2; shift 2 ;;
        --environment) [[ $# -ge 2 && -z $DEPLOY_ENVIRONMENT && $2 =~ ^(production|staging)$ ]] || fail 'invalid or repeated environment'; DEPLOY_ENVIRONMENT=$2; shift 2 ;;
        --root) [[ $# -ge 2 && -n $2 && $2 == /* ]] || fail 'root must be absolute'; ROOT=${2%/}; shift 2 ;;
        --binary) [[ $# -ge 2 ]] || fail 'binary argument missing'; BINARY=$2; shift 2 ;;
        --binary-sha256) [[ $# -ge 2 && -z $BINARY_SHA256 ]] || fail 'binary digest argument missing or repeated'; BINARY_SHA256=$2; shift 2 ;;
        --dry-run) DRY_RUN=1; shift ;;
        *) fail 'usage: install-host-automation.sh --binary-sha256 <64-hex> [--root DIR] [--dry-run] [--binary FILE]' ;;
    esac
done
# The operator copies binary_sha256 from the verified release record produced
# by verify-artifact.sh. No installed-host metadata or source placeholder is used.
[[ $BINARY_SHA256 =~ ^[a-f0-9]{64}$ ]] || fail '--binary-sha256 is required and must be 64 lowercase hex characters'
[[ "$ROOT" != *'/../'* && "$ROOT" != */.. && "$ROOT" != *'/./'* ]] || fail 'unsafe root'
for tool in jq sha256sum; do command -v "$tool" >/dev/null || fail "missing $tool"; done
# Exact source/destination pairs are an installer allowlist, not manifest authority.
allowed_destination() {
    case $1 in
        verdaxis-monitor.sysusers) printf /usr/lib/sysusers.d/verdaxis-monitor.conf ;;
        verdaxis-monitor.tmpfiles) printf /usr/lib/tmpfiles.d/verdaxis-monitor.conf ;;
        verdaxis-monitor-handoff.acl.tmpfiles) printf /usr/lib/tmpfiles.d/verdaxis-monitor-handoff-acl.conf ;;
        ../external_monitor/verdaxis-render.sysusers) printf /usr/lib/sysusers.d/verdaxis-render.conf ;;
        ../external_monitor/verdaxis-render.tmpfiles) printf /usr/lib/tmpfiles.d/verdaxis-render.conf ;;
        ../external_monitor/verdaxis-monitor.env.example) printf /usr/share/doc/verdaxis-monitor/verdaxis-monitor.env.example ;;
        alert.env.example|runtime-identity.example|runtime-v2-readiness-corpus.json)
            printf '/usr/share/doc/verdaxis-monitor/%s' "$1" ;;
        verdaxis-health.service|verdaxis-health.timer|verdaxis-backup-verify.service|verdaxis-backup-verify.timer|verdaxis-demo-activity.service|verdaxis-demo-activity.timer|verdaxis-demo-activity-staging.service|verdaxis-demo-activity-staging.timer|verdaxis-monitor-alert@.service|verdaxis-monitor-alert-reminder.service|verdaxis-monitor-alert-reminder.timer)
            printf '/etc/systemd/system/%s' "$1" ;;
        ../external_monitor/systemd/verdaxis-monitor.service|../external_monitor/systemd/verdaxis-monitor.timer|../external_monitor/systemd/verdaxis-monitor-verify.service|../external_monitor/systemd/verdaxis-onboarding-attention.service|../external_monitor/systemd/verdaxis-onboarding-attention.timer)
            printf '/etc/systemd/system/%s' "${1##*/}" ;;
        *) return 1 ;;
    esac
}
# An omitted environment is retained only for existing disposable --root tests.
[[ -n $DEPLOY_ENVIRONMENT || -n $ROOT ]] || fail '--environment is required for installed paths'
artifact_in_environment() {
    if [[ $HOST_SCOPE == environment ]]; then
        case $1 in
            verdaxis-demo-activity-staging.*) [[ $DEPLOY_ENVIRONMENT == staging ]] ;;
            verdaxis-demo-activity.*|../external_monitor/systemd/verdaxis-onboarding-attention.*) [[ $DEPLOY_ENVIRONMENT == production ]] ;;
            *) return 1 ;;
        esac
        return
    fi
    case $1 in
        verdaxis-demo-activity-staging.*) [[ $DEPLOY_ENVIRONMENT != production ]] ;;
        verdaxis-demo-activity.*|../external_monitor/systemd/verdaxis-onboarding-attention.*) [[ $DEPLOY_ENVIRONMENT != staging ]] ;;
        *) return 0 ;;
    esac
}
hash() { local digest; digest=$(sha256sum -- "$1"); printf '%s' "${digest%% *}"; }
verify() {
    [[ -f $1 && ! -L $1 ]] || fail "not a regular source: $1"
    [[ $2 =~ ^[a-f0-9]{64}$ && $(hash "$1") == "$2" ]] || fail "digest mismatch: $1"
}
# Validate everything before creating even the destination root. The source
# manifest pins policy bytes; the verified release record pins the binary.
jq -e 'keys == ["artifacts","binary","schema_version"] and .schema_version == 1
    and (.binary | keys == ["destination","mode","source"])
    and .binary.source == ".runtime-rust/verdaxis-read"
    and .binary.destination == "/usr/local/libexec/verdaxis/verdaxis-read"
    and .binary.mode == "0755"
    and (.artifacts | type == "array" and length == 25)
    and ([.artifacts[].source] | unique | length == 25)
    and ([.artifacts[].destination] | unique | length == 25)
    and all(.artifacts[]; keys == ["destination","mode","sha256","source"]
        and .mode == "0644" and (.sha256 | type == "string" and test("^[a-f0-9]{64}$")))' "$MANIFEST" >/dev/null || fail 'invalid manifest'
verify "$BINARY" "$BINARY_SHA256"
SOURCES=()
DESTINATIONS=()
MODES=()
if [[ $HOST_SCOPE == host ]]; then
    SOURCES+=("$BINARY")
    DESTINATIONS+=(/usr/local/libexec/verdaxis/verdaxis-read)
    MODES+=(0755)
    # External monitoring has a separate executable name so its selective
    # shared-host release does not replace the local backend job binary.
    SOURCES+=("$BINARY")
    DESTINATIONS+=(/usr/local/libexec/verdaxis/verdaxis-read-monitor)
    MODES+=(0755)
fi
while IFS=$'\t' read -r source destination mode digest; do
    expected=$(allowed_destination "$source") || fail "unapproved artifact: $source"
    [[ $destination == "$expected" ]] || fail "unapproved destination: $source"
    source_path="$SOURCE_ROOT/deploy/monitor/$source"
    verify "$source_path" "$digest"
    artifact_in_environment "$source" || continue
    SOURCES+=("$source_path"); DESTINATIONS+=("$destination"); MODES+=("$mode")
done < <(jq -r '.artifacts[] | [.source,.destination,.mode,.sha256] | @tsv' "$MANIFEST")
# Refuse symlink ancestors before writes, including when a --root is used.
for destination in "${DESTINATIONS[@]}" /var/lib/verdaxis-monitor /var/lib/verdaxis-render; do
    path="$ROOT$destination"
    while [[ $path != / && -n $path ]]; do
        [[ ! -L $path ]] || fail "symlink destination: $path"
        path=${path%/*}
    done
done
if (( DRY_RUN )); then
    printf 'host-automation: digest checks passed; no writes\n'
    exit 0
fi
(( EUID == 0 )) || fail 'apply requires root (use a disposable target for --root)'
for tool in systemd-sysusers systemd-tmpfiles getfacl stat; do command -v "$tool" >/dev/null || fail "missing $tool"; done
TEMP=""
trap 'if [[ -n $TEMP ]]; then rm -f -- "$TEMP"; fi' EXIT
# Snapshot each file into its destination directory and verify the copied bytes
# before rename. A changed source cannot become an unchecked installed artifact.
for index in "${!SOURCES[@]}"; do
    destination="$ROOT${DESTINATIONS[$index]}"
    install -d -o root -g root -m 0755 "${destination%/*}"
    TEMP=$(mktemp "${destination}.pending.XXXXXXXX")
    install -o root -g root -m "${MODES[$index]}" "${SOURCES[$index]}" "$TEMP"
    [[ $(hash "$TEMP") == $(hash "${SOURCES[$index]}") ]] || fail 'copy changed'
    if [[ ${DESTINATIONS[$index]} == /usr/local/libexec/verdaxis/verdaxis-read || ${DESTINATIONS[$index]} == /usr/local/libexec/verdaxis/verdaxis-read-monitor ]]; then expected=$BINARY_SHA256; else
        expected=$(jq -r --arg destination "${DESTINATIONS[$index]}" '.artifacts[] | select(.destination == $destination) | .sha256' "$MANIFEST")
    fi
    verify "$TEMP" "$expected"
    mv -f -- "$TEMP" "$destination"
    TEMP=""
done
if [[ $HOST_SCOPE == environment ]]; then
    printf 'host-automation: environment units installed; host policy unchanged; no units activated\n'
    exit 0
fi
ROOT_ARGS=()
[[ -z $ROOT ]] || ROOT_ARGS=(--root "$ROOT")
systemd-sysusers "${ROOT_ARGS[@]}" /usr/lib/sysusers.d/verdaxis-monitor.conf /usr/lib/sysusers.d/verdaxis-render.conf
# One ordered tmpfiles invocation. f (not f+) keeps stable lock inodes; no
# cleanup or age policy can remove an admitted entry, cursor or receipt.
systemd-tmpfiles "${ROOT_ARGS[@]}" --create /usr/lib/tmpfiles.d/verdaxis-monitor.conf /usr/lib/tmpfiles.d/verdaxis-monitor-handoff-acl.conf /usr/lib/tmpfiles.d/verdaxis-render.conf
# Compare the entire effective ACL, not just the presence of a named entry.
check_acl() {
    local path=$1 owner=$2 group=$3 expected=$4 actual
    [[ ! -L $ROOT$path ]] || fail 'symlink state path'
    [[ $(stat -c '%U:%G' -- "$ROOT$path") == "$owner:$group" ]] || fail "wrong state owner: $path"
    actual=$(getfacl -cp -- "$ROOT$path" | grep -v '^$')
    [[ $actual == "$expected" ]] || fail "unsafe ACL: $path"
}
for check in health backup; do
    if [[ $check == health ]]; then parent=/var/lib/verdaxis-health; owner=verdaxis-health; group=verdaxis-health; else
        parent=/var/lib/verdaxis-backup-verify; owner=verdaxis-backup-reader; group=verdaxis-backup-readers
    fi
    check_acl "$parent" "$owner" "$group" $'user::rwx\nuser:verdaxis-monitor-alert:--x\ngroup::---\nmask::--x\nother::---'
    check_acl "$parent/.$check.journal.lock" "$owner" "$group" $'user::rw-\nuser:verdaxis-monitor-alert:r--\ngroup::---\nmask::r--\nother::---'
    check_acl "$parent/alert-handoff" "$owner" "$group" $'user::rwx\nuser:verdaxis-monitor-alert:rwx\ngroup::---\nmask::rwx\nother::---\ndefault:user::rwx\ndefault:user:verdaxis-monitor-alert:r--\ndefault:group::---\ndefault:mask::r--\ndefault:other::---'
done
check_acl /var/lib/verdaxis-monitor-alert verdaxis-monitor-alert verdaxis-monitor-alert $'user::rwx\ngroup::---\nother::---'
printf 'host-automation: installed; no units activated\n'

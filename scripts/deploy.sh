#!/usr/bin/env bash
# Systemd-aware deploy helper for the Verdaxis backend.
#
# Dry-run may fetch Git objects and create private temporary files, but it does
# not change deployed source/runtime state. It never executes candidate
# application code or supplies candidate code with live secrets, home,
# database, agent, or network access.
set -euo pipefail

SCRIPT_DIR="$(cd -- "${BASH_SOURCE[0]%/*}" && pwd -P)"
BACKEND_DIR="${SCRIPT_DIR%/*}"
DRY_RUN=0

case "${1:-}" in
    "") ;;
    --dry-run) DRY_RUN=1 ;;
    *)
        echo "usage: $0 [--dry-run]" >&2
        exit 2
        ;;
esac

cd "$BACKEND_DIR"
CANONICAL_DEPLOY_LAYOUT=0

case "$BACKEND_DIR" in
    */prod/be)
        DEFAULT_BRANCH="staging"
        DEPLOY_ENVIRONMENT="production"
        SERVICE_NAME="verdaxis-backend.service"
        HEALTH_URL="https://api.verdaxis.exchange/health/ready"
        CANONICAL_DEPLOY_LAYOUT=1
        ;;
    */staging/be)
        DEFAULT_BRANCH="staging"
        DEPLOY_ENVIRONMENT="staging"
        SERVICE_NAME="verdaxis-backend-staging.service"
        HEALTH_URL="https://api-staging.verdaxis.exchange/health/ready"
        CANONICAL_DEPLOY_LAYOUT=1
        ;;
    *)
        echo "Cannot infer backend environment from path: $BACKEND_DIR" >&2
        echo "Set TARGET_BRANCH, SERVICE_NAME, and HEALTH_URL explicitly." >&2
        : "${TARGET_BRANCH:?TARGET_BRANCH is required outside prod/staging layout}"
        : "${SERVICE_NAME:?SERVICE_NAME is required outside prod/staging layout}"
        : "${HEALTH_URL:?HEALTH_URL is required outside prod/staging layout}"
        : "${DEPLOY_ENVIRONMENT:?DEPLOY_ENVIRONMENT is required outside prod/staging layout}"
        DEFAULT_BRANCH="$TARGET_BRANCH"
        ;;
esac

if [[ "$CANONICAL_DEPLOY_LAYOUT" == "1" ]]; then
    PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
    export PATH
    for GIT_VARIABLE in "${!GIT_@}"; do
        unset "$GIT_VARIABLE"
    done
    export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
    export GIT_NO_REPLACE_OBJECTS=1
    unset DATABASE_URL MIGRATOR_DATABASE_URL DATABASE_HOST DATABASE_PORT \
        DATABASE_NAME DATABASE_USER DATABASE_PASSWORD
    unset PYTHONHOME PYTHONPATH
    PYTHON_ENV=(/usr/bin/env -i PATH="$PATH")
    GIT=(/usr/bin/git -c "safe.directory=$BACKEND_DIR" -c core.hooksPath=/dev/null)
    DEPLOY_STATE_DIR="$BACKEND_DIR/.runtime-deploy"
    if [[ -n "${TARGET_BRANCH:-}" && "$TARGET_BRANCH" != "$DEFAULT_BRANCH" ]]; then
        echo "Canonical deploy checkout refuses a different target branch." >&2
        exit 2
    fi
    TARGET_BRANCH="$DEFAULT_BRANCH"
else
    export GIT_NO_REPLACE_OBJECTS=1
    GIT=(git -c "safe.directory=$BACKEND_DIR" -c core.hooksPath=/dev/null)
    DEPLOY_STATE_DIR="${DEPLOY_STATE_DIR:-$BACKEND_DIR/.runtime-deploy}"
    TARGET_BRANCH="${TARGET_BRANCH:-$DEFAULT_BRANCH}"
    PYTHON_ENV=(/usr/bin/env)
fi

case "$DEPLOY_ENVIRONMENT" in
    production|staging) ;;
    *)
        echo "DEPLOY_ENVIRONMENT must be production or staging." >&2
        exit 2
        ;;
esac
UNIT_MANIFEST_PATH="deploy/systemd/runtime-units.manifest"
MIGRATION_POLICY_PATH="deploy/migration-checkpoints.tsv"
ACL_POLICY_PATH="deploy/postgres/app_acl_policy.sql"
ACL_CONVERGENCE_SQL_PATH="deploy/postgres/converge_runtime_object_acls.sql"
ACL_CONVERGENCE_HELPER_PATH="scripts/converge_runtime_acls.py"
DEPLOY_START_GUARD_HELPER_PATH="scripts/verify_deploy_start_guard.py"
RUST_STAGING_POLICY_PATH="deploy/rust/staging-policy.json"
RUST_POLICY_PATH="deploy/rust/$DEPLOY_ENVIRONMENT-policy.json"
# Production deploys the staging tip and CI signs the artifact on the staging
# push, so both environments bind provenance to that one ref (PD-P1). The
# repository, workflow path, exact commit and attestation checks stay exact.
RUST_WORKFLOW_REF="refs/heads/staging"
RUST_PROVENANCE_KEY_PATH="deploy/rust/artifact-provenance.pub.pem"
RUST_ARTIFACT_VERIFIER_PATH="scripts/verify_rust_artifact.py"
RUST_RUNTIME_PREPARER_PATH="scripts/prepare_rust_runtime.py"
RUST_UNIT_NAME="verdaxis-rust-staging.service"
if [[ "$DEPLOY_ENVIRONMENT" == "production" ]]; then
    RUST_UNIT_NAME="verdaxis-rust.service"
fi
HOST_PROVENANCE_KEY_PATH="${HOST_PROVENANCE_KEY_PATH:-/etc/verdaxis/artifact-provenance.pub.pem}"
RUST_ARTIFACT_ARCHIVE="${RUST_ARTIFACT_ARCHIVE:-}"
RUST_PROVENANCE_RECORD="${RUST_PROVENANCE_RECORD:-}"
RUST_PROVENANCE_SIGNATURE="${RUST_PROVENANCE_SIGNATURE:-}"
HEALTH_ATTEMPTS="${HEALTH_ATTEMPTS:-60}"
HEALTH_RETRY_DELAY="${HEALTH_RETRY_DELAY:-2}"
if [[ ! "$HEALTH_ATTEMPTS" =~ ^([1-9]|[1-9][0-9]|1[01][0-9]|120)$ ]]; then
    echo "HEALTH_ATTEMPTS must be an integer from 1 to 120." >&2
    exit 2
fi
if [[ ! "$HEALTH_RETRY_DELAY" =~ ^([0-9]|[1-9][0-9]|[12][0-9]{2}|300)$ ]]; then
    echo "HEALTH_RETRY_DELAY must be an integer from 0 to 300." >&2
    exit 2
fi
RELEASE_ENV_FILE="$BACKEND_DIR/.runtime-release.env"
DEPLOY_LOCK_FILE="$DEPLOY_STATE_DIR/${DEPLOY_ENVIRONMENT}.lock"
RESTART_WINDOW_LOCK_FILE="$DEPLOY_STATE_DIR/${DEPLOY_ENVIRONMENT}.restart.lock"
DEPLOY_STATE_FILE="$DEPLOY_STATE_DIR/${DEPLOY_ENVIRONMENT}.state"
APPROVED_RELEASE_SHA="${APPROVED_RELEASE_SHA:-}"
MIGRATION_APPROVED_SOURCE_SHA="${MIGRATION_APPROVED_SOURCE_SHA:-}"
MIGRATION_EXPECTED_CURRENT_REVISION="${MIGRATION_EXPECTED_CURRENT_REVISION:-}"
MIGRATION_TARGET_REVISION="${MIGRATION_TARGET_REVISION:-}"
DEPLOY_LOCK_FD=""
RESTART_WINDOW_LOCK_FD=""
DEPLOY_STARTED=0
SERVICE_RESTART_ATTEMPTED=0
RUST_PHASE_STARTED=0
DEPLOY_PHASE="preflight"
DRY_RUN_TEMP=""
ACL_CONVERGENCE_TEMP=""
MIGRATION_POLICY_TEMP=""
RUST_POLICY_TEMP=""
RUST_POLICY_PRESENT=0
RUST_SELECTED=0
RUST_EXTRACT_DIR=""
RUST_RUNTIME_DIR="$BACKEND_DIR/.runtime-rust"

assert_clean_tree() {
    local phase="$1"
    local status
    status="$("${GIT[@]}" status --porcelain)"
    if [[ -n "$status" ]]; then
        echo "$phase changed tracked source; refusing a mismatched release identity." >&2
        "${GIT[@]}" status --short >&2
        exit 1
    fi
}

load_unit_manifest() {
    local manifest="$1"
    local manifest_environment unit_name extra
    local -A seen=()
    UNIT_NAMES=()

    while IFS=$'\t' read -r manifest_environment unit_name extra; do
        [[ -z "$manifest_environment" || "$manifest_environment" == \#* ]] && continue
        if [[ -n "$extra" \
            || ! "$manifest_environment" =~ ^(production|staging)$ \
            || ! "$unit_name" =~ ^verdaxis-[a-z0-9-]+\.(service|timer)$ \
            || ( "$manifest_environment" == "production" && "$unit_name" == *-staging.* ) \
            || ( "$manifest_environment" == "staging" && "$unit_name" != *-staging.* ) \
            || -n "${seen[$unit_name]:-}" ]]; then
            echo "Invalid or duplicate runtime unit manifest entry." >&2
            exit 1
        fi
        seen[$unit_name]=1
        if [[ "$manifest_environment" == "$DEPLOY_ENVIRONMENT" ]]; then
            UNIT_NAMES+=("$unit_name")
        fi
    done < "$manifest"
    if [[ "${#UNIT_NAMES[@]}" == 0 ]]; then
        echo "Runtime unit manifest has no entries for $DEPLOY_ENVIRONMENT." >&2
        exit 1
    fi
}

validate_migration_policy() {
    local policy="$1"
    local required_current="${2:-}" required_target="${3:-}"
    local current target extra key
    local entry_count=0 required_pair_found=0
    local -A seen=()

    while IFS=$'\t' read -r current target extra; do
        [[ -z "$current" || "$current" == \#* ]] && continue
        if [[ -n "$extra" \
            || ! "$current" =~ ^[A-Za-z0-9][A-Za-z0-9_]{0,127}$ \
            || ! "$target" =~ ^[A-Za-z0-9][A-Za-z0-9_]{0,127}$ \
            || "${current,,}" =~ ^(base|head|heads)$ \
            || "${target,,}" =~ ^(base|head|heads)$ ]]; then
            echo "Invalid migration checkpoint policy entry." >&2
            exit 1
        fi
        key="$current/$target"
        if [[ -n "${seen[$key]:-}" ]]; then
            echo "Invalid migration checkpoint policy duplicate." >&2
            exit 1
        fi
        seen[$key]=1
        entry_count=$((entry_count + 1))
        if [[ "$current" == "$required_current" \
            && "$target" == "$required_target" ]]; then
            required_pair_found=1
        fi
    done < "$policy"
    if [[ "$entry_count" == "0" ]]; then
        echo "Invalid migration checkpoint policy: no transitions." >&2
        exit 1
    fi
    if [[ -n "$required_current" && "$required_pair_found" != "1" ]]; then
        echo "Requested migration checkpoint pair is not allowlisted." >&2
        exit 1
    fi
}

verify_archived_blob() {
    local source_sha="$1"
    local relative="$2"
    local staged="$3"
    local committed_digest staged_digest

    if [[ ! -f "$staged" || -L "$staged" ]]; then
        echo "Candidate artifact must be a regular committed file: $relative" >&2
        exit 1
    fi
    committed_digest="$("${GIT[@]}" show "$source_sha:$relative" \
        | sha256sum | awk '{print $1}')"
    staged_digest="$(sha256sum -- "$staged" | awk '{print $1}')"
    if [[ "$staged_digest" != "$committed_digest" ]]; then
        echo "Archived candidate bytes differ from the committed blob: $relative" >&2
        exit 1
    fi
}

# The committed staging policy and the manifest's Rust unit are two
# independent activation records: the transaction deploys the Rust side only
# when both name it. Production reads neither file.
load_rust_staging_policy() {
    local source_sha="$1" staged="$2"

    RUST_POLICY_PRESENT=0
    local policy_path="${RUST_POLICY_PATH:-$RUST_STAGING_POLICY_PATH}"
    if [[ ! -f "$staged/$policy_path" ]]; then
        # Older trees without this environment's policy remain Python-only.
        RUST_ENABLED=0
        return 0
    fi
    RUST_POLICY_PRESENT=1
    RUST_ENABLED="$(python3 - "$staged/$policy_path" \
        "$staged/$UNIT_MANIFEST_PATH" "$RUST_UNIT_NAME" "$DEPLOY_ENVIRONMENT" <<'PY'
import json, sys
try:
    policy = json.load(open(sys.argv[1], encoding="utf-8"))
    enabled = policy.get("rust_enabled")
except (OSError, ValueError):
    sys.exit(3)
if not isinstance(enabled, bool) or policy.get("environment") != sys.argv[4]:
    sys.exit(3)
if enabled:
    manifest_lines = open(sys.argv[2], encoding="utf-8").read().splitlines()
    environment, unit = sys.argv[4], sys.argv[3]
    if not any(
        line == f"{environment}\t{unit}" for line in manifest_lines
    ):
        sys.exit(4)
print("1" if enabled else "0")
PY
)" || { echo "Invalid Rust staging policy or Rust unit is not manifest-selected." >&2; exit 1; }
}

# Resolve which provenance public key to pass to the verifier. The reviewed
# tree's digest is authoritative: an operator-installed host key is accepted
# only when its SHA-256 equals that reviewed digest, and is then preferred
# because it is root-owned. Anything else refuses before a service stops.
resolve_provenance_public_key() {
    local source_sha="$1"
    local tree_key="$BACKEND_DIR/$RUST_PROVENANCE_KEY_PATH"
    local expected_digest host_digest

    expected_digest="$("${GIT[@]}" show "$source_sha:$RUST_PROVENANCE_KEY_PATH" \
        | sha256sum | awk '{print $1}')"
    if [[ -e "$HOST_PROVENANCE_KEY_PATH" ]]; then
        if [[ -L "$HOST_PROVENANCE_KEY_PATH" || ! -f "$HOST_PROVENANCE_KEY_PATH" ]]; then
            echo "The host provenance public key must be a regular file." >&2
            exit 1
        fi
        host_digest="$(sha256sum -- "$HOST_PROVENANCE_KEY_PATH" | awk '{print $1}')"
        if [[ "$host_digest" != "$expected_digest" ]]; then
            echo "The host provenance public key digest differs from the reviewed tree; refusing before any service stops." >&2
            exit 1
        fi
        PROVENANCE_PUBLIC_KEY_PATH="$HOST_PROVENANCE_KEY_PATH"
    else
        if [[ -L "$tree_key" || ! -f "$tree_key" ]]; then
            echo "The reviewed provenance public key is missing from the selected tree." >&2
            exit 1
        fi
        if [[ "$(sha256sum -- "$tree_key" | awk '{print $1}')" != "$expected_digest" ]]; then
            echo "The selected tree's provenance public key differs from its committed blob." >&2
            exit 1
        fi
        PROVENANCE_PUBLIC_KEY_PATH="$tree_key"
    fi
}

# Verify the separately approved Rust artifact against its provenance record
# before any lock is acquired and any service stops. Every refusal here is a
# pre-mutation refusal.
verify_rust_artifact_for_release() {
    local source_sha="$1"
    local extract_dir="$2"

    if [[ ! -f "$RUST_ARTIFACT_ARCHIVE" || -L "$RUST_ARTIFACT_ARCHIVE" ]]; then
        echo "RUST_ARTIFACT_ARCHIVE must name a regular release archive file." >&2
        exit 1
    fi
    if [[ -z "$RUST_PROVENANCE_RECORD" || -z "$RUST_PROVENANCE_SIGNATURE" ]]; then
        echo "The Rust provenance record and signature are both required." >&2
        exit 1
    fi
    for input in "$RUST_PROVENANCE_RECORD" "$RUST_PROVENANCE_SIGNATURE"; do
        if [[ ! -f "$input" || -L "$input" ]]; then
            echo "The Rust provenance record and signature must be regular files." >&2
            exit 1
        fi
    done
    resolve_provenance_public_key "$source_sha"
    echo "+ verify the separately approved Rust artifact"
    if ! "${PYTHON_ENV[@]}" ./venv/bin/python "$RUST_ARTIFACT_VERIFIER_PATH" \
        --archive "$RUST_ARTIFACT_ARCHIVE" \
        --source-sha "$source_sha" \
        --workflow-ref "$RUST_WORKFLOW_REF" \
        --target x86_64-unknown-linux-gnu \
        --output-dir "$extract_dir" \
        --provenance-record "$RUST_PROVENANCE_RECORD" \
        --provenance-signature "$RUST_PROVENANCE_SIGNATURE" \
        --provenance-public-key "$PROVENANCE_PUBLIC_KEY_PATH"; then
        echo "Rust artifact verification refused; no service was stopped." >&2
        exit 1
    fi
}

cleanup_rust_extract() {
    if [[ -n "$RUST_EXTRACT_DIR" ]]; then
        case "$RUST_EXTRACT_DIR" in
            "$RUST_RUNTIME_DIR"/releases/*)
                rm -rf -- "$RUST_EXTRACT_DIR"
                RUST_EXTRACT_DIR=""
                ;;
            *)
                echo "WARNING: refusing unexpected Rust extract cleanup path." >&2
                ;;
        esac
    fi
    return 0
}

assert_release_tree_regular() {
    local source_sha="$1"
    local tree

    if ! tree="$("${GIT[@]}" ls-tree -r "$source_sha")"; then
        echo "Unable to inspect every selected release-tree entry." >&2
        exit 1
    fi
    if awk '$1 == "120000" || $1 == "160000" { found=1 } END { exit found ? 0 : 1 }' \
        <<< "$tree"; then
        echo "Candidate release tree entries must be regular committed files; symlinks and gitlinks are refused." >&2
        exit 1
    fi
}

assert_secure_deploy_state_dir() {
    local current_uid directory_mode directory_owner

    if [[ -L "$DEPLOY_STATE_DIR" || ! -d "$DEPLOY_STATE_DIR" ]]; then
        echo "Deployment state directory must be a real directory." >&2
        exit 1
    fi
    current_uid="$(id -u)"
    directory_owner="$(stat -Lc "%u" -- "$DEPLOY_STATE_DIR")"
    directory_mode="$(stat -Lc "%a" -- "$DEPLOY_STATE_DIR")"
    if [[ "$directory_owner" != "$current_uid" ]] \
        || (( (8#$directory_mode & 8#022) != 0 )); then
        echo "Deployment state directory must be owner-controlled." >&2
        exit 1
    fi
}

validate_open_lock_file() {
    local lock_path="$1" lock_fd="$2"
    local current_uid descriptor_identity path_identity

    current_uid="$(id -u)"
    if [[ -L "$lock_path" || ! -f "$lock_path" \
        || ! -f "/proc/self/fd/$lock_fd" ]]; then
        echo "Deployment lock must be a regular file: $lock_path" >&2
        exit 1
    fi
    descriptor_identity="$(stat -Lc "%d:%i:%u:%h" -- "/proc/self/fd/$lock_fd")"
    path_identity="$(stat -Lc "%d:%i:%u:%h" -- "$lock_path")"
    if [[ "$descriptor_identity" != "$path_identity" \
        || "$descriptor_identity" != *":$current_uid:1" ]]; then
        echo "Deployment lock descriptor does not match its owner-controlled path." >&2
        exit 1
    fi
    chmod 0600 "/proc/self/fd/$lock_fd"
    descriptor_identity="$(stat -Lc "%d:%i:%u:%a:%h" -- "/proc/self/fd/$lock_fd")"
    path_identity="$(stat -Lc "%d:%i:%u:%a:%h" -- "$lock_path")"
    if [[ "$descriptor_identity" != "$path_identity" \
        || "$descriptor_identity" != *":$current_uid:600:1" ]]; then
        echo "Deployment lock mode could not be restricted safely." >&2
        exit 1
    fi
}

acquire_deploy_lock() {
    local previous_umask

    previous_umask="$(umask)"
    if [[ ! -e "$DEPLOY_STATE_DIR" && ! -L "$DEPLOY_STATE_DIR" ]]; then
        umask 022
        mkdir -m 0755 -- "$DEPLOY_STATE_DIR"
        umask "$previous_umask"
    fi
    assert_secure_deploy_state_dir
    if [[ -L "$DEPLOY_LOCK_FILE" \
        || ( -e "$DEPLOY_LOCK_FILE" && ! -f "$DEPLOY_LOCK_FILE" ) ]]; then
        echo "Deployment lock path is not a regular file." >&2
        exit 1
    fi
    umask 077
    exec {DEPLOY_LOCK_FD}>>"$DEPLOY_LOCK_FILE"
    umask "$previous_umask"
    validate_open_lock_file "$DEPLOY_LOCK_FILE" "$DEPLOY_LOCK_FD"
    if ! flock -n "$DEPLOY_LOCK_FD"; then
        echo "A $DEPLOY_ENVIRONMENT deployment is already running; refusing concurrent deploy." >&2
        exit 1
    fi
}

acquire_restart_window_lock() {
    local previous_umask

    if [[ -L "$RESTART_WINDOW_LOCK_FILE" \
        || ( -e "$RESTART_WINDOW_LOCK_FILE" && ! -f "$RESTART_WINDOW_LOCK_FILE" ) ]]; then
        echo "Restart-window lock path is not a regular file." >&2
        exit 1
    fi
    previous_umask="$(umask)"
    umask 077
    exec {RESTART_WINDOW_LOCK_FD}>>"$RESTART_WINDOW_LOCK_FILE"
    umask "$previous_umask"
    validate_open_lock_file "$RESTART_WINDOW_LOCK_FILE" "$RESTART_WINDOW_LOCK_FD"
    if ! flock -n "$RESTART_WINDOW_LOCK_FD"; then
        echo "Another process owns the restart window; deployment remains blocked." >&2
        exit 1
    fi
}

write_monitor_identity() {
    local release_sha="$1"
    local identity_file="$BACKEND_DIR/.runtime-monitor.env"
    local temporary_file="${identity_file}.tmp.$$"

    printf 'ENVIRONMENT=%s\nRELEASE_SHA=%s\n' \
        "$DEPLOY_ENVIRONMENT" "$release_sha" > "$temporary_file"
    chmod 0644 "$temporary_file"
    mv -- "$temporary_file" "$identity_file"
}

write_release_artifact() {
    local release_sha="$1"
    local migration_revision="$2"
    local temporary_file="${RELEASE_ENV_FILE}.tmp.$$"

    echo "+ write immutable release metadata to $RELEASE_ENV_FILE"
    umask 077
    printf 'ENVIRONMENT=%s\nRELEASE_SHA=%s\nMIGRATION_REVISION=%s\n' \
        "$DEPLOY_ENVIRONMENT" "$release_sha" "$migration_revision" \
        > "$temporary_file"
    chmod 0600 "$temporary_file"
    mv -- "$temporary_file" "$RELEASE_ENV_FILE"
    # Both identities publish under durable blocked state, before candidate code
    # or a restart. A failed second replacement leaves starts blocked.
    write_monitor_identity "$release_sha"
}

write_deploy_state() {
    local phase="$1"
    local release_sha="${2:-${CURRENT_SHA:-$APPROVED_RELEASE_SHA}}"
    local previous_umask temporary_file

    previous_umask="$(umask)"
    umask 077
    temporary_file="$(mktemp --tmpdir="$DEPLOY_STATE_DIR" \
        ".${DEPLOY_ENVIRONMENT}.state.XXXXXXXX")"
    umask "$previous_umask"
    if ! printf 'DEPLOYMENT_STATE=%s\nENVIRONMENT=%s\nRELEASE_SHA=%s\n' \
        "$phase" "$DEPLOY_ENVIRONMENT" "$release_sha" > "$temporary_file"; then
        rm -f -- "$temporary_file"
        return 1
    fi
    chmod 0644 "$temporary_file"
    mv -- "$temporary_file" "$DEPLOY_STATE_FILE"
}

clear_deploy_state() {
    rm -f -- "$DEPLOY_STATE_FILE"
}

cleanup_dry_run_candidate() {
    if [[ -n "$DRY_RUN_TEMP" ]]; then
        case "$DRY_RUN_TEMP" in
            /tmp/verdaxis-deploy-dry-run.*)
                rm -rf -- "$DRY_RUN_TEMP"
                DRY_RUN_TEMP=""
                ;;
            *)
                echo "WARNING: refusing unexpected dry-run cleanup path." >&2
                ;;
        esac
    fi
    return 0
}

cleanup_acl_convergence_bundle() {
    if [[ -n "$ACL_CONVERGENCE_TEMP" ]]; then
        case "$ACL_CONVERGENCE_TEMP" in
            /tmp/verdaxis-runtime-acl.*)
                rm -rf -- "$ACL_CONVERGENCE_TEMP"
                ACL_CONVERGENCE_TEMP=""
                ;;
            *)
                echo "WARNING: refusing unexpected ACL bundle cleanup path." >&2
                ;;
        esac
    fi
    return 0
}

cleanup_migration_policy_bundle() {
    if [[ -n "$MIGRATION_POLICY_TEMP" ]]; then
        case "$MIGRATION_POLICY_TEMP" in
            /tmp/verdaxis-migration-policy.*)
                rm -rf -- "$MIGRATION_POLICY_TEMP"
                MIGRATION_POLICY_TEMP=""
                ;;
            *)
                echo "WARNING: refusing unexpected migration policy cleanup path." >&2
                ;;
        esac
    fi
    return 0
}

cleanup_rust_policy_bundle() {
    if [[ -n "$RUST_POLICY_TEMP" ]]; then
        case "$RUST_POLICY_TEMP" in
            /tmp/verdaxis-rust-policy.*)
                rm -rf -- "$RUST_POLICY_TEMP"
                RUST_POLICY_TEMP=""
                ;;
            *)
                echo "WARNING: refusing unexpected Rust policy cleanup path." >&2
                ;;
        esac
    fi
    return 0
}

prepare_migration_policy_bundle() {
    local source_sha="$1"

    umask 077
    MIGRATION_POLICY_TEMP="$(mktemp -d /tmp/verdaxis-migration-policy.XXXXXXXX)"
    chmod 0700 "$MIGRATION_POLICY_TEMP"
    "${GIT[@]}" cat-file -e "$source_sha:$MIGRATION_POLICY_PATH"
    "${GIT[@]}" archive --format=tar "$source_sha" "$MIGRATION_POLICY_PATH" \
        | tar --extract --file=- --directory="$MIGRATION_POLICY_TEMP"
    verify_archived_blob "$source_sha" "$MIGRATION_POLICY_PATH" \
        "$MIGRATION_POLICY_TEMP/$MIGRATION_POLICY_PATH"
}

prepare_acl_convergence_bundle() {
    local source_sha="$1" relative
    local -a paths=(
        "$ACL_POLICY_PATH"
        "$ACL_CONVERGENCE_SQL_PATH"
        "$ACL_CONVERGENCE_HELPER_PATH"
    )

    umask 077
    ACL_CONVERGENCE_TEMP="$(mktemp -d /tmp/verdaxis-runtime-acl.XXXXXXXX)"
    chmod 0700 "$ACL_CONVERGENCE_TEMP"
    for relative in "${paths[@]}"; do
        "${GIT[@]}" cat-file -e "$source_sha:$relative"
    done
    "${GIT[@]}" archive --format=tar "$source_sha" "${paths[@]}" \
        | tar --extract --file=- --directory="$ACL_CONVERGENCE_TEMP"
    for relative in "${paths[@]}"; do
        verify_archived_blob "$source_sha" "$relative" \
            "$ACL_CONVERGENCE_TEMP/$relative"
    done
}

preserve_failed_release() {
    local status="$?"
    trap - EXIT
    cleanup_dry_run_candidate
    cleanup_acl_convergence_bundle
    cleanup_migration_policy_bundle
    cleanup_rust_policy_bundle
    cleanup_rust_extract
    if [[ "$status" != "0" && "$DRY_RUN" == "0" && "$DEPLOY_STARTED" == "1" ]]; then
        if [[ "$RUST_PHASE_STARTED" == "1" ]] \
            && systemctl is-active --quiet "$SERVICE_NAME" \
            && HEALTH_PAYLOAD="$(curl --fail --silent --show-error --max-time 15 "$HEALTH_URL")" \
            && printf '%s' "$HEALTH_PAYLOAD" | "${PYTHON_ENV[@]}" ./venv/bin/python \
                scripts/validate_health_response.py --environment "$DEPLOY_ENVIRONMENT" \
                --release-sha "$CURRENT_SHA"; then
            write_deploy_state "rust-failed" "$CURRENT_SHA" || \
                echo "WARNING: could not record Rust failure state." >&2
            sudo -n systemctl stop "$RUST_UNIT_NAME" || echo "WARNING: Rust stop failed." >&2
            sudo -n systemctl disable "$RUST_UNIT_NAME" || echo "WARNING: Rust disable failed." >&2
            echo "Rust activation failed; exact-ready Python remains serving. Canonical retry required." >&2
            exit "$status"
        fi
        if ! write_deploy_state "blocked" "${CURRENT_SHA:-$APPROVED_RELEASE_SHA}"; then
            echo "WARNING: could not restore durable deployment state." >&2
        fi
        if [[ "$SERVICE_RESTART_ATTEMPTED" == "1" ]]; then
            if ! sudo -n systemctl stop "$SERVICE_NAME"; then
                echo "WARNING: could not stop the failed backend service." >&2
            fi
            if [[ "$RUST_SELECTED" == "1" ]] && ! sudo -n systemctl stop "$RUST_UNIT_NAME"; then
                echo "WARNING: could not stop the failed Rust service." >&2
            fi
        fi
        echo "Deployment failed; code and published identity remain aligned and the durable state stays fail-closed." >&2
    fi
    exit "$status"
}

trap preserve_failed_release EXIT

echo "=== Verdaxis Backend Deployment ==="
echo "Timestamp: $(date)"
echo "Directory: $BACKEND_DIR"
echo "Current branch: $("${GIT[@]}" branch --show-current)"
echo "Current SHA: $("${GIT[@]}" rev-parse --short HEAD)"
echo "Target branch: $TARGET_BRANCH"
echo "Service: $SERVICE_NAME"
echo "Health URL: $HEALTH_URL"
echo "Dry run: $DRY_RUN"

if [[ "$DRY_RUN" == "1" ]]; then
    CURRENT_BRANCH="$("${GIT[@]}" branch --show-current)"
    if [[ "$CURRENT_BRANCH" != "$TARGET_BRANCH" ]]; then
        echo "Dry-run requires the checkout to be on target branch $TARGET_BRANCH." >&2
        exit 1
    fi
    assert_clean_tree "Dry-run preflight"
    if [[ -e "$DEPLOY_STATE_FILE" ]]; then
        echo "Durable deployment state is present; recover the interrupted deploy before dry-run." >&2
        exit 1
    fi

    mapfile -t REMOTE_REFS < <(
        "${GIT[@]}" ls-remote --exit-code origin "refs/heads/$TARGET_BRANCH"
    )
    if [[ "${#REMOTE_REFS[@]}" != "1" ]]; then
        echo "Remote source check did not resolve exactly one target branch." >&2
        exit 1
    fi
    read -r REMOTE_SHA REMOTE_REF <<< "${REMOTE_REFS[0]}"
    if [[ ! "$REMOTE_SHA" =~ ^[0-9a-f]{40}$ \
        || "$REMOTE_REF" != "refs/heads/$TARGET_BRANCH" ]]; then
        echo "Dry-run could not attest the remote target's full commit SHA." >&2
        exit 1
    fi

    umask 077
    DRY_RUN_TEMP="$(mktemp -d /tmp/verdaxis-deploy-dry-run.XXXXXXXX)"
    chmod 0700 "$DRY_RUN_TEMP"
    "${GIT[@]}" fetch --quiet --no-tags origin "$REMOTE_SHA"
    "${GIT[@]}" cat-file -e "$REMOTE_SHA^{commit}"
    assert_release_tree_regular "$REMOTE_SHA"
    mkdir -m 0700 "$DRY_RUN_TEMP/candidate-units"
    RUST_ATTEST_PATHS=()
    if "${GIT[@]}" cat-file -e "$REMOTE_SHA:$RUST_POLICY_PATH" 2>/dev/null; then
        "${GIT[@]}" cat-file -e "$REMOTE_SHA:$RUST_PROVENANCE_KEY_PATH"
        RUST_ATTEST_PATHS+=("$RUST_POLICY_PATH" "$RUST_PROVENANCE_KEY_PATH")
    fi
    "${GIT[@]}" cat-file -e "$REMOTE_SHA:$UNIT_MANIFEST_PATH"
    "${GIT[@]}" cat-file -e "$REMOTE_SHA:$MIGRATION_POLICY_PATH"
    "${GIT[@]}" cat-file -e "$REMOTE_SHA:$ACL_POLICY_PATH"
    "${GIT[@]}" cat-file -e "$REMOTE_SHA:$ACL_CONVERGENCE_SQL_PATH"
    "${GIT[@]}" cat-file -e "$REMOTE_SHA:$ACL_CONVERGENCE_HELPER_PATH"
    "${GIT[@]}" cat-file -e "$REMOTE_SHA:$DEPLOY_START_GUARD_HELPER_PATH"
    "${GIT[@]}" archive --format=tar "$REMOTE_SHA" \
        "$UNIT_MANIFEST_PATH" "$MIGRATION_POLICY_PATH" "$ACL_POLICY_PATH" \
        "$ACL_CONVERGENCE_SQL_PATH" "$ACL_CONVERGENCE_HELPER_PATH" \
        "$DEPLOY_START_GUARD_HELPER_PATH" "${RUST_ATTEST_PATHS[@]}" \
        | tar --extract --file=- --directory="$DRY_RUN_TEMP/candidate-units"
    verify_archived_blob "$REMOTE_SHA" "$UNIT_MANIFEST_PATH" \
        "$DRY_RUN_TEMP/candidate-units/$UNIT_MANIFEST_PATH"
    verify_archived_blob "$REMOTE_SHA" "$MIGRATION_POLICY_PATH" \
        "$DRY_RUN_TEMP/candidate-units/$MIGRATION_POLICY_PATH"
    verify_archived_blob "$REMOTE_SHA" "$ACL_POLICY_PATH" \
        "$DRY_RUN_TEMP/candidate-units/$ACL_POLICY_PATH"
    verify_archived_blob "$REMOTE_SHA" "$ACL_CONVERGENCE_SQL_PATH" \
        "$DRY_RUN_TEMP/candidate-units/$ACL_CONVERGENCE_SQL_PATH"
    verify_archived_blob "$REMOTE_SHA" "$ACL_CONVERGENCE_HELPER_PATH" \
        "$DRY_RUN_TEMP/candidate-units/$ACL_CONVERGENCE_HELPER_PATH"
    verify_archived_blob "$REMOTE_SHA" "$DEPLOY_START_GUARD_HELPER_PATH" \
        "$DRY_RUN_TEMP/candidate-units/$DEPLOY_START_GUARD_HELPER_PATH"
    for relative in "${RUST_ATTEST_PATHS[@]}"; do
        verify_archived_blob "$REMOTE_SHA" "$relative" \
            "$DRY_RUN_TEMP/candidate-units/$relative"
    done
    load_unit_manifest "$DRY_RUN_TEMP/candidate-units/$UNIT_MANIFEST_PATH"
    load_rust_staging_policy "$REMOTE_SHA" "$DRY_RUN_TEMP/candidate-units"
    validate_migration_policy \
        "$DRY_RUN_TEMP/candidate-units/$MIGRATION_POLICY_PATH"
    ARCHIVE_PATHS=(
        "$UNIT_MANIFEST_PATH"
        "$MIGRATION_POLICY_PATH"
        "$ACL_POLICY_PATH"
        "$ACL_CONVERGENCE_SQL_PATH"
        "$ACL_CONVERGENCE_HELPER_PATH"
    )
    CANDIDATE_UNITS=()
    for unit_name in "${UNIT_NAMES[@]}"; do
        relative="deploy/systemd/$unit_name"
        "${GIT[@]}" cat-file -e "$REMOTE_SHA:$relative"
        ARCHIVE_PATHS+=("$relative")
        CANDIDATE_UNITS+=("$DRY_RUN_TEMP/candidate-units/$relative")
    done
    "${GIT[@]}" archive --format=tar "$REMOTE_SHA" "${ARCHIVE_PATHS[@]}" \
        | tar --extract --file=- --directory="$DRY_RUN_TEMP/candidate-units"
    for relative in "${ARCHIVE_PATHS[@]}"; do
        verify_archived_blob "$REMOTE_SHA" "$relative" \
            "$DRY_RUN_TEMP/candidate-units/$relative"
    done
    systemd-analyze verify "${CANDIDATE_UNITS[@]}"
    assert_clean_tree "Dry-run checks"
    cleanup_dry_run_candidate

    echo "Dry-run checks passed for immutable candidate SHA $REMOTE_SHA."
    echo "APPROVED_RELEASE_SHA=$REMOTE_SHA"
    echo "Migration remains unapproved until the operator also sets:"
    echo "  MIGRATION_APPROVED_SOURCE_SHA=$REMOTE_SHA"
    echo "  MIGRATION_EXPECTED_CURRENT_REVISION=<exact-live-revision>"
    echo "  MIGRATION_TARGET_REVISION=<allowlisted-checkpoint>"
    if [[ "$RUST_POLICY_PRESENT" == "1" ]]; then
        echo "Rust staging policy rust_enabled=$RUST_ENABLED; a Rust-enabled deploy additionally requires RUST_ARTIFACT_ARCHIVE, RUST_PROVENANCE_RECORD and RUST_PROVENANCE_SIGNATURE."
        echo "Artifact runtime compatibility remains untested: no Cargo, binary, or candidate Python was executed."
    fi
    echo "Candidate Python code was not executed; live secrets, home, database, and network were not supplied to candidate code."
    echo "Skipped mutations:"
    echo "  - source update (the exact SHA was inspected from an immutable Git archive)"
    echo "  - candidate application preflight (omitted; this dry-run does not execute candidate code)"
    echo "  - dependency installation and resolver/build metadata"
    echo "  - migration upgrade or live database inspection"
    echo "  - release identity and deployment state publication"
    echo "  - service restart"
    echo "  - health gate (valid only after the candidate service restarts)"
    exit 0
fi

assert_clean_tree "Initial deploy preflight"
if [[ ! "$APPROVED_RELEASE_SHA" =~ ^[0-9a-f]{40}$ ]]; then
    echo "APPROVED_RELEASE_SHA must be the exact full SHA printed by a prior dry-run." >&2
    exit 1
fi
if [[ "$MIGRATION_APPROVED_SOURCE_SHA" != "$APPROVED_RELEASE_SHA" ]]; then
    echo "MIGRATION_APPROVED_SOURCE_SHA must equal the exact approved release SHA." >&2
    exit 1
fi
if [[ ! "$MIGRATION_EXPECTED_CURRENT_REVISION" =~ ^[A-Za-z0-9][A-Za-z0-9_]{0,127}$ \
    || ! "$MIGRATION_TARGET_REVISION" =~ ^[A-Za-z0-9][A-Za-z0-9_]{0,127}$ \
    || "${MIGRATION_EXPECTED_CURRENT_REVISION,,}" =~ ^(base|head|heads)$ \
    || "${MIGRATION_TARGET_REVISION,,}" =~ ^(base|head|heads)$ ]]; then
    echo "Exact literal migration current and target revisions are required." >&2
    exit 1
fi

CURRENT_BRANCH="$("${GIT[@]}" branch --show-current)"
if [[ "$CURRENT_BRANCH" != "$TARGET_BRANCH" ]]; then
    echo "Deploy requires the checkout to be on target branch $TARGET_BRANCH." >&2
    exit 1
fi
mapfile -t REMOTE_REFS < <(
    "${GIT[@]}" ls-remote --exit-code origin "refs/heads/$TARGET_BRANCH"
)
if [[ "${#REMOTE_REFS[@]}" != "1" ]]; then
    echo "Remote source check did not resolve exactly one target branch." >&2
    exit 1
fi
read -r REMOTE_SHA REMOTE_REF <<< "${REMOTE_REFS[0]}"
if [[ ! "$REMOTE_SHA" =~ ^[0-9a-f]{40}$ \
    || "$REMOTE_REF" != "refs/heads/$TARGET_BRANCH" \
    || "$REMOTE_SHA" != "$APPROVED_RELEASE_SHA" ]]; then
    echo "Remote target moved after approval; refusing before source mutation." >&2
    exit 1
fi
"${GIT[@]}" fetch --quiet --no-tags origin "$REMOTE_SHA"
"${GIT[@]}" cat-file -e "$REMOTE_SHA^{commit}"
assert_release_tree_regular "$REMOTE_SHA"
prepare_migration_policy_bundle "$REMOTE_SHA"
validate_migration_policy \
    "$MIGRATION_POLICY_TEMP/$MIGRATION_POLICY_PATH" \
    "$MIGRATION_EXPECTED_CURRENT_REVISION" "$MIGRATION_TARGET_REVISION"

# Rust selection is read from the archived policy before the deploy lock. The
# artifact itself is verified under the main lock (the extraction directory is
# per-SHA), so a re-deploy of the same release replaces a stale staging and a
# concurrent deploy still serializes. Every artifact refusal happens before
# any state write or service stop.
RUST_ENABLED=0
if "${GIT[@]}" cat-file -e "$REMOTE_SHA:$RUST_POLICY_PATH" 2>/dev/null; then
    umask 077
    RUST_POLICY_TEMP="$(mktemp -d /tmp/verdaxis-rust-policy.XXXXXXXX)"
    chmod 0700 "$RUST_POLICY_TEMP"
    "${GIT[@]}" archive --format=tar "$REMOTE_SHA" \
        "$RUST_POLICY_PATH" "$UNIT_MANIFEST_PATH" "$RUST_PROVENANCE_KEY_PATH" \
        | tar --extract --file=- --directory="$RUST_POLICY_TEMP"
    for relative in "$RUST_POLICY_PATH" "$UNIT_MANIFEST_PATH" "$RUST_PROVENANCE_KEY_PATH"; do
        verify_archived_blob "$REMOTE_SHA" "$relative" \
            "$RUST_POLICY_TEMP/$relative"
    done
    load_rust_staging_policy "$REMOTE_SHA" "$RUST_POLICY_TEMP"
fi

# A first activation must not reach a joint restart before the root-owned
# unit has been installed from the prior, policy-off bootstrap release.
if [[ "$RUST_ENABLED" == "1" ]]; then
    if [[ "$(systemctl show --property=LoadState --value "$RUST_UNIT_NAME")" != "loaded" ]]; then
        echo "Rust unit is not loaded; install the reviewed bootstrap unit before activation." >&2
        exit 1
    fi
fi

# Prove every exact privileged service command before state/source mutation.
# Noninteractive execution must never discover missing authority at restart.
for action in restart stop; do
    sudo -n -l systemctl "$action" "$SERVICE_NAME" >/dev/null || {
        echo "Required backend service authority is unavailable." >&2; exit 1;
    }
done
if [[ "$RUST_ENABLED" == "1" ]]; then
    for action in restart stop disable enable; do
        sudo -n -l systemctl "$action" "$RUST_UNIT_NAME" >/dev/null || {
            echo "Required Rust service authority is unavailable." >&2; exit 1;
        }
    done
fi

acquire_deploy_lock
# The artifact is verified and extracted under the main lock, before any state
# write or service stop. A stale staging for the same SHA (a retried deploy)
# is replaced; concurrent deploys were already refused by the lock.
if [[ "$RUST_ENABLED" == "1" ]]; then
    RUST_EXTRACT_DIR="$RUST_RUNTIME_DIR/releases/$REMOTE_SHA"
    if [[ -e "$RUST_EXTRACT_DIR" ]]; then
        case "$RUST_EXTRACT_DIR" in
            "$RUST_RUNTIME_DIR"/releases/[0-9a-f]*)
                rm -rf -- "$RUST_EXTRACT_DIR"
                ;;
            *)
                echo "Refusing an unexpected staged release path." >&2
                exit 1
                ;;
        esac
    fi
    verify_rust_artifact_for_release "$REMOTE_SHA" "$RUST_EXTRACT_DIR"
    RUST_SELECTED=1
    # The verified policy bytes travel with the deploy so the health URL
    # cannot drift between selection and readiness.
    umask 077
    mkdir -p -- "$RUST_RUNTIME_DIR"
    if [[ -L "$RUST_RUNTIME_DIR/$DEPLOY_ENVIRONMENT-policy.json" ]] \
        || ! cp -- "$RUST_POLICY_TEMP/$RUST_POLICY_PATH" \
            "$RUST_RUNTIME_DIR/$DEPLOY_ENVIRONMENT-policy.json.tmp.$$" \
        || ! mv -- "$RUST_RUNTIME_DIR/$DEPLOY_ENVIRONMENT-policy.json.tmp.$$" \
            "$RUST_RUNTIME_DIR/$DEPLOY_ENVIRONMENT-policy.json"; then
        echo "Could not stage the verified Rust policy bytes." >&2
        exit 1
    fi
fi
CURRENT_SHA="$("${GIT[@]}" rev-parse HEAD)"
"${PYTHON_ENV[@]}" ENVIRONMENT="$DEPLOY_ENVIRONMENT" RELEASE_SHA="$CURRENT_SHA" \
    ./venv/bin/python scripts/verify_migration_revision.py \
    --expected "$MIGRATION_EXPECTED_CURRENT_REVISION"
cleanup_migration_policy_bundle
cleanup_rust_policy_bundle

DEPLOY_STARTED=1
write_deploy_state "blocked" "$APPROVED_RELEASE_SHA"

"${GIT[@]}" fetch origin "refs/heads/$TARGET_BRANCH"
REMOTE_SHA="$("${GIT[@]}" rev-parse 'FETCH_HEAD^{commit}')"
if [[ "$REMOTE_SHA" != "$APPROVED_RELEASE_SHA" ]]; then
    echo "Remote target moved after approval; refusing to mutate source." >&2
    exit 1
fi
"${GIT[@]}" checkout "$TARGET_BRANCH"
"${GIT[@]}" merge --ff-only FETCH_HEAD

CURRENT_BRANCH="$("${GIT[@]}" branch --show-current)"
CURRENT_SHA="$("${GIT[@]}" rev-parse HEAD)"
if [[ "$MIGRATION_APPROVED_SOURCE_SHA" != "$CURRENT_SHA" ]]; then
    echo "Migration approval is not bound to the selected source SHA." >&2
    exit 1
fi
if [[ "$CURRENT_BRANCH" != "$TARGET_BRANCH" \
    || ! "$CURRENT_SHA" =~ ^[0-9a-f]{40}$ \
    || "$CURRENT_SHA" != "$REMOTE_SHA" ]]; then
    echo "Selected source is not the exact full-SHA remote target release." >&2
    exit 1
fi
assert_clean_tree "Selected release"
assert_release_tree_regular "$CURRENT_SHA"
prepare_acl_convergence_bundle "$CURRENT_SHA"

# Publish the selected tree's exact identity before invoking any executable
# from that tree. The durable state lets units remain fail-closed during this
# transition and through any restart/readiness failure.
write_release_artifact "$CURRENT_SHA" "$MIGRATION_TARGET_REVISION"

"${PYTHON_ENV[@]}" ENVIRONMENT="$DEPLOY_ENVIRONMENT" RELEASE_SHA="$CURRENT_SHA" \
    ./venv/bin/python scripts/preflight_runtime.py \
    --environment "$DEPLOY_ENVIRONMENT" --release-sha "$CURRENT_SHA"

if [[ -f requirements.txt ]]; then
    if [[ ! -f constraints.txt ]]; then
        echo "constraints.txt is required for reproducible deployed dependency installation." >&2
        exit 1
    fi
    umask 022
    "${PYTHON_ENV[@]}" PIP_CONFIG_FILE=/dev/null \
        ./venv/bin/python -m pip install -r requirements.txt -c constraints.txt
    "${PYTHON_ENV[@]}" PIP_CONFIG_FILE=/dev/null \
        ./venv/bin/python -m pip check
    find venv -type d ! -perm -0005 -exec chmod o+rx {} +
    find venv -type f ! -perm -0004 -exec chmod o+r {} +
fi

# The effective Task 2 connection budget is prepared whenever a Rust unit is
# selected, before any schema change; a refused budget leaves the blocked state
# in place. The artifact is private (mode 0600) and never carries credentials.
# The deployed .env decides whether the one dedicated cache listener connection
# is budgeted: an absent key or 0 stays byte-identical to a listener-less
# budget, and any other value refuses the deploy before any schema change.
if [[ "$RUST_SELECTED" == "1" ]]; then
    RUST_CACHE_LISTENER="0"
    while IFS= read -r env_line || [[ -n "$env_line" ]]; do
        # Only a dotenv definition of this key counts (the same forms the
        # Python helper's dotenv reader parses: optional leading whitespace
        # and `export`); a value or comment that merely contains the text is
        # another key's business.
        [[ "$env_line" =~ ^([[:space:]]*)(export[[:space:]]+)?RUST_CACHE_LISTENER= ]] || continue
        listener_value="${env_line#*RUST_CACHE_LISTENER=}"
        # One layer of matching quotes, as the dotenv readers unquote it.
        if [[ "$listener_value" == \"*\" || "$listener_value" == \'*\' ]]; then
            listener_value="${listener_value:1:${#listener_value}-2}"
        fi
        if [[ "$listener_value" != "0" && "$listener_value" != "1" ]]; then
            echo "RUST_CACHE_LISTENER in .env must be exactly 0 or 1; refusing the deploy." >&2
            exit 1
        fi
        RUST_CACHE_LISTENER="$listener_value"
    done < "$BACKEND_DIR/.env"
    RUST_STREAM_LISTENER="0"
    if [[ "$MIGRATION_TARGET_REVISION" == "sse_20261021_replay_floor" ]]; then
        RUST_STREAM_LISTENER="1"
    fi
    if ! "${PYTHON_ENV[@]}" ./venv/bin/python scripts/prepare_rust_budget.py \
        --cache-listener "$RUST_CACHE_LISTENER" \
        --stream-listener "$RUST_STREAM_LISTENER" \
        --environment-file "$BACKEND_DIR/.env" \
        --release-sha "$CURRENT_SHA" \
        --output "$RUST_RUNTIME_DIR/budget.json"; then
        echo "Rust runtime budget preparation refused." >&2
        exit 1
    fi
fi

"${PYTHON_ENV[@]}" ENVIRONMENT="$DEPLOY_ENVIRONMENT" RELEASE_SHA="$CURRENT_SHA" \
    ./venv/bin/python scripts/apply_migration_checkpoint.py \
    --source-root "$BACKEND_DIR" \
    --environment-file "$BACKEND_DIR/.env" \
    --source-sha "$CURRENT_SHA" \
    --approved-source-sha "$MIGRATION_APPROVED_SOURCE_SHA" \
    --expected-current "$MIGRATION_EXPECTED_CURRENT_REVISION" \
    --target "$MIGRATION_TARGET_REVISION"

"${PYTHON_ENV[@]}" ./venv/bin/python \
    "$ACL_CONVERGENCE_TEMP/$ACL_CONVERGENCE_HELPER_PATH" \
    --environment "$DEPLOY_ENVIRONMENT" \
    --environment-file "$BACKEND_DIR/.env" \
    --bundle-root "$ACL_CONVERGENCE_TEMP"
cleanup_acl_convergence_bundle

# Freeze the migrator-verified checkpoint for the app-only Rust startup.
if [[ "$RUST_SELECTED" == "1" ]]; then
    if [[ -L "$RUST_RUNTIME_DIR/revision.json" ]]; then
        echo "Revision record must not be a symlink." >&2; exit 1
    fi
    REVISION_TEMP="$(mktemp "$RUST_RUNTIME_DIR/.revision.XXXXXXXX")"
    printf '{"release_sha":"%s","revision":"%s"}\n' "$CURRENT_SHA" "$MIGRATION_TARGET_REVISION" > "$REVISION_TEMP"
    chmod 0600 "$REVISION_TEMP"
    mv -- "$REVISION_TEMP" "$RUST_RUNTIME_DIR/revision.json"
fi

assert_clean_tree "Deploy steps"

# The minimal Rust runtime environment is prepared after the exact migration
# helper and ACL convergence, before the restart window. It receives the app
# database settings, runtime identity and the Phase 2 verification/invite
# settings (Task 23), never the migrator URL.
if [[ "$RUST_SELECTED" == "1" ]]; then
    if ! "${PYTHON_ENV[@]}" ./venv/bin/python \
        "$RUST_RUNTIME_PREPARER_PATH" \
        --environment "$DEPLOY_ENVIRONMENT" \
        --environment-file "$BACKEND_DIR/.env" \
        --release-sha "$CURRENT_SHA" \
        --output-dir "$RUST_RUNTIME_DIR" \
        --source-root "$BACKEND_DIR" \
        --migration-revision "$MIGRATION_TARGET_REVISION"; then
        echo "Rust runtime environment preparation refused." >&2
        exit 1
    fi
fi

# Publish the artifact selection under blocked state: a regular-file active
# binary installed by atomic rename from the verified staged release, never a
# source-tree symlink. The Rust unit's ExecStart resolves this path.
if [[ "$RUST_SELECTED" == "1" ]]; then
    STAGED_BINARY="$RUST_EXTRACT_DIR/verdaxis-read"
    ACTIVE_BINARY="$RUST_RUNTIME_DIR/verdaxis-read"
    if [[ ! -f "$STAGED_BINARY" || -L "$STAGED_BINARY" ]]; then
        echo "The staged Rust binary must be a regular file." >&2
        exit 1
    fi
    if [[ -e "$ACTIVE_BINARY" && ! -f "$ACTIVE_BINARY" ]]; then
        echo "The active Rust binary path is not a regular file." >&2
        exit 1
    fi
    if ! cp -- "$STAGED_BINARY" "$ACTIVE_BINARY.tmp.$$" \
        || ! chmod 0755 "$ACTIVE_BINARY.tmp.$$" \
        || ! mv -- "$ACTIVE_BINARY.tmp.$$" "$ACTIVE_BINARY"; then
        echo "Could not install the active Rust binary by atomic rename." >&2
        exit 1
    fi
fi

# Recheck drift under the same deploy lock without giving the Rust unit .env.
if [[ "$RUST_SELECTED" == "1" ]]; then
    "${PYTHON_ENV[@]}" ./venv/bin/python scripts/prepare_rust_budget.py --verify \
        --environment-file "$BACKEND_DIR/.env" --release-sha "$CURRENT_SHA" \
        --output "$RUST_RUNTIME_DIR/budget.json"
fi

acquire_restart_window_lock
DEPLOY_PHASE="restart-authorized"
write_deploy_state "$DEPLOY_PHASE" "$CURRENT_SHA"
SERVICE_RESTART_ATTEMPTED=1
sudo -n systemctl restart "$SERVICE_NAME"
DEPLOY_PHASE="readiness-pending"
write_deploy_state "$DEPLOY_PHASE" "$CURRENT_SHA"

systemctl is-active --quiet "$SERVICE_NAME"
for ((attempt = 1; attempt <= HEALTH_ATTEMPTS; attempt++)); do
    if HEALTH_PAYLOAD="$(curl --fail --silent --show-error --max-time 15 "$HEALTH_URL")"; then
        if printf '%s' "$HEALTH_PAYLOAD" | "${PYTHON_ENV[@]}" ./venv/bin/python \
            scripts/validate_health_response.py \
            --environment "$DEPLOY_ENVIRONMENT" --release-sha "$CURRENT_SHA"; then
            break
        fi
    fi
    if [[ "$attempt" == "$HEALTH_ATTEMPTS" ]]; then
        echo "Backend health check failed after ${HEALTH_ATTEMPTS} attempts: ${HEALTH_URL}" >&2
        exit 1
    fi
    echo "Backend not ready (${attempt}/${HEALTH_ATTEMPTS}); retrying in ${HEALTH_RETRY_DELAY}s..."
    sleep "$HEALTH_RETRY_DELAY"
done

echo "Backend is healthy: $HEALTH_URL"
if [[ "$RUST_SELECTED" == "1" ]]; then
    RUST_PHASE_STARTED=1
    write_deploy_state "restart-authorized" "$CURRENT_SHA"
    sudo -n systemctl restart "$RUST_UNIT_NAME"
    RUST_HEALTH_URL="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["health_url"])' \
        "$RUST_RUNTIME_DIR/$DEPLOY_ENVIRONMENT-policy.json")" \
        || { echo "The staged Rust policy does not name a health URL." >&2; exit 1; }
    systemctl is-active --quiet "$RUST_UNIT_NAME"
    for ((attempt = 1; attempt <= HEALTH_ATTEMPTS; attempt++)); do
        if RUST_PAYLOAD="$(curl --fail --silent --show-error --max-time 15 "$RUST_HEALTH_URL")"; then
            if printf '%s' "$RUST_PAYLOAD" | "${PYTHON_ENV[@]}" ./venv/bin/python \
                scripts/validate_health_response.py \
                --environment "$DEPLOY_ENVIRONMENT" --release-sha "$CURRENT_SHA"; then
                break
            fi
        fi
        if [[ "$attempt" == "$HEALTH_ATTEMPTS" ]]; then
            echo "Rust health check failed after ${HEALTH_ATTEMPTS} attempts: $RUST_HEALTH_URL" >&2
            exit 1
        fi
        echo "Rust not ready (${attempt}/${HEALTH_ATTEMPTS}); retrying in ${HEALTH_RETRY_DELAY}s..."
        sleep "$HEALTH_RETRY_DELAY"
    done
    echo "Rust service is healthy: $RUST_HEALTH_URL"
    # Both services are exact-ready, so enable the Rust unit before the deploy
    # records success: a host reboot must not leave the activated unit stopped.
    sudo -n systemctl enable "$RUST_UNIT_NAME"
fi
clear_deploy_state
echo "=== Backend Deployment Complete ==="
DEPLOY_STARTED=0
cleanup_acl_convergence_bundle
cleanup_migration_policy_bundle
cleanup_rust_policy_bundle
cleanup_rust_extract
trap - EXIT

#!/usr/bin/env bash
# Systemd-aware deploy helper for the Verdaxis backend.
#
# Dry-run may fetch Git objects and create private temporary files, but it does
# not change deployed source/runtime state. It never executes candidate
# application code or supplies candidate code with live secrets, home,
# database, agent, or network access.
#
# The real run executes no interpreter of the release: every database, budget,
# runtime-environment and readiness step runs the verified release binary
# (staged until it is installed), and the ACL helper is reviewed shell.
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
    BINARY_ENV=(/usr/bin/env -i PATH="$PATH")
    # shellcheck source=deploy/scripts/safe-root-git.sh
    source "$BACKEND_DIR/deploy/scripts/safe-root-git.sh"
    GIT=(safe_root_git "$BACKEND_DIR")
    DEPLOY_STATE_DIR="$BACKEND_DIR/.runtime-deploy"
    if [[ -n "${TARGET_BRANCH:-}" && "$TARGET_BRANCH" != "$DEFAULT_BRANCH" ]]; then
        echo "Canonical deploy checkout refuses a different target branch." >&2
        exit 2
    fi
    TARGET_BRANCH="$DEFAULT_BRANCH"
else
    # Ambient Git routing must stay inert outside the canonical layout too;
    # the boundary helper also clears it inside its own subshell.
    # shellcheck disable=SC2031
    export GIT_NO_REPLACE_OBJECTS=1
    # Disposable fixtures use the same Git boundary as deployed checkouts.
    # shellcheck source=deploy/scripts/safe-root-git.sh
    source "$BACKEND_DIR/deploy/scripts/safe-root-git.sh"
    GIT=(safe_root_git "$BACKEND_DIR")
    DEPLOY_STATE_DIR="${DEPLOY_STATE_DIR:-$BACKEND_DIR/.runtime-deploy}"
    TARGET_BRANCH="${TARGET_BRANCH:-$DEFAULT_BRANCH}"
    BINARY_ENV=(/usr/bin/env)
fi
for required_tool in jq flock openssl sync; do
    command -v "$required_tool" > /dev/null || {
        echo "The deploy requires $required_tool." >&2
        exit 2
    }
done

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
ACL_CONVERGENCE_HELPER_PATH="deploy/scripts/converge-acls.sh"
DEPLOY_START_GUARD_HELPER_PATH="deploy/scripts/verify-deploy-start-guard.sh"
RUST_STAGING_POLICY_PATH="deploy/rust/staging-policy.json"
RUST_POLICY_PATH="deploy/rust/$DEPLOY_ENVIRONMENT-policy.json"
# Production deploys the staging tip and CI signs the artifact on the staging
# push, so both environments bind provenance to that one ref. The
# repository, workflow path, exact commit and attestation checks stay exact.
RUST_WORKFLOW_REF="refs/heads/staging"
HOSTED_REHEARSAL=0
ARTIFACT_VERIFIER_ENV=("${BINARY_ENV[@]}")
if [[ -n ${REHEARSAL_WORKFLOW_REF:-} ]]; then
    # Preserve the actual attested feature ref only inside the disposable VM.
    # Normal deploys remain bound to the promoted staging artifact.
    [[ ${CI:-} == true && ${GITHUB_ACTIONS:-} == true &&
       ${RUNNER_ENVIRONMENT:-} == github-hosted &&
       ( $EUID == 0 || ( $EUID == "$(id -u verdaxis-prod)" && $EUID == "$(stat -c %u "$BACKEND_DIR")" ) ) &&
       ${VERDAXIS_DISPOSABLE_DEPLOY:-} == 1 && $(uname -s) == Linux &&
       $(</proc/1/comm) == systemd &&
       $REHEARSAL_WORKFLOW_REF =~ ^refs/heads/[A-Za-z0-9._/-]+$ &&
       ${GITHUB_RUN_ID:-} =~ ^[0-9]+$ &&
       ${REHEARSAL_CHECKOUT:-} == "$BACKEND_DIR" &&
       $BACKEND_DIR =~ ^/home/verdaxis-prod/verdaxis/(prod|staging)/be$ &&
       -f /run/verdaxis-disposable-rehearsal && ! -L /run/verdaxis-disposable-rehearsal &&
       $(stat -c '%u:%a' /run/verdaxis-disposable-rehearsal) == 0:644 ]] || {
        printf 'Disposable artifact source requires a hosted rehearsal.\n' >&2
        exit 2
    }
    grep -Fxq "$GITHUB_RUN_ID:$BACKEND_DIR" /run/verdaxis-disposable-rehearsal || {
        printf 'Disposable source marker differs.\n' >&2
        exit 2
    }
    RUST_WORKFLOW_REF=$REHEARSAL_WORKFLOW_REF
    HOSTED_REHEARSAL=1
    # These values reach only the trusted verifier, never a candidate binary.
    ARTIFACT_VERIFIER_ENV+=(CI=true GITHUB_ACTIONS=true RUNNER_ENVIRONMENT=github-hosted
        VERDAXIS_DISPOSABLE_DEPLOY=1 "REHEARSAL_WORKFLOW_REF=$REHEARSAL_WORKFLOW_REF"
        "REHEARSAL_CHECKOUT=$REHEARSAL_CHECKOUT" "GITHUB_RUN_ID=$GITHUB_RUN_ID")
fi
RUST_PROVENANCE_KEY_PATH="deploy/rust/artifact-provenance.pub.pem"
RUST_ARTIFACT_VERIFIER_PATH="deploy/scripts/verify-artifact.sh"
RUST_UNIT_NAME="verdaxis-rust-staging.service"
if [[ "$DEPLOY_ENVIRONMENT" == "production" ]]; then
    RUST_UNIT_NAME="verdaxis-rust.service"
fi
HOST_PROVENANCE_KEY_PATH="${HOST_PROVENANCE_KEY_PATH:-/etc/verdaxis/artifact-provenance.pub.pem}"
# A trusted acceptance producer writes these private files after the native
# restart. The record must match this transaction's nonce and installed bytes.
DEPLOY_ACCEPTANCE_RECORD="${DEPLOY_ACCEPTANCE_RECORD:-}"
DEPLOY_ACCEPTANCE_SIGNATURE="${DEPLOY_ACCEPTANCE_SIGNATURE:-}"
DEPLOY_PUBLICATION_LOG="${DEPLOY_PUBLICATION_LOG:-}"
DEPLOY_KILL_POINT="${DEPLOY_KILL_POINT:-}"
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
# Optional approval of the watchlist producer activation. Without them the
# keys already recorded in the release metadata are carried forward unchanged.
WATCHLIST_EVENT_PRODUCER="${WATCHLIST_EVENT_PRODUCER:-}"
WATCHLIST_PRODUCER_ACTIVATED_AT="${WATCHLIST_PRODUCER_ACTIVATED_AT:-}"
RELEASE_WATCHLIST_EVENT_PRODUCER=""
RELEASE_WATCHLIST_PRODUCER_ACTIVATED_AT=""
DEPLOY_LOCK_FD=""
DEPLOY_NONCE=""
STAGED_BINARY=""
VERIFIED_BINARY_SHA256=""
RESTART_WINDOW_LOCK_FD=""
DEPLOY_STARTED=0
SERVICE_RESTART_ATTEMPTED=0
DRY_RUN_TEMP=""
ACL_CONVERGENCE_TEMP=""
MIGRATION_POLICY_TEMP=""
RUST_POLICY_TEMP=""
RUST_POLICY_PRESENT=0
RUST_SELECTED=0
RUST_EXTRACT_DIR=""
RUST_RUNTIME_DIR="$BACKEND_DIR/.runtime-rust"
ACTIVE_BINARY="$RUST_RUNTIME_DIR/verdaxis-read"

# Publication probes contain identity and boundary names only, never payloads.
# Kill points are available only in an explicitly disposable noncanonical tree.
publication_boundary() {
    local side="$1" name="$2"
    if [[ -n "$DEPLOY_PUBLICATION_LOG" ]]; then
        if [[ -L "$DEPLOY_PUBLICATION_LOG" \
            || ( -e "$DEPLOY_PUBLICATION_LOG" && ! -f "$DEPLOY_PUBLICATION_LOG" ) ]]; then
            echo "Publication log must be a regular file." >&2; exit 1
        fi
        (umask 077; printf '%s %s %s %s\n' "$side" "$name" \
            "$DEPLOY_ENVIRONMENT" "$APPROVED_RELEASE_SHA" >> "$DEPLOY_PUBLICATION_LOG")
        sync -f "$DEPLOY_PUBLICATION_LOG"
    fi
    if [[ -n "$DEPLOY_KILL_POINT" ]]; then
        local hosted_rehearsal=0
        if [[ ${CI:-} == true && ${GITHUB_ACTIONS:-} == true && ${RUNNER_ENVIRONMENT:-} == github-hosted &&
              ( $EUID == 0 || ( $EUID == "$(id -u verdaxis-prod)" && $EUID == "$(stat -c %u "$BACKEND_DIR")" ) ) && $(uname -s) == Linux && $(</proc/1/comm) == systemd &&
              -f /run/verdaxis-disposable-rehearsal && ! -L /run/verdaxis-disposable-rehearsal &&
              $(stat -c '%u:%a' /run/verdaxis-disposable-rehearsal) == 0:644 ]]; then
            if grep -Fxq "${GITHUB_RUN_ID:-missing}:$BACKEND_DIR" /run/verdaxis-disposable-rehearsal; then
                hosted_rehearsal=1
            fi
        fi
        if [[ ( "$CANONICAL_DEPLOY_LAYOUT" == "1" && $hosted_rehearsal == 0 ) || "${VERDAXIS_DISPOSABLE_DEPLOY:-}" != "1" ]]; then
            echo "Publication kill points require a disposable checkout." >&2; exit 2
        fi
        if [[ "$DEPLOY_KILL_POINT" == "$side:$name" ]]; then
            kill -KILL "$$"
        fi
    fi
}

verify_window_acceptance() {
    local record="$DEPLOY_ACCEPTANCE_RECORD" signature="$DEPLOY_ACCEPTANCE_SIGNATURE"
    local digest
    [[ -n "$record" && -n "$signature" && -f "$record" && ! -L "$record" \
        && -f "$signature" && ! -L "$signature" ]] || return 1
    [[ $(stat -Lc '%s' -- "$record") -le 16384 \
        && $(stat -Lc '%s' -- "$signature") -le 16384 ]] || return 1
    # Copy once, then verify and parse the same private immutable snapshot.
    local directory
    directory="$(mktemp -d /tmp/verdaxis-window-acceptance.XXXXXXXX)" || return 1
    if ! chmod 0700 "$directory" \
        || ! cp -- "$record" "$directory/record.json" \
        || ! cp -- "$signature" "$directory/signature"; then
        rm -rf -- "$directory"; return 1
    fi
    if [[ $(stat -Lc '%s' -- "$directory/record.json") -gt 16384 \
        || $(stat -Lc '%s' -- "$directory/signature") -gt 16384 ]]; then
        rm -rf -- "$directory"; return 1
    fi
    if ! openssl pkeyutl -verify -pubin -rawin -inkey "$PROVENANCE_PUBLIC_KEY_PATH" \
        -sigfile "$directory/signature" -in "$directory/record.json" >/dev/null 2>&1; then
        rm -rf -- "$directory"; return 1
    fi
    if ! verify_binary_digest "$ACTIVE_BINARY"; then
        rm -rf -- "$directory"; return 1
    fi
    digest="$VERIFIED_BINARY_SHA256"
    local result=0
    jq -e -s --arg environment "$DEPLOY_ENVIRONMENT" --arg sha "$CURRENT_SHA" \
        --arg nonce "$DEPLOY_NONCE" --arg current "$MIGRATION_EXPECTED_CURRENT_REVISION" \
        --arg target "$MIGRATION_TARGET_REVISION" --arg digest "$digest" '
        length == 1 and (.[0] |
        keys == ["auth","binary_sha256","deploy_nonce","environment","expected_current",
                 "monitor","passive_harm","release_sha","session","target"] and
        .environment == $environment and .release_sha == $sha and
        .deploy_nonce == $nonce and .expected_current == $current and .target == $target and
        .binary_sha256 == $digest and .auth == true and .session == true and
        .monitor == true and .passive_harm == true)' "$directory/record.json" >/dev/null || result=1
    rm -rf -- "$directory"
    return "$result"
}

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
            || "${current,,}" =~ ^(base|head|heads|latest)$ \
            || "${target,,}" =~ ^(base|head|heads|latest)$ ]]; then
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
        # Older trees without this environment's policy have no Rust side;
        # the real run refuses them.
        RUST_ENABLED=0
        return 0
    fi
    RUST_POLICY_PRESENT=1
    if ! RUST_ENABLED="$(jq -e -r --arg environment "$DEPLOY_ENVIRONMENT" \
        'if (.rust_enabled | type) == "boolean" and .environment == $environment
         then (if .rust_enabled then "1" else "0" end) else error("unexpected policy") end' \
        "$staged/$policy_path" 2> /dev/null)"; then
        echo "Invalid Rust staging policy or Rust unit is not manifest-selected." >&2
        exit 1
    fi
    if [[ "$RUST_ENABLED" == "1" ]] \
        && ! grep -qxF "$DEPLOY_ENVIRONMENT"$'\t'"$RUST_UNIT_NAME" "$staged/$UNIT_MANIFEST_PATH"; then
        echo "Invalid Rust staging policy or Rust unit is not manifest-selected." >&2
        exit 1
    fi
}

# Resolve which provenance public key to pass to the verifier. The reviewed
# tree's digest is authoritative: an operator-installed host key is accepted
# only when its SHA-256 equals that reviewed digest, and is then preferred
# because it is root-owned. Anything else refuses before a service stops.
resolve_provenance_public_key() {
    local source_sha="$1"
    local tree_key="$BACKEND_DIR/$RUST_PROVENANCE_KEY_PATH"
    local expected_digest host_digest

    if [[ -n ${REHEARSAL_PUBLIC_KEY:-} ]]; then
        # The hosted VM uses its own key, not the serving-host release key.
        # HOSTED_REHEARSAL is set only after the complete marker/ref gate above.
        [[ $HOSTED_REHEARSAL == 1 && $REHEARSAL_PUBLIC_KEY == /etc/verdaxis/artifact-provenance.pub.pem &&
           -f $REHEARSAL_PUBLIC_KEY && ! -L $REHEARSAL_PUBLIC_KEY &&
           $(stat -c '%u:%a' "$REHEARSAL_PUBLIC_KEY") == 0:644 &&
           ! -L /etc/verdaxis && $(stat -c '%u:%a' /etc/verdaxis) == 0:755 ]] || {
            printf 'Disposable provenance key requires the hosted rehearsal.\n' >&2
            exit 2
        }
        PROVENANCE_PUBLIC_KEY_PATH=$REHEARSAL_PUBLIC_KEY
        return 0
    fi
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
    if ! "${ARTIFACT_VERIFIER_ENV[@]}" bash "$RUST_ARTIFACT_VERIFIER_PATH" \
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
    VERIFIED_BINARY_SHA256="$(jq -er '.binary.sha256 | strings | select(test("^[0-9a-f]{64}$"))' \
        "$extract_dir/artifact-metadata.json")" || {
        echo "Verified artifact has no exact executable digest." >&2; exit 1;
    }
    verify_binary_digest "$extract_dir/verdaxis-read"
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
    local temporary_file
    temporary_file="$(mktemp --tmpdir="$DEPLOY_STATE_DIR" '.monitor.XXXXXXXX')"

    printf 'ENVIRONMENT=%s\nRELEASE_SHA=%s\n' \
        "$DEPLOY_ENVIRONMENT" "$release_sha" > "$temporary_file"
    chmod 0644 "$temporary_file"
    sync -f "$temporary_file"
    publication_boundary before monitor
    mv -- "$temporary_file" "$identity_file"
    sync -f "$BACKEND_DIR"
    publication_boundary after monitor
}

# True when the revision is the watchlist producer checkpoint or a later one.
# The release metadata tests keep this list equal to that part of
# deploy/rust/allowed-revisions.json.
revision_has_watchlist_producer() {
    case "$1" in
        p89_20261022_watchlist_producer|p89_20261022_trade_freeze|ob_20261031_offboarding|sqlx_1|sqlx_2|sqlx_3) return 0 ;;
    esac
    return 1
}

# Print the value of KEY recorded in the current release metadata, or nothing.
# A repeated key is refused rather than guessed.
recorded_release_value() {
    local key="$1" line value="" count=0
    if [[ ! -e "$RELEASE_ENV_FILE" ]]; then
        return 0
    fi
    while IFS= read -r line || [[ -n "$line" ]]; do
        if [[ "$line" == "$key="* ]]; then
            value="${line#*=}"
            count=$((count + 1))
        fi
    done < "$RELEASE_ENV_FILE"
    if (( count > 1 )); then
        echo "The release metadata records $key more than once; refusing." >&2
        return 1
    fi
    printf '%s' "$value"
}

valid_watchlist_producer() {
    [[ "$1" == "legacy-python" || "$1" == "rust-market-v1" ]]
}

# A real UTC time in RFC 3339 form: seconds, an optional fraction and the
# offset Z or +00:00, on a calendar date and clock time that GNU date reads
# back unchanged (so 2026-02-30 or 25:00 is refused). The instant is recorded
# once and never changes, so nothing else may be recorded.
valid_activation_instant() {
    local day clock normalized
    [[ "$1" =~ ^([0-9]{4}-[0-9]{2}-[0-9]{2})T([0-9]{2}:[0-9]{2}:[0-9]{2})(\.[0-9]{1,9})?(Z|\+00:00)$ ]] \
        || return 1
    day="${BASH_REMATCH[1]}"
    clock="${BASH_REMATCH[2]}"
    normalized="$(TZ=UTC date -u -d "$day $clock" '+%Y-%m-%d %H:%M:%S' 2>/dev/null)" || return 1
    [[ "$normalized" == "$day $clock" ]]
}

# Decide the watchlist producer keys of the release about to be published.
# The recorded keys are carried forward byte for byte unless the operator
# supplies a value. A recorded activation instant never changes; the producer
# may move between its two names under the same instant. A producer needs an
# instant and an instant needs a producer. Below the producer checkpoint the
# schema has no producer, so both keys are dropped and supplying one refuses.
resolve_watchlist_producer_keys() {
    local target="$1" recorded_producer recorded_instant producer instant
    recorded_producer="$(recorded_release_value WATCHLIST_EVENT_PRODUCER)" || return 1
    recorded_instant="$(recorded_release_value WATCHLIST_PRODUCER_ACTIVATED_AT)" || return 1
    if ! revision_has_watchlist_producer "$target"; then
        if [[ -n "$WATCHLIST_EVENT_PRODUCER" || -n "$WATCHLIST_PRODUCER_ACTIVATED_AT" ]]; then
            echo "Watchlist producer keys require the producer checkpoint or a later target; refusing." >&2
            return 1
        fi
        RELEASE_WATCHLIST_EVENT_PRODUCER=""
        RELEASE_WATCHLIST_PRODUCER_ACTIVATED_AT=""
        return 0
    fi
    producer="${WATCHLIST_EVENT_PRODUCER:-$recorded_producer}"
    instant="${WATCHLIST_PRODUCER_ACTIVATED_AT:-$recorded_instant}"
    if [[ -n "$producer" ]] && ! valid_watchlist_producer "$producer"; then
        echo "WATCHLIST_EVENT_PRODUCER must be legacy-python or rust-market-v1; refusing." >&2
        return 1
    fi
    if [[ -n "$instant" ]] && ! valid_activation_instant "$instant"; then
        echo "WATCHLIST_PRODUCER_ACTIVATED_AT must be a real UTC time (RFC 3339, offset Z or +00:00); refusing." >&2
        return 1
    fi
    if [[ -n "$recorded_instant" && "$instant" != "$recorded_instant" ]]; then
        echo "The recorded watchlist producer activation instant never changes; refusing." >&2
        return 1
    fi
    if [[ -n "$producer" && -z "$instant" ]]; then
        echo "WATCHLIST_EVENT_PRODUCER requires WATCHLIST_PRODUCER_ACTIVATED_AT; refusing." >&2
        return 1
    fi
    if [[ -z "$producer" && -n "$instant" ]]; then
        echo "WATCHLIST_PRODUCER_ACTIVATED_AT requires WATCHLIST_EVENT_PRODUCER; refusing." >&2
        return 1
    fi
    RELEASE_WATCHLIST_EVENT_PRODUCER="$producer"
    RELEASE_WATCHLIST_PRODUCER_ACTIVATED_AT="$instant"
}

write_release_artifact() {
    local release_sha="$1"
    local migration_revision="$2"
    local temporary_file
    temporary_file="$(mktemp --tmpdir="$DEPLOY_STATE_DIR" '.release.XXXXXXXX')"

    echo "+ write immutable release metadata to $RELEASE_ENV_FILE"
    umask 077
    printf 'ENVIRONMENT=%s\nRELEASE_SHA=%s\nMIGRATION_REVISION=%s\n' \
        "$DEPLOY_ENVIRONMENT" "$release_sha" "$migration_revision" \
        > "$temporary_file"
    # The keys are absent until a release records an activation; the
    # resolver sets them before this runs.
    if [[ -n "${RELEASE_WATCHLIST_EVENT_PRODUCER:-}" ]]; then
        printf 'WATCHLIST_EVENT_PRODUCER=%s\nWATCHLIST_PRODUCER_ACTIVATED_AT=%s\n' \
            "$RELEASE_WATCHLIST_EVENT_PRODUCER" "${RELEASE_WATCHLIST_PRODUCER_ACTIVATED_AT:-}" \
            >> "$temporary_file"
    fi
    chmod 0600 "$temporary_file"
    sync -f "$temporary_file"
    publication_boundary before release
    mv -- "$temporary_file" "$RELEASE_ENV_FILE"
    sync -f "$BACKEND_DIR"
    publication_boundary after release
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
    # `migrate` accepts only the blocked state of the deploy that runs it,
    # named by this run's nonce. The restart phases keep the three-line form
    # the start guards read.
    if [[ "$phase" == "blocked" && -n "$DEPLOY_NONCE" ]] \
        && ! printf 'DEPLOY_NONCE=%s\n' "$DEPLOY_NONCE" >> "$temporary_file"; then
        rm -f -- "$temporary_file"
        return 1
    fi
    chmod 0644 "$temporary_file"
    sync -f "$temporary_file"
    mv -- "$temporary_file" "$DEPLOY_STATE_FILE"
    sync -f "$DEPLOY_STATE_DIR"
}

clear_deploy_state() {
    rm -f -- "$DEPLOY_STATE_FILE"
    sync -f "$DEPLOY_STATE_DIR"
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

# Run a verified release binary with the release identity. Each command
# names the deployed .env itself when it needs it and reads only its own keys.
verify_binary_digest() {
    local binary="$1" digest
    [[ -f "$binary" && ! -L "$binary" ]] || return 1
    digest="$(sha256sum -- "$binary" | cut -d ' ' -f 1)" || return 1
    if [[ ! "$VERIFIED_BINARY_SHA256" =~ ^[0-9a-f]{64}$ || "$digest" != "$VERIFIED_BINARY_SHA256" ]]; then
        echo "Executable bytes differ from the verified artifact." >&2; return 1
    fi
}

run_release_binary() {
    local binary="$1"
    shift
    verify_binary_digest "$binary" || return 1
    "${BINARY_ENV[@]}" DEPLOY_STATE_DIR="$DEPLOY_STATE_DIR" "$binary" "$@" \
        --environment "$DEPLOY_ENVIRONMENT" --release-sha "$APPROVED_RELEASE_SHA"
}

# Exact readiness: the body must be exactly the four deployed keys with this
# environment and release (`guard health`, stricter than the old extra-key rule).
readiness_exact() {
    local url="$1" payload
    payload="$(curl --fail --silent --show-error --max-time 15 "$url")" || return 1
    printf '%s' "$payload" | run_release_binary "$ACTIVE_BINARY" guard health
}

# The retained backend unit was removed from both hosts after the cutover. A
# unit that systemd does not know is already stopped and disabled, so its
# stop/disable commands and their sudo authority are not needed. Any other
# answer, including an unreadable one, keeps the installed-unit path.
retained_backend_installed() {
    local load_state
    load_state="$(systemctl show --property=LoadState --value "$SERVICE_NAME")" || return 0
    [[ "$load_state" != "not-found" ]]
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
        if ! write_deploy_state "blocked" "${CURRENT_SHA:-$APPROVED_RELEASE_SHA}"; then
            echo "WARNING: could not restore durable deployment state." >&2
        fi
        if [[ "$SERVICE_RESTART_ATTEMPTED" == "1" ]]; then
            if ! sudo -n systemctl stop "$RUST_UNIT_NAME"; then
                echo "WARNING: could not stop the failed Rust service." >&2
            fi
            sudo -n systemctl disable "$RUST_UNIT_NAME" || echo "WARNING: Rust disable failed." >&2
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
    if [[ -n "$MIGRATION_EXPECTED_CURRENT_REVISION" || -n "$MIGRATION_TARGET_REVISION" ]]; then
        if [[ -z "$MIGRATION_EXPECTED_CURRENT_REVISION" || -z "$MIGRATION_TARGET_REVISION" ]]; then
            echo "Dry-run requires both literal migration current and target, or neither." >&2; exit 1
        fi
        validate_migration_policy \
            "$DRY_RUN_TEMP/candidate-units/$MIGRATION_POLICY_PATH" \
            "$MIGRATION_EXPECTED_CURRENT_REVISION" "$MIGRATION_TARGET_REVISION"
    else
        validate_migration_policy "$DRY_RUN_TEMP/candidate-units/$MIGRATION_POLICY_PATH"
    fi
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
    if [[ -n "$MIGRATION_EXPECTED_CURRENT_REVISION" ]]; then
        echo "APPROVED_MIGRATION_CURRENT=$MIGRATION_EXPECTED_CURRENT_REVISION"
        echo "APPROVED_MIGRATION_TARGET=$MIGRATION_TARGET_REVISION"
    fi
    echo "Migration remains unapproved until the operator also sets:"
    echo "  MIGRATION_APPROVED_SOURCE_SHA=$REMOTE_SHA"
    echo "  MIGRATION_EXPECTED_CURRENT_REVISION=<exact-live-revision>"
    echo "  MIGRATION_TARGET_REVISION=<allowlisted-checkpoint>"
    echo "Watchlist producer keys (carried forward unless supplied):"
    for producer_key in WATCHLIST_EVENT_PRODUCER WATCHLIST_PRODUCER_ACTIVATED_AT; do
        recorded_value="$(recorded_release_value "$producer_key")" || exit 1
        echo "  $producer_key recorded=${recorded_value:-<absent>} supplied=${!producer_key:-<absent>}"
    done
    if [[ "$RUST_POLICY_PRESENT" == "1" ]]; then
        echo "Rust staging policy rust_enabled=$RUST_ENABLED; a Rust-enabled deploy additionally requires RUST_ARTIFACT_ARCHIVE, RUST_PROVENANCE_RECORD and RUST_PROVENANCE_SIGNATURE."
        echo "Artifact runtime compatibility remains untested: no Cargo or binary was executed."
    fi
    echo "No candidate code was executed; live secrets, home, database, and network were not supplied to candidate code."
    echo "Skipped mutations:"
    echo "  - source update (the exact SHA was inspected from an immutable Git archive)"
    echo "  - candidate application preflight (omitted; this dry-run does not execute candidate code)"
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
    || "${MIGRATION_EXPECTED_CURRENT_REVISION,,}" =~ ^(base|head|heads|latest)$ \
    || "${MIGRATION_TARGET_REVISION,,}" =~ ^(base|head|heads|latest)$ ]]; then
    echo "Exact literal migration current and target revisions are required." >&2
    exit 1
fi
# The release serves only sqlx_3 (deploy/rust/allowed-revisions.json). The
# first deploy of a host moves it from sqlx_2; later deploys keep sqlx_3.
case "$MIGRATION_EXPECTED_CURRENT_REVISION:$MIGRATION_TARGET_REVISION" in
    sqlx_2:sqlx_3|sqlx_3:sqlx_3) ;;
    *)
        echo "Deploy requires the sqlx_2 to sqlx_3 transition or the sqlx_3 self-transition." >&2; exit 1
        ;;
esac
if [[ -z "$DEPLOY_ACCEPTANCE_RECORD" || -z "$DEPLOY_ACCEPTANCE_SIGNATURE" ]]; then
    echo "Signed window acceptance paths are required before mutation." >&2; exit 1
fi
if ! resolve_watchlist_producer_keys "$MIGRATION_TARGET_REVISION"; then
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

# The backend service keeps its installed dependencies: this deploy installs
# none. A release that changes the dependency files is refused before any
# state, so the service never starts against code its environment lacks.
for dependency_file in requirements.txt constraints.txt; do
    installed_blob="$("${GIT[@]}" rev-parse --verify --quiet "HEAD:$dependency_file" || true)"
    release_blob="$("${GIT[@]}" rev-parse --verify --quiet "$REMOTE_SHA:$dependency_file" || true)"
    if [[ "$installed_blob" != "$release_blob" ]]; then
        echo "The release changes $dependency_file, and this deploy installs no dependency; refusing before any state." >&2
        exit 1
    fi
done

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
# Every database step below runs the verified release binary, so a release
# without an enabled Rust side cannot be deployed by this transaction.
if [[ "$RUST_ENABLED" != "1" ]]; then
    echo "The release policy does not enable the Rust service; this deploy needs its verified binary. Refusing before any state." >&2
    exit 1
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
if retained_backend_installed; then
    for action in stop disable; do
        sudo -n -l systemctl "$action" "$SERVICE_NAME" >/dev/null || {
            echo "Required retained backend stop/disable authority is unavailable." >&2; exit 1;
        }
    done
fi
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
fi
CURRENT_SHA="$("${GIT[@]}" rev-parse HEAD)"
STAGED_BINARY="$RUST_EXTRACT_DIR/verdaxis-read"
if [[ ! -f "$STAGED_BINARY" || -L "$STAGED_BINARY" ]]; then
    echo "The staged Rust binary must be a regular file." >&2
    exit 1
fi
# An interrupted deploy of this same release left its durable state, and its
# migration may already have reached the approved target. Only then is the
# target accepted as the current revision; `migrate run` reports it at target.
RESUMING=0
if [[ -f "$DEPLOY_STATE_FILE" && ! -L "$DEPLOY_STATE_FILE" ]] \
    && grep -qxF "ENVIRONMENT=$DEPLOY_ENVIRONMENT" "$DEPLOY_STATE_FILE" \
    && grep -qxF "RELEASE_SHA=$APPROVED_RELEASE_SHA" "$DEPLOY_STATE_FILE"; then
    RESUMING=1
fi
if ! run_release_binary "$STAGED_BINARY" migrate status --check \
    --expected-current "$MIGRATION_EXPECTED_CURRENT_REVISION" \
    --environment-file "$BACKEND_DIR/.env"; then
    if [[ "$RESUMING" != "1" ]] || ! run_release_binary "$STAGED_BINARY" migrate status --check \
        --expected-current "$MIGRATION_TARGET_REVISION" \
        --environment-file "$BACKEND_DIR/.env"; then
        echo "The database is not at the approved current revision; refusing before any state." >&2
        exit 1
    fi
    echo "Resuming the interrupted deploy of $APPROVED_RELEASE_SHA; the database is at the approved target."
fi
cleanup_migration_policy_bundle

# This run's nonce names its blocked state; `migrate` accepts only that state
# while both deploy locks are held. It is an ownership token, not a secret.
DEPLOY_NONCE="$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
if [[ ! "$DEPLOY_NONCE" =~ ^[0-9a-f]{64}$ ]]; then
    echo "Could not create the deploy nonce." >&2
    exit 1
fi
DEPLOY_STARTED=1
write_deploy_state "blocked" "$APPROVED_RELEASE_SHA"
# The restart window is held from here: the deploy-owned migration needs both
# locks, and the start guard still denies every start while state is blocked.
acquire_restart_window_lock

# The retained runtime is idle throughout M; a retry repeats these fences.
# Do not restart it, even on a failed native activation.
if retained_backend_installed; then
    sudo -n systemctl stop "$SERVICE_NAME"
    sudo -n systemctl disable "$SERVICE_NAME"
    if systemctl is-active --quiet "$SERVICE_NAME" \
        || systemctl is-enabled --quiet "$SERVICE_NAME"; then
        echo "Retained backend did not become stopped and disabled." >&2; exit 1
    fi
fi
sudo -n systemctl stop "$RUST_UNIT_NAME"
# Revoke every cached-read lease before source/identity changes. The native
# operation waits for the lease drain; activation below binds a fresh database
# incarnation and publishes authority only after the bind commits.
publication_boundary before cache-revoke
run_release_binary "$STAGED_BINARY" ops cache-authority revoke \
    --runtime-dir "$RUST_RUNTIME_DIR"
publication_boundary after cache-revoke
publication_boundary before source
"${GIT[@]}" fetch origin "refs/heads/$TARGET_BRANCH"
REMOTE_SHA="$("${GIT[@]}" rev-parse 'FETCH_HEAD^{commit}')"
if [[ "$REMOTE_SHA" != "$APPROVED_RELEASE_SHA" ]]; then
    echo "Remote target moved after approval; refusing to mutate source." >&2
    exit 1
fi
"${GIT[@]}" checkout "$TARGET_BRANCH"
"${GIT[@]}" merge --ff-only FETCH_HEAD
sync -f "$BACKEND_DIR"
publication_boundary after source

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

# The selected native artifact is already provenance-verified. Keep starts
# blocked until migration and ACLs complete, then publish runtime identities.
# The deployed identity and the app database role, attested by the staged
# release binary on one connection (it replaces the runtime preflight).
run_release_binary "$STAGED_BINARY" ops preflight --environment-file "$BACKEND_DIR/.env"

# The effective Task 2 connection budget is prepared whenever a Rust unit is
# selected, before any schema change; a refused budget leaves the blocked state
# in place. The artifact is private (mode 0600) and never carries credentials.
# The deployed .env decides whether the one dedicated cache listener connection
# is budgeted: an absent key or 0 stays byte-identical to a listener-less
# budget, and any other value refuses the deploy before any schema change.
if [[ "$RUST_SELECTED" == "1" ]]; then
    RUST_CACHE_LISTENER="0"
    while IFS= read -r env_line || [[ -n "$env_line" ]]; do
        # Only a dotenv definition of this key counts (optional leading
        # whitespace and `export`, as the dotenv readers parse it); a value or
        # comment that merely contains the text is another key's business.
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
    # The stream tables exist from the stream checkpoint on, and every later
    # checkpoint keeps them. The stream listener checkpoint test keeps this
    # list equal to that part of deploy/rust/allowed-revisions.json.
    RUST_STREAM_LISTENER="0"
    case "$MIGRATION_TARGET_REVISION" in
        sse_20261021_replay_floor|p5_20261020_read_indexes|p89_20261022_watchlist_producer|p89_20261022_trade_freeze|ob_20261031_offboarding|sqlx_1|sqlx_2|sqlx_3) RUST_STREAM_LISTENER="1" ;;
    esac
    publication_boundary before budget
    if ! run_release_binary "$STAGED_BINARY" ops prepare-budget \
        --cache-listener "$RUST_CACHE_LISTENER" \
        --stream-listener "$RUST_STREAM_LISTENER" \
        --environment-file "$BACKEND_DIR/.env" \
        --output "$RUST_RUNTIME_DIR/budget.json"; then
        echo "Rust runtime budget preparation refused." >&2
        exit 1
    fi
    sync -f "$RUST_RUNTIME_DIR/budget.json"
    sync -f "$RUST_RUNTIME_DIR"
    publication_boundary after budget
fi

# The one approved transition, as the migrator, owned by this deploy's
# blocked state and both locks. The migrator URL stays inside the binary.
run_release_binary "$STAGED_BINARY" migrate run \
    --expected-current "$MIGRATION_EXPECTED_CURRENT_REVISION" \
    --target "$MIGRATION_TARGET_REVISION" --apply \
    --deploy-nonce "$DEPLOY_NONCE" --environment-file "$BACKEND_DIR/.env"
# Exact postcheck: the database reports the approved target and nothing else.
run_release_binary "$STAGED_BINARY" migrate status --check \
    --expected-current "$MIGRATION_TARGET_REVISION" --environment-file "$BACKEND_DIR/.env"

"${BINARY_ENV[@]}" bash "$ACL_CONVERGENCE_TEMP/$ACL_CONVERGENCE_HELPER_PATH" \
    --binary "$STAGED_BINARY" \
    --environment "$DEPLOY_ENVIRONMENT" \
    --release-sha "$CURRENT_SHA" \
    --env-file "$BACKEND_DIR/.env" \
    --bundle-root "$ACL_CONVERGENCE_TEMP"
cleanup_acl_convergence_bundle

write_release_artifact "$CURRENT_SHA" "$MIGRATION_TARGET_REVISION"
# Carry the exact immutable policy into the selected runtime only under blocked
# state; failure between copies cannot authorize any service start.
publication_boundary before policy
if [[ -L "$RUST_RUNTIME_DIR/$DEPLOY_ENVIRONMENT-policy.json" ]]; then
    echo "Runtime policy must not be a symlink." >&2; exit 1
fi
cp -- "$RUST_POLICY_TEMP/$RUST_POLICY_PATH" "$RUST_RUNTIME_DIR/.policy.tmp.$$"
chmod 0600 "$RUST_RUNTIME_DIR/.policy.tmp.$$"
sync -f "$RUST_RUNTIME_DIR/.policy.tmp.$$"
mv -- "$RUST_RUNTIME_DIR/.policy.tmp.$$" "$RUST_RUNTIME_DIR/$DEPLOY_ENVIRONMENT-policy.json"
sync -f "$RUST_RUNTIME_DIR"
publication_boundary after policy
cleanup_rust_policy_bundle

# Freeze the migrator-verified checkpoint for the app-only Rust startup.
if [[ "$RUST_SELECTED" == "1" ]]; then
    if [[ -L "$RUST_RUNTIME_DIR/revision.json" ]]; then
        echo "Revision record must not be a symlink." >&2; exit 1
    fi
    REVISION_TEMP="$(mktemp "$RUST_RUNTIME_DIR/.revision.XXXXXXXX")"
    printf '{"release_sha":"%s","revision":"%s"}\n' "$CURRENT_SHA" "$MIGRATION_TARGET_REVISION" > "$REVISION_TEMP"
    chmod 0600 "$REVISION_TEMP"
    sync -f "$REVISION_TEMP"
    publication_boundary before revision
    mv -- "$REVISION_TEMP" "$RUST_RUNTIME_DIR/revision.json"
    sync -f "$RUST_RUNTIME_DIR"
    publication_boundary after revision
fi

assert_clean_tree "Deploy steps"

# The minimal Rust runtime environment is prepared after the exact migration
# helper and ACL convergence, before the restart window. It receives the app
# database settings, runtime identity and the Phase 2 verification/invite
# settings (Task 23), never the migrator URL.
if [[ "$RUST_SELECTED" == "1" ]]; then
    publication_boundary before runtime
    if ! run_release_binary "$STAGED_BINARY" ops prepare-runtime \
        --environment-file "$BACKEND_DIR/.env" \
        --output-dir "$RUST_RUNTIME_DIR" \
        --migration-revision "$MIGRATION_TARGET_REVISION"; then
        echo "Rust runtime environment preparation refused." >&2
        exit 1
    fi
    sync -f "$RUST_RUNTIME_DIR"
    publication_boundary after runtime
fi

# Rebind only after migration, ACL and matching runtime publications. The
# native command revokes/drains again and leaves caching revoked on failure.
publication_boundary before cache-bind
run_release_binary "$STAGED_BINARY" ops cache-authority activate --apply \
    --runtime-dir "$RUST_RUNTIME_DIR" --environment-file "$BACKEND_DIR/.env"
publication_boundary after cache-bind

# Publish the artifact selection under blocked state: a regular-file active
# binary installed by atomic rename from the verified staged release, never a
# source-tree symlink. The Rust unit's ExecStart resolves this path.
if [[ "$RUST_SELECTED" == "1" ]]; then
    if [[ -L "$ACTIVE_BINARY" || ( -e "$ACTIVE_BINARY" && ! -f "$ACTIVE_BINARY" ) ]]; then
        echo "The active Rust binary path is not a regular file." >&2
        exit 1
    fi
    publication_boundary before binary
    if ! cp -- "$STAGED_BINARY" "$ACTIVE_BINARY.tmp.$$" \
        || ! chmod 0755 "$ACTIVE_BINARY.tmp.$$" \
        || ! verify_binary_digest "$ACTIVE_BINARY.tmp.$$" \
        || ! sync -f "$ACTIVE_BINARY.tmp.$$" \
        || ! mv -- "$ACTIVE_BINARY.tmp.$$" "$ACTIVE_BINARY"; then
        echo "Could not install the active Rust binary by atomic rename." >&2
        exit 1
    fi
    verify_binary_digest "$ACTIVE_BINARY"
    sync -f "$RUST_RUNTIME_DIR"
    publication_boundary after binary
fi

# Recheck drift under the same deploy lock without giving the Rust unit .env.
# From here the installed binary runs; the staged copy is no longer used.
if [[ "$RUST_SELECTED" == "1" ]]; then
    run_release_binary "$ACTIVE_BINARY" ops prepare-budget --verify \
        --environment-file "$BACKEND_DIR/.env" --output "$RUST_RUNTIME_DIR/budget.json"
fi

if [[ "$RUST_SELECTED" == "1" ]]; then
    SERVICE_RESTART_ATTEMPTED=1
    publication_boundary before authorization
    write_deploy_state "restart-authorized" "$CURRENT_SHA"
    publication_boundary after authorization
    sudo -n systemctl restart "$RUST_UNIT_NAME"
    write_deploy_state "readiness-pending" "$CURRENT_SHA"
    RUST_HEALTH_URL="$(jq -e -r '.health_url | strings' \
        "$RUST_RUNTIME_DIR/$DEPLOY_ENVIRONMENT-policy.json")" \
        || { echo "The staged Rust policy does not name a health URL." >&2; exit 1; }
    systemctl is-active --quiet "$RUST_UNIT_NAME"
    for ((attempt = 1; attempt <= HEALTH_ATTEMPTS; attempt++)); do
        if readiness_exact "$RUST_HEALTH_URL"; then
            break
        fi
        if [[ "$attempt" == "$HEALTH_ATTEMPTS" ]]; then
            echo "Rust health check failed after ${HEALTH_ATTEMPTS} attempts: $RUST_HEALTH_URL" >&2
            exit 1
        fi
        echo "Rust not ready (${attempt}/${HEALTH_ATTEMPTS}); retrying in ${HEALTH_RETRY_DELAY}s..."
        sleep "$HEALTH_RETRY_DELAY"
    done
    echo "Rust service is healthy: $RUST_HEALTH_URL"
fi
# Preserve the public-surface gate as well as the native loopback gate.
for ((attempt = 1; attempt <= HEALTH_ATTEMPTS; attempt++)); do
    if readiness_exact "$HEALTH_URL"; then break; fi
    if [[ "$attempt" == "$HEALTH_ATTEMPTS" ]]; then
        echo "Public readiness failed: $HEALTH_URL" >&2; exit 1
    fi
    sleep "$HEALTH_RETRY_DELAY"
done
# The rehearsal/window producer signs fresh acceptance for this nonce. These
# identifiers are not credentials; the signer keeps its private key off-host.
echo "DEPLOY_ACCEPTANCE_NONCE=$DEPLOY_NONCE"
echo "DEPLOY_ACCEPTANCE_BINARY_SHA256=$(sha256sum -- "$ACTIVE_BINARY" | cut -d ' ' -f 1)"
# Exact readiness alone does not establish auth/session or passive safety.
for ((attempt = 1; attempt <= HEALTH_ATTEMPTS; attempt++)); do
    if verify_window_acceptance; then break; fi
    if [[ "$attempt" == "$HEALTH_ATTEMPTS" ]]; then
        echo "Signed auth/session/monitor/passive acceptance is missing or mismatched; canonical retry required." >&2
        exit 1
    fi
    sleep "$HEALTH_RETRY_DELAY"
done
if retained_backend_installed \
    && { systemctl is-active --quiet "$SERVICE_NAME" \
        || systemctl is-enabled --quiet "$SERVICE_NAME"; }; then
    echo "Retained backend must remain stopped and disabled." >&2; exit 1
fi
# Both surfaces and signed acceptance passed; preserve selection on reboot.
sudo -n systemctl enable "$RUST_UNIT_NAME"
publication_boundary before clear
clear_deploy_state
publication_boundary after clear
echo "=== Backend Deployment Complete ==="
DEPLOY_STARTED=0
cleanup_acl_convergence_bundle
cleanup_migration_policy_bundle
cleanup_rust_policy_bundle
cleanup_rust_extract
trap - EXIT

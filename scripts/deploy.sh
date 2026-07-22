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
        DEFAULT_BRANCH="prod"
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
HEALTH_ATTEMPTS="${HEALTH_ATTEMPTS:-12}"
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
DEPLOY_STATE_FILE="$DEPLOY_STATE_DIR/${DEPLOY_ENVIRONMENT}.state"
APPROVED_RELEASE_SHA="${APPROVED_RELEASE_SHA:-}"
MIGRATION_APPROVED_SOURCE_SHA="${MIGRATION_APPROVED_SOURCE_SHA:-}"
MIGRATION_EXPECTED_CURRENT_REVISION="${MIGRATION_EXPECTED_CURRENT_REVISION:-}"
MIGRATION_TARGET_REVISION="${MIGRATION_TARGET_REVISION:-}"
DEPLOY_LOCK_FD=""
DEPLOY_STARTED=0
SERVICE_RESTART_ATTEMPTED=0
DEPLOY_PHASE="preflight"
DRY_RUN_TEMP=""
ACL_CONVERGENCE_TEMP=""
MIGRATION_POLICY_TEMP=""

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

acquire_deploy_lock() {
    mkdir -p -- "$DEPLOY_STATE_DIR"
    chmod 0755 "$DEPLOY_STATE_DIR"
    exec {DEPLOY_LOCK_FD}>"$DEPLOY_LOCK_FILE"
    if ! flock -n "$DEPLOY_LOCK_FD"; then
        echo "A $DEPLOY_ENVIRONMENT deployment is already running; refusing concurrent deploy." >&2
        exit 1
    fi
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
}

write_deploy_state() {
    local phase="$1"
    local release_sha="${2:-${CURRENT_SHA:-$APPROVED_RELEASE_SHA}}"
    local temporary_file="${DEPLOY_STATE_FILE}.tmp.$$"

    umask 022
    printf 'DEPLOYMENT_STATE=%s\nENVIRONMENT=%s\nRELEASE_SHA=%s\n' \
        "$phase" "$DEPLOY_ENVIRONMENT" "$release_sha" > "$temporary_file"
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
    if [[ "$status" != "0" && "$DRY_RUN" == "0" && "$DEPLOY_STARTED" == "1" ]]; then
        if ! write_deploy_state "blocked" "${CURRENT_SHA:-$APPROVED_RELEASE_SHA}"; then
            echo "WARNING: could not restore durable deployment state." >&2
        fi
        if [[ "$SERVICE_RESTART_ATTEMPTED" == "1" ]] \
            && ! sudo systemctl stop "$SERVICE_NAME"; then
            echo "WARNING: could not stop the failed backend service." >&2
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
    "${GIT[@]}" cat-file -e "$REMOTE_SHA:$UNIT_MANIFEST_PATH"
    "${GIT[@]}" cat-file -e "$REMOTE_SHA:$MIGRATION_POLICY_PATH"
    "${GIT[@]}" cat-file -e "$REMOTE_SHA:$ACL_POLICY_PATH"
    "${GIT[@]}" cat-file -e "$REMOTE_SHA:$ACL_CONVERGENCE_SQL_PATH"
    "${GIT[@]}" cat-file -e "$REMOTE_SHA:$ACL_CONVERGENCE_HELPER_PATH"
    "${GIT[@]}" archive --format=tar "$REMOTE_SHA" \
        "$UNIT_MANIFEST_PATH" "$MIGRATION_POLICY_PATH" "$ACL_POLICY_PATH" \
        "$ACL_CONVERGENCE_SQL_PATH" "$ACL_CONVERGENCE_HELPER_PATH" \
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
    load_unit_manifest "$DRY_RUN_TEMP/candidate-units/$UNIT_MANIFEST_PATH"
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
acquire_deploy_lock
CURRENT_SHA="$("${GIT[@]}" rev-parse HEAD)"
"${PYTHON_ENV[@]}" ENVIRONMENT="$DEPLOY_ENVIRONMENT" RELEASE_SHA="$CURRENT_SHA" \
    ./venv/bin/python scripts/verify_migration_revision.py \
    --expected "$MIGRATION_EXPECTED_CURRENT_REVISION"
cleanup_migration_policy_bundle

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
    "${PYTHON_ENV[@]}" PIP_CONFIG_FILE=/dev/null \
        ./venv/bin/python -m pip install -r requirements.txt -c constraints.txt
    "${PYTHON_ENV[@]}" PIP_CONFIG_FILE=/dev/null \
        ./venv/bin/python -m pip check
fi

"${PYTHON_ENV[@]}" ENVIRONMENT="$DEPLOY_ENVIRONMENT" RELEASE_SHA="$CURRENT_SHA" \
    ./venv/bin/python scripts/apply_migration_checkpoint.py \
    --source-root "$BACKEND_DIR" \
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

assert_clean_tree "Deploy steps"
DEPLOY_PHASE="restart-authorized"
write_deploy_state "$DEPLOY_PHASE" "$CURRENT_SHA"
SERVICE_RESTART_ATTEMPTED=1
sudo systemctl restart "$SERVICE_NAME"
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
clear_deploy_state
echo "=== Backend Deployment Complete ==="
DEPLOY_STARTED=0
cleanup_acl_convergence_bundle
cleanup_migration_policy_bundle
trap - EXIT

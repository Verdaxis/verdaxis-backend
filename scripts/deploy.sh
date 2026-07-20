#!/usr/bin/env bash
# Systemd-aware deploy helper for the Verdaxis backend.
#
# Dry-run is an immutable source/configuration inspection only. It never
# executes candidate application code or supplies candidate code with live
# secrets, home, database, agent, or network access.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BACKEND_DIR="$(dirname "$SCRIPT_DIR")"
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
export GIT_NO_REPLACE_OBJECTS=1
GIT=(git -c "safe.directory=$BACKEND_DIR" -c core.hooksPath=/dev/null)

case "$BACKEND_DIR" in
    */prod/be)
        DEFAULT_BRANCH="prod"
        DEPLOY_ENVIRONMENT="production"
        SERVICE_NAME="verdaxis-backend.service"
        HEALTH_URL="https://api.verdaxis.exchange/health/ready"
        UNIT_NAMES=(
            verdaxis-backend.service
            verdaxis-news-refresh.service
            verdaxis-news-refresh.timer
            verdaxis-product-analytics-prune.service
            verdaxis-product-analytics-prune.timer
        )
        ;;
    */staging/be)
        DEFAULT_BRANCH="staging"
        DEPLOY_ENVIRONMENT="staging"
        SERVICE_NAME="verdaxis-backend-staging.service"
        HEALTH_URL="https://api-staging.verdaxis.exchange/health/ready"
        UNIT_NAMES=(
            verdaxis-backend-staging.service
            verdaxis-news-refresh-staging.service
            verdaxis-news-refresh-staging.timer
            verdaxis-product-analytics-prune-staging.service
            verdaxis-product-analytics-prune-staging.timer
        )
        ;;
    *)
        echo "Cannot infer backend environment from path: $BACKEND_DIR" >&2
        echo "Set TARGET_BRANCH, SERVICE_NAME, and HEALTH_URL explicitly." >&2
        : "${TARGET_BRANCH:?TARGET_BRANCH is required outside prod/staging layout}"
        : "${SERVICE_NAME:?SERVICE_NAME is required outside prod/staging layout}"
        : "${HEALTH_URL:?HEALTH_URL is required outside prod/staging layout}"
        : "${DEPLOY_ENVIRONMENT:?DEPLOY_ENVIRONMENT is required outside prod/staging layout}"
        DEFAULT_BRANCH="$TARGET_BRANCH"
        UNIT_NAMES=()
        ;;
esac

TARGET_BRANCH="${TARGET_BRANCH:-$DEFAULT_BRANCH}"
if [[ "${#UNIT_NAMES[@]}" == 0 ]]; then
    case "$DEPLOY_ENVIRONMENT" in
        production)
            UNIT_NAMES=(
                verdaxis-backend.service
                verdaxis-news-refresh.service
                verdaxis-news-refresh.timer
                verdaxis-product-analytics-prune.service
                verdaxis-product-analytics-prune.timer
            )
            ;;
        staging)
            UNIT_NAMES=(
                verdaxis-backend-staging.service
                verdaxis-news-refresh-staging.service
                verdaxis-news-refresh-staging.timer
                verdaxis-product-analytics-prune-staging.service
                verdaxis-product-analytics-prune-staging.timer
            )
            ;;
        *)
            echo "DEPLOY_ENVIRONMENT must be production or staging." >&2
            exit 2
            ;;
    esac
fi
RELEASE_ENV_FILE="$BACKEND_DIR/.runtime-release.env"
DEPLOY_STATE_DIR="${DEPLOY_STATE_DIR:-$BACKEND_DIR/.runtime-deploy}"
DEPLOY_LOCK_FILE="$DEPLOY_STATE_DIR/${DEPLOY_ENVIRONMENT}.lock"
DEPLOY_STATE_FILE="$DEPLOY_STATE_DIR/${DEPLOY_ENVIRONMENT}.state"
APPROVED_RELEASE_SHA="${APPROVED_RELEASE_SHA:-}"
DEPLOY_LOCK_FD=""
DEPLOY_STARTED=0
SERVICE_RESTART_ATTEMPTED=0
DEPLOY_PHASE="preflight"
DRY_RUN_TEMP=""

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
    local temporary_file="${RELEASE_ENV_FILE}.tmp.$$"

    echo "+ write immutable release metadata to $RELEASE_ENV_FILE"
    umask 077
    printf 'ENVIRONMENT=%s\nRELEASE_SHA=%s\n' \
        "$DEPLOY_ENVIRONMENT" "$release_sha" > "$temporary_file"
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

preserve_failed_release() {
    local status="$?"
    trap - EXIT
    cleanup_dry_run_candidate
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
    git -c core.hooksPath=/dev/null -C "$BACKEND_DIR" fetch --quiet --no-tags \
        origin "$REMOTE_SHA"
    git -c core.hooksPath=/dev/null -C "$BACKEND_DIR" cat-file -e "$REMOTE_SHA^{commit}"
    mkdir -m 0700 "$DRY_RUN_TEMP/candidate-units"
    ARCHIVE_PATHS=()
    CANDIDATE_UNITS=()
    for unit_name in "${UNIT_NAMES[@]}"; do
        relative="deploy/systemd/$unit_name"
        "${GIT[@]}" cat-file -e "$REMOTE_SHA:$relative"
        ARCHIVE_PATHS+=("$relative")
        CANDIDATE_UNITS+=("$DRY_RUN_TEMP/candidate-units/$relative")
    done
    "${GIT[@]}" archive --format=tar "$REMOTE_SHA" "${ARCHIVE_PATHS[@]}" \
        | tar --extract --file=- --directory="$DRY_RUN_TEMP/candidate-units"
    sha256sum "${CANDIDATE_UNITS[@]}" > "$DRY_RUN_TEMP/SHA256SUMS"
    systemd-analyze verify "${CANDIDATE_UNITS[@]}"
    assert_clean_tree "Dry-run checks"
    cleanup_dry_run_candidate

    echo "Dry-run checks passed for immutable candidate SHA $REMOTE_SHA."
    echo "APPROVED_RELEASE_SHA=$REMOTE_SHA"
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

acquire_deploy_lock
assert_clean_tree "Initial deploy preflight"
if [[ ! "$APPROVED_RELEASE_SHA" =~ ^[0-9a-f]{40}$ ]]; then
    echo "APPROVED_RELEASE_SHA must be the exact full SHA printed by a prior dry-run." >&2
    exit 1
fi
DEPLOY_STARTED=1
write_deploy_state "blocked" "$APPROVED_RELEASE_SHA"

"${GIT[@]}" fetch origin "refs/heads/$TARGET_BRANCH"
"${GIT[@]}" checkout "$TARGET_BRANCH"
"${GIT[@]}" merge --ff-only FETCH_HEAD

CURRENT_BRANCH="$("${GIT[@]}" branch --show-current)"
CURRENT_SHA="$("${GIT[@]}" rev-parse HEAD)"
REMOTE_SHA="$("${GIT[@]}" rev-parse 'FETCH_HEAD^{commit}')"
if [[ "$REMOTE_SHA" != "$APPROVED_RELEASE_SHA" ]]; then
    echo "Remote target moved after approval; refusing to deploy a different SHA." >&2
    exit 1
fi
if [[ "$CURRENT_BRANCH" != "$TARGET_BRANCH" \
    || ! "$CURRENT_SHA" =~ ^[0-9a-f]{40}$ \
    || "$CURRENT_SHA" != "$REMOTE_SHA" ]]; then
    echo "Selected source is not the exact full-SHA remote target release." >&2
    exit 1
fi
assert_clean_tree "Selected release"

# Publish the selected tree's exact identity before invoking any executable
# from that tree. The durable state lets units remain fail-closed during this
# transition and through any restart/readiness failure.
write_release_artifact "$CURRENT_SHA"

env ENVIRONMENT="$DEPLOY_ENVIRONMENT" RELEASE_SHA="$CURRENT_SHA" \
    ./venv/bin/python scripts/preflight_runtime.py \
    --environment "$DEPLOY_ENVIRONMENT" --release-sha "$CURRENT_SHA"

if [[ -f requirements.txt ]]; then
    ./venv/bin/python -m pip install -r requirements.txt
    ./venv/bin/python -m pip check
fi

if [[ -f alembic.ini ]]; then
    env ENVIRONMENT="$DEPLOY_ENVIRONMENT" RELEASE_SHA="$CURRENT_SHA" \
        ./venv/bin/alembic upgrade head
fi

assert_clean_tree "Deploy steps"
DEPLOY_PHASE="restart-authorized"
write_deploy_state "$DEPLOY_PHASE" "$CURRENT_SHA"
SERVICE_RESTART_ATTEMPTED=1
sudo systemctl restart "$SERVICE_NAME"
DEPLOY_PHASE="readiness-pending"
write_deploy_state "$DEPLOY_PHASE" "$CURRENT_SHA"

systemctl is-active --quiet "$SERVICE_NAME"
HEALTH_ATTEMPTS="${HEALTH_ATTEMPTS:-12}"
HEALTH_RETRY_DELAY="${HEALTH_RETRY_DELAY:-2}"
for ((attempt = 1; attempt <= HEALTH_ATTEMPTS; attempt++)); do
    if HEALTH_PAYLOAD="$(curl --fail --silent --show-error --max-time 15 "$HEALTH_URL")"; then
        if printf '%s' "$HEALTH_PAYLOAD" | ./venv/bin/python \
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
trap - EXIT

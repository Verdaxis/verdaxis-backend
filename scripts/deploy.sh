#!/usr/bin/env bash
# Systemd-aware deploy helper for the Verdaxis backend.
#
# Defaults are inferred from the live VPS layout:
#   /home/verdaxis-prod/verdaxis/prod/be     -> branch prod, service verdaxis-backend.service
#   /home/verdaxis-prod/verdaxis/staging/be  -> branch staging, service verdaxis-backend-staging.service
#
# Usage:
#   ./scripts/deploy.sh --dry-run
#   ./scripts/deploy.sh
# Dirty trees are always refused because a commit SHA cannot identify modified
# source. Rollbacks are forward-only revert releases; see docs/runtime-hardening.md.
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
GIT=(git -c "safe.directory=$BACKEND_DIR" -c core.hooksPath=/dev/null)

case "$BACKEND_DIR" in
    */prod/be)
        DEFAULT_BRANCH="prod"
        DEPLOY_ENVIRONMENT="production"
        SERVICE_NAME="verdaxis-backend.service"
        HEALTH_URL="https://api.verdaxis.exchange/health/ready"
        ;;
    */staging/be)
        DEFAULT_BRANCH="staging"
        DEPLOY_ENVIRONMENT="staging"
        SERVICE_NAME="verdaxis-backend-staging.service"
        HEALTH_URL="https://api-staging.verdaxis.exchange/health/ready"
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

TARGET_BRANCH="${TARGET_BRANCH:-$DEFAULT_BRANCH}"
RELEASE_ENV_FILE="$BACKEND_DIR/.runtime-release.env"
DEPLOY_GUARD_FILE="$BACKEND_DIR/.runtime-deploying"
DEPLOY_STARTED=0
RELEASE_IDENTITY_PUBLISHED=0
SERVICE_RESTART_ATTEMPTED=0
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

write_deploy_guard() {
    local temporary_file="${DEPLOY_GUARD_FILE}.tmp.$$"

    umask 077
    printf 'DEPLOYMENT_STATE=blocked\nENVIRONMENT=%s\n' \
        "$DEPLOY_ENVIRONMENT" > "$temporary_file"
    chmod 0600 "$temporary_file"
    mv -- "$temporary_file" "$DEPLOY_GUARD_FILE"
}

clear_deploy_guard() {
    rm -f -- "$DEPLOY_GUARD_FILE"
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
        if ! write_deploy_guard; then
            echo "WARNING: could not restore the deployment guard." >&2
        fi
        if [[ "$SERVICE_RESTART_ATTEMPTED" == "1" ]] \
            && ! sudo systemctl stop "$SERVICE_NAME"; then
            echo "WARNING: could not stop the failed backend service." >&2
        fi
        if [[ "$RELEASE_IDENTITY_PUBLISHED" == "1" ]]; then
            echo "Deployment failed; selected release remains fail-closed with code and published identity aligned." >&2
        else
            echo "Deployment failed before release identity publication; unit starts remain fail-closed." >&2
        fi
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

    CURRENT_SHA="$("${GIT[@]}" rev-parse HEAD)"
    if [[ ! "$CURRENT_SHA" =~ ^[0-9a-f]{40}$ ]]; then
        echo "Resolved release identity is not a full commit SHA." >&2
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
    CANDIDATE_REPOSITORY="$DRY_RUN_TEMP/repository"
    CANDIDATE_TREE="$DRY_RUN_TEMP/candidate"
    ORIGIN_URL="$("${GIT[@]}" remote get-url origin)"
    git -c core.hooksPath=/dev/null clone --quiet --no-checkout --depth 1 \
        --single-branch --branch "$TARGET_BRANCH" -- \
        "$ORIGIN_URL" "$CANDIDATE_REPOSITORY"
    CANDIDATE_SHA="$(git -c core.hooksPath=/dev/null \
        -C "$CANDIDATE_REPOSITORY" rev-parse HEAD)"
    if [[ "$CANDIDATE_SHA" != "$REMOTE_SHA" ]]; then
        echo "Materialized candidate does not match the attested remote SHA." >&2
        exit 1
    fi
    mkdir -m 0700 "$CANDIDATE_TREE"
    git -c core.hooksPath=/dev/null -C "$CANDIDATE_REPOSITORY" \
        archive --format=tar "$CANDIDATE_SHA" \
        | tar --extract --file=- --directory="$CANDIDATE_TREE"
    if [[ -f "$BACKEND_DIR/.env" ]]; then
        ln -s "$BACKEND_DIR/.env" "$CANDIDATE_TREE/.env"
    fi

    (
        cd "$CANDIDATE_TREE"
        env ENVIRONMENT="$DEPLOY_ENVIRONMENT" RELEASE_SHA="$CANDIDATE_SHA" \
            "$BACKEND_DIR/venv/bin/python" scripts/preflight_runtime.py \
            --environment "$DEPLOY_ENVIRONMENT" --release-sha "$CANDIDATE_SHA"
        if [[ -f requirements.txt ]]; then
            "$BACKEND_DIR/venv/bin/python" -m pip install --dry-run \
                --no-cache-dir -r requirements.txt
            "$BACKEND_DIR/venv/bin/python" -m pip check
        fi
        if [[ -f alembic.ini ]]; then
            env ENVIRONMENT="$DEPLOY_ENVIRONMENT" RELEASE_SHA="$CANDIDATE_SHA" \
                "$BACKEND_DIR/venv/bin/alembic" current --check-heads
        fi
    )
    assert_clean_tree "Dry-run checks"
    cleanup_dry_run_candidate

    echo "Dry-run checks passed for the exact clean remote release."
    echo "Skipped mutations:"
    echo "  - source update (the remote candidate was staged privately instead)"
    echo "  - dependency installation (resolver dry-run and pip check ran)"
    echo "  - migration upgrade (Alembic current --check-heads ran)"
    echo "  - release identity and deployment guard publication"
    echo "  - service restart"
    echo "  - health gate (valid only after the candidate service restarts)"
    exit 0
fi

assert_clean_tree "Initial deploy preflight"
DEPLOY_STARTED=1
write_deploy_guard

"${GIT[@]}" fetch origin "refs/heads/$TARGET_BRANCH"
"${GIT[@]}" checkout "$TARGET_BRANCH"
"${GIT[@]}" merge --ff-only FETCH_HEAD

CURRENT_BRANCH="$("${GIT[@]}" branch --show-current)"
CURRENT_SHA="$("${GIT[@]}" rev-parse HEAD)"
REMOTE_SHA="$("${GIT[@]}" rev-parse 'FETCH_HEAD^{commit}')"
if [[ "$CURRENT_BRANCH" != "$TARGET_BRANCH" \
    || ! "$CURRENT_SHA" =~ ^[0-9a-f]{40}$ \
    || "$CURRENT_SHA" != "$REMOTE_SHA" ]]; then
    echo "Selected source is not the exact full-SHA remote target release." >&2
    exit 1
fi
assert_clean_tree "Selected release"

# Publish the selected tree's exact identity before invoking any executable
# from that tree. Installed runtime units refuse to start while the guard is
# present, so publication failure cannot start mismatched bytes.
write_release_artifact "$CURRENT_SHA"
RELEASE_IDENTITY_PUBLISHED=1

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
clear_deploy_guard
SERVICE_RESTART_ATTEMPTED=1
sudo systemctl restart "$SERVICE_NAME"

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
echo "=== Backend Deployment Complete ==="
DEPLOY_STARTED=0
trap - EXIT

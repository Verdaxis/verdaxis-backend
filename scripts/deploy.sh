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

if [[ "${1:-}" == "--dry-run" ]]; then
    DRY_RUN=1
fi

cd "$BACKEND_DIR"
GIT=(git -c "safe.directory=$BACKEND_DIR")

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

run() {
    echo "+ $*"
    if [[ "$DRY_RUN" == "0" ]]; then
        "$@"
    fi
}

write_release_artifact() {
    local release_sha="$1"
    local temporary_file="${RELEASE_ENV_FILE}.tmp.$$"

    echo "+ write immutable release metadata to $RELEASE_ENV_FILE"
    if [[ "$DRY_RUN" == "1" ]]; then
        return
    fi
    umask 077
    printf 'ENVIRONMENT=%s\nRELEASE_SHA=%s\n' \
        "$DEPLOY_ENVIRONMENT" "$release_sha" > "$temporary_file"
    chmod 0600 "$temporary_file"
    mv -- "$temporary_file" "$RELEASE_ENV_FILE"
}

echo "=== Verdaxis Backend Deployment ==="
echo "Timestamp: $(date)"
echo "Directory: $BACKEND_DIR"
echo "Current branch: $("${GIT[@]}" branch --show-current)"
echo "Current SHA: $("${GIT[@]}" rev-parse --short HEAD)"
echo "Target branch: $TARGET_BRANCH"
echo "Service: $SERVICE_NAME"
echo "Health URL: $HEALTH_URL"
echo "Dry run: $DRY_RUN"

if [[ -n "$("${GIT[@]}" status --porcelain)" ]]; then
    echo "Working tree is dirty; refusing to publish a mismatched release SHA." >&2
    "${GIT[@]}" status --short >&2
    exit 1
fi

run "${GIT[@]}" fetch origin "$TARGET_BRANCH"
run "${GIT[@]}" checkout "$TARGET_BRANCH"
run "${GIT[@]}" pull --ff-only origin "$TARGET_BRANCH"

CURRENT_SHA="$("${GIT[@]}" rev-parse HEAD)"
if [[ ! "$CURRENT_SHA" =~ ^[0-9a-f]{40}$ ]]; then
    echo "Resolved release identity is not a full commit SHA." >&2
    exit 1
fi

run env ENVIRONMENT="$DEPLOY_ENVIRONMENT" RELEASE_SHA="$CURRENT_SHA" \
    ./venv/bin/python scripts/preflight_runtime.py \
    --environment "$DEPLOY_ENVIRONMENT" --release-sha "$CURRENT_SHA"

if [[ -f requirements.txt ]]; then
    run ./venv/bin/python -m pip install -r requirements.txt
fi

if [[ -f alembic.ini ]]; then
    run env ENVIRONMENT="$DEPLOY_ENVIRONMENT" RELEASE_SHA="$CURRENT_SHA" \
        ./venv/bin/alembic upgrade head
fi

if [[ -n "$("${GIT[@]}" status --porcelain)" ]]; then
    echo "Deploy steps changed tracked source; refusing a mismatched release artifact." >&2
    "${GIT[@]}" status --short >&2
    exit 1
fi
write_release_artifact "$CURRENT_SHA"
run sudo systemctl restart "$SERVICE_NAME"

if [[ "$DRY_RUN" == "1" ]]; then
    echo "Dry run complete; service was not restarted."
    exit 0
fi

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

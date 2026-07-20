#!/usr/bin/env bash
# Install one environment's units only from its exact approved clean checkout.
# The script deliberately never enables, starts, or restarts a service/timer.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BACKEND_ROOT="$(dirname "$SCRIPT_DIR")"
SYSTEMD_DIR="/etc/systemd/system"
MODE="dry-run"
MODE_SET=0
DEPLOY_ENVIRONMENT=""
SOURCE_REF=""

usage() {
    echo "usage: $0 [--dry-run|--apply] --environment production|staging --source-ref <40-hex-sha>" >&2
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run|--apply)
            if [[ "$MODE_SET" == 1 ]]; then
                usage
                exit 2
            fi
            MODE="${1#--}"
            MODE_SET=1
            shift
            ;;
        --environment)
            [[ $# -ge 2 && -z "$DEPLOY_ENVIRONMENT" ]] || { usage; exit 2; }
            DEPLOY_ENVIRONMENT="$2"
            shift 2
            ;;
        --source-ref)
            [[ $# -ge 2 && -z "$SOURCE_REF" ]] || { usage; exit 2; }
            SOURCE_REF="$2"
            shift 2
            ;;
        *)
            usage
            exit 2
            ;;
    esac
done

if [[ ! "$SOURCE_REF" =~ ^[0-9a-f]{40}$ ]]; then
    echo "--source-ref must be a full lowercase commit SHA" >&2
    exit 2
fi

case "$DEPLOY_ENVIRONMENT" in
    production)
        SOURCE_ROOT="/home/verdaxis-prod/verdaxis/prod/be"
        UNIT_NAMES=(
            verdaxis-backend.service
            verdaxis-news-refresh.service
            verdaxis-news-refresh.timer
        )
        ;;
    staging)
        SOURCE_ROOT="/home/verdaxis-prod/verdaxis/staging/be"
        UNIT_NAMES=(
            verdaxis-backend-staging.service
            verdaxis-news-refresh-staging.service
            verdaxis-news-refresh-staging.timer
        )
        ;;
    *)
        usage
        exit 2
        ;;
esac

if [[ "$(realpath -e "$BACKEND_ROOT")" != "$(realpath -e "$SOURCE_ROOT")" ]]; then
    echo "installer must run from the selected environment's fixed deploy checkout" >&2
    exit 1
fi
if [[ ! -x "$SOURCE_ROOT/venv/bin/python" || ! -x "$SOURCE_ROOT/venv/bin/alembic" ]]; then
    echo "selected deploy checkout is missing its virtualenv" >&2
    exit 1
fi

UNIT_PATH_ARGS=()
UNIT_SOURCES=()
for unit_name in "${UNIT_NAMES[@]}"; do
    UNIT_PATH_ARGS+=(--unit "deploy/systemd/$unit_name")
    UNIT_SOURCES+=("$SOURCE_ROOT/deploy/systemd/$unit_name")
done

"$SOURCE_ROOT/venv/bin/python" "$SOURCE_ROOT/scripts/verify_systemd_source.py" \
    --source-root "$SOURCE_ROOT" \
    --source-ref "$SOURCE_REF" \
    --mode "$MODE" \
    "${UNIT_PATH_ARGS[@]}"

(
    cd "$SOURCE_ROOT"
    env ENVIRONMENT="$DEPLOY_ENVIRONMENT" RELEASE_SHA="$SOURCE_REF" \
        ./venv/bin/python scripts/preflight_runtime.py \
        --environment "$DEPLOY_ENVIRONMENT" --release-sha "$SOURCE_REF"
    env ENVIRONMENT="$DEPLOY_ENVIRONMENT" RELEASE_SHA="$SOURCE_REF" \
        ./venv/bin/alembic current --check-heads
)
systemd-analyze verify "${UNIT_SOURCES[@]}"

echo "Verdaxis systemd unit installation mode: $MODE"
echo "Environment: $DEPLOY_ENVIRONMENT"
echo "Approved source ref: $SOURCE_REF"
if [[ "$MODE" == "dry-run" ]]; then
    echo "Provenance and preflights passed; no unit files were changed."
    exit 0
fi

UNITS_CHANGED=0
for index in "${!UNIT_NAMES[@]}"; do
    unit_name="${UNIT_NAMES[$index]}"
    source="${UNIT_SOURCES[$index]}"
    destination="$SYSTEMD_DIR/$unit_name"
    if sudo cmp -s "$source" "$destination"; then
        echo "$unit_name is already current"
        continue
    fi
    sudo install -m 0644 -o root -g root "$source" "$destination"
    UNITS_CHANGED=1
done
if [[ "$UNITS_CHANGED" == 1 ]]; then
    sudo systemctl daemon-reload
fi
echo "Unit files installed; services and timers were left unchanged."

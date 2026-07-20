#!/usr/bin/env bash
# Install one environment's units only from its exact approved clean checkout.
# The script deliberately never enables, starts, or restarts a service/timer.
set -euo pipefail
export GIT_NO_REPLACE_OBJECTS=1

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
            verdaxis-product-analytics-prune.service
            verdaxis-product-analytics-prune.timer
        )
        ;;
    staging)
        SOURCE_ROOT="/home/verdaxis-prod/verdaxis/staging/be"
        UNIT_NAMES=(
            verdaxis-backend-staging.service
            verdaxis-news-refresh-staging.service
            verdaxis-news-refresh-staging.timer
            verdaxis-product-analytics-prune-staging.service
            verdaxis-product-analytics-prune-staging.timer
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

SOURCE_GIT=(git -C "$SOURCE_ROOT" -c "safe.directory=$SOURCE_ROOT" -c core.hooksPath=/dev/null)
if [[ "$("${SOURCE_GIT[@]}" rev-parse --show-toplevel)" != "$SOURCE_ROOT" \
    || "$("${SOURCE_GIT[@]}" rev-parse HEAD)" != "$SOURCE_REF" ]]; then
    echo "selected deploy checkout is not the exact approved source ref" >&2
    exit 1
fi
if [[ -n "$("${SOURCE_GIT[@]}" replace -l)" \
    || -n "$("${SOURCE_GIT[@]}" status --porcelain --untracked-files=all)" ]]; then
    echo "selected deploy checkout is dirty or contains replacement refs" >&2
    exit 1
fi
if "${SOURCE_GIT[@]} ls-files -v --" | awk '$1 ~ /^[a-zS]$/ { found=1 } END { exit found ? 0 : 1 }'; then
    echo "selected deploy checkout contains hidden tracked-file index flags" >&2
    exit 1
fi

STAGING_DIR="$(sudo mktemp -d /run/verdaxis-systemd-units.XXXXXXXX)"
case "$STAGING_DIR" in
    /run/verdaxis-systemd-units.*) ;;
    *)
        echo "refusing unexpected systemd staging path" >&2
        exit 1
        ;;
esac
cleanup_staging() {
    case "$STAGING_DIR" in
        /run/verdaxis-systemd-units.*)
            sudo rm -rf -- "$STAGING_DIR"
            ;;
        *)
            echo "refusing unsafe systemd staging cleanup path" >&2
            ;;
    esac
}
trap cleanup_staging EXIT
sudo chmod 0700 "$STAGING_DIR"
if [[ "$(sudo stat -c '%u:%g:%a' "$STAGING_DIR")" != "0:0:700" ]]; then
    echo "systemd staging directory must be private and root-owned" >&2
    exit 1
fi

ARCHIVE_PATHS=()
for unit_name in "${UNIT_NAMES[@]}"; do
    relative="deploy/systemd/$unit_name"
    "${SOURCE_GIT[@]}" cat-file -e "$SOURCE_REF:$relative"
    ARCHIVE_PATHS+=("$relative")
done
"${SOURCE_GIT[@]}" archive --format=tar "$SOURCE_REF" "${ARCHIVE_PATHS[@]}" \
    | sudo tar --extract --file=- --directory="$STAGING_DIR"
sudo /bin/sh -c 'cd "$1" && shift && /usr/bin/sha256sum "$@" > SHA256SUMS' \
    systemd-unit-digest-manifest "$STAGING_DIR" "${ARCHIVE_PATHS[@]}"
sudo /bin/sh -c 'cd "$1" && /usr/bin/sha256sum --check SHA256SUMS' \
    systemd-unit-digest-check "$STAGING_DIR"

UNIT_SOURCES=()
for unit_name in "${UNIT_NAMES[@]}"; do
    UNIT_SOURCES+=("$STAGING_DIR/deploy/systemd/$unit_name")
done
sudo systemd-analyze verify "${UNIT_SOURCES[@]}"

echo "Verdaxis systemd unit installation mode: $MODE"
echo "Environment: $DEPLOY_ENVIRONMENT"
echo "Approved source ref: $SOURCE_REF"
if [[ "$MODE" == "dry-run" ]]; then
    echo "Immutable unit provenance and trusted systemd syntax checks passed; no unit files were changed."
    exit 0
fi

PENDING_DIR="${INSTALL_PENDING_DIR:-/var/lib/verdaxis/systemd-units}"
PENDING_STATE="$PENDING_DIR/${DEPLOY_ENVIRONMENT}.pending"
sudo install -d -m 0750 -o root -g root "$PENDING_DIR"
printf 'ENVIRONMENT=%s\nSOURCE_REF=%s\n' "$DEPLOY_ENVIRONMENT" "$SOURCE_REF" \
    | sudo tee "${PENDING_STATE}.tmp.$$" >/dev/null
sudo chmod 0644 "${PENDING_STATE}.tmp.$$"
sudo mv -- "${PENDING_STATE}.tmp.$$" "$PENDING_STATE"

for index in "${!UNIT_NAMES[@]}"; do
    unit_name="${UNIT_NAMES[$index]}"
    source="${UNIT_SOURCES[$index]}"
    destination="$SYSTEMD_DIR/$unit_name"
    if sudo cmp -s "$source" "$destination"; then
        echo "$unit_name is already current"
        continue
    fi
    sudo install -m 0644 -o root -g root "$source" "$destination"
done
sudo systemctl daemon-reload
sudo rm -f -- "$PENDING_STATE"
echo "Unit files installed; services and timers were left unchanged."

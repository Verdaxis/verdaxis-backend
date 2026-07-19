#!/usr/bin/env bash
# Idempotently install checked-in Verdaxis backend units after read-only
# configuration, database-identity, and Alembic-head preflights. This script
# deliberately does not enable, start, or restart either service.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BACKEND_ROOT="$(dirname "$SCRIPT_DIR")"
SYSTEMD_DIR="/etc/systemd/system"
APPLY=0

if [[ "${1:-}" == "--apply" ]]; then
    APPLY=1
elif [[ -n "${1:-}" && "${1:-}" != "--dry-run" ]]; then
    echo "usage: $0 [--dry-run|--apply]" >&2
    exit 2
fi

preflight_backend() {
    local environment="$1"
    local backend="$2"
    local release_sha

    if [[ ! -d "$backend/.git" || ! -x "$backend/venv/bin/python" ]]; then
        echo "missing deploy checkout or virtualenv for $environment" >&2
        exit 1
    fi
    if [[ -n "$(git -C "$backend" status --porcelain)" ]]; then
        echo "$environment deploy checkout is dirty; refusing unit installation" >&2
        exit 1
    fi
    release_sha="$(git -C "$backend" rev-parse HEAD)"
    if [[ ! "$release_sha" =~ ^[0-9a-f]{40}$ ]]; then
        echo "$environment checkout does not resolve to a full release SHA" >&2
        exit 1
    fi

    (
        cd "$backend"
        env ENVIRONMENT="$environment" RELEASE_SHA="$release_sha" \
            ./venv/bin/python scripts/preflight_runtime.py \
            --environment "$environment" --release-sha "$release_sha"
        env ENVIRONMENT="$environment" RELEASE_SHA="$release_sha" \
            ./venv/bin/alembic current --check-heads
    )
}

install_unit() {
    local unit_name="$1"
    local source="$BACKEND_ROOT/deploy/systemd/$unit_name"
    local destination="$SYSTEMD_DIR/$unit_name"

    sudo systemd-analyze verify "$source"
    if sudo cmp -s "$source" "$destination"; then
        echo "$unit_name is already current"
        return
    fi
    sudo install -m 0644 -o root -g root "$source" "$destination"
    UNITS_CHANGED=1
}

echo "Verdaxis systemd unit installation mode: $([[ "$APPLY" == 1 ]] && echo apply || echo dry-run)"
if [[ "$APPLY" == 0 ]]; then
    echo "Would preflight production and staging, verify both units, copy changed units, and reload systemd."
    exit 0
fi

preflight_backend production /home/verdaxis-prod/verdaxis/prod/be
preflight_backend staging /home/verdaxis-prod/verdaxis/staging/be

UNITS_CHANGED=0
install_unit verdaxis-backend.service
install_unit verdaxis-backend-staging.service
if [[ "$UNITS_CHANGED" == 1 ]]; then
    sudo systemctl daemon-reload
fi
echo "Unit files installed; services were left unchanged."

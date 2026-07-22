#!/usr/bin/env bash
# Install one environment's units only from its exact approved clean checkout.
# The script deliberately never enables, starts, or restarts a service/timer.
set -euo pipefail
PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
export PATH
for GIT_VARIABLE in "${!GIT_@}"; do
    unset "$GIT_VARIABLE"
done
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_NO_REPLACE_OBJECTS=1

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BACKEND_ROOT="$(dirname "$SCRIPT_DIR")"
SYSTEMD_DIR="/etc/systemd/system"
MODE="dry-run"
MODE_SET=0
DEPLOY_ENVIRONMENT=""
SOURCE_REF=""
UNIT_MANIFEST_PATH="deploy/systemd/runtime-units.manifest"

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
        ;;
    staging)
        SOURCE_ROOT="/home/verdaxis-prod/verdaxis/staging/be"
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

if [[ "$MODE" == "apply" \
    && ( "$EUID" -ne 0 || "${VERDAXIS_SYSTEMD_INSTALL_LOCKED:-}" != "1" ) ]]; then
    exec sudo /usr/bin/env -i \
        PATH="$PATH" VERDAXIS_SYSTEMD_INSTALL_LOCKED=1 \
        /usr/bin/flock --exclusive --nonblock \
        "/run/lock/verdaxis-systemd-units-${DEPLOY_ENVIRONMENT}.lock" \
        "$SCRIPT_DIR/install_systemd_units.sh" --apply \
        --environment "$DEPLOY_ENVIRONMENT" --source-ref "$SOURCE_REF"
fi

SOURCE_GIT=(/usr/bin/git -C "$SOURCE_ROOT" -c "safe.directory=$SOURCE_ROOT" -c core.hooksPath=/dev/null)
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
if "${SOURCE_GIT[@]}" ls-files -v -- | awk '$1 ~ /^[a-zS]$/ { found=1 } END { exit found ? 0 : 1 }'; then
    echo "selected deploy checkout contains hidden tracked-file index flags" >&2
    exit 1
fi

verify_staged_blob() {
    local relative="$1"
    local staged="$STAGING_DIR/$relative"
    local committed_digest staged_digest

    if ! sudo test -f "$staged" || sudo test -L "$staged"; then
        echo "approved artifact must be a regular committed file: $relative" >&2
        exit 1
    fi
    committed_digest="$("${SOURCE_GIT[@]}" show "$SOURCE_REF:$relative" \
        | sha256sum | awk '{print $1}')"
    staged_digest="$(sudo sha256sum -- "$staged" | awk '{print $1}')"
    if [[ "$staged_digest" != "$committed_digest" ]]; then
        echo "staged artifact differs from the approved committed blob: $relative" >&2
        exit 1
    fi
}

STAGING_DIR="$(sudo mktemp -d /run/verdaxis-systemd-units.XXXXXXXX)"
APPLY_STARTED=0
REPLACEMENTS_COMMITTED=0
DESTINATION_TEMPS=()
APPLIED_DESTINATIONS=()
APPLIED_UNIT_NAMES=()
APPLIED_EXISTED=()
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
rollback_replacements() {
    local index destination unit_name existed
    for ((index = ${#APPLIED_DESTINATIONS[@]} - 1; index >= 0; index--)); do
        destination="${APPLIED_DESTINATIONS[$index]}"
        unit_name="${APPLIED_UNIT_NAMES[$index]}"
        existed="${APPLIED_EXISTED[$index]}"
        if [[ "$existed" == "1" ]]; then
            sudo cp -a -- "$STAGING_DIR/previous/$unit_name" "$destination"
        else
            sudo rm -f -- "$destination"
        fi
    done
    sudo systemctl daemon-reload || \
        echo "WARNING: daemon-reload failed after unit rollback." >&2
}
cleanup_install() {
    local status="$?"
    trap - EXIT
    if [[ "$status" != "0" && "$APPLY_STARTED" == "1" \
        && "$REPLACEMENTS_COMMITTED" == "0" ]]; then
        rollback_replacements
    fi
    if [[ "${#DESTINATION_TEMPS[@]}" -gt 0 ]]; then
        sudo rm -f -- "${DESTINATION_TEMPS[@]}"
    fi
    cleanup_staging
    exit "$status"
}
trap cleanup_install EXIT
sudo chmod 0700 "$STAGING_DIR"
if [[ "$(sudo stat -c '%u:%g:%a' "$STAGING_DIR")" != "0:0:700" ]]; then
    echo "systemd staging directory must be private and root-owned" >&2
    exit 1
fi

"${SOURCE_GIT[@]}" cat-file -e "$SOURCE_REF:$UNIT_MANIFEST_PATH"
"${SOURCE_GIT[@]}" archive --format=tar "$SOURCE_REF" "$UNIT_MANIFEST_PATH" \
    | sudo tar --extract --file=- --directory="$STAGING_DIR"
verify_staged_blob "$UNIT_MANIFEST_PATH"

UNIT_NAMES=()
declare -A MANIFEST_ENTRIES=()
while IFS=$'\t' read -r manifest_environment unit_name extra; do
    [[ -z "$manifest_environment" || "$manifest_environment" == \#* ]] && continue
    if [[ -n "$extra" \
        || ! "$manifest_environment" =~ ^(production|staging)$ \
        || ! "$unit_name" =~ ^verdaxis-[a-z0-9-]+\.(service|timer)$ \
        || ( "$manifest_environment" == "production" && "$unit_name" == *-staging.* ) \
        || ( "$manifest_environment" == "staging" && "$unit_name" != *-staging.* ) \
        || -n "${MANIFEST_ENTRIES[$unit_name]:-}" ]]; then
        echo "invalid or duplicate runtime unit manifest entry" >&2
        exit 1
    fi
    MANIFEST_ENTRIES[$unit_name]=1
    if [[ "$manifest_environment" == "$DEPLOY_ENVIRONMENT" ]]; then
        UNIT_NAMES+=("$unit_name")
    fi
done < <(sudo /bin/cat "$STAGING_DIR/$UNIT_MANIFEST_PATH")
if [[ "${#UNIT_NAMES[@]}" == 0 ]]; then
    echo "runtime unit manifest has no entries for $DEPLOY_ENVIRONMENT" >&2
    exit 1
fi

ARCHIVE_PATHS=("$UNIT_MANIFEST_PATH")
for unit_name in "${UNIT_NAMES[@]}"; do
    relative="deploy/systemd/$unit_name"
    "${SOURCE_GIT[@]}" cat-file -e "$SOURCE_REF:$relative"
    ARCHIVE_PATHS+=("$relative")
done
"${SOURCE_GIT[@]}" archive --format=tar "$SOURCE_REF" "${ARCHIVE_PATHS[@]}" \
    | sudo tar --extract --file=- --directory="$STAGING_DIR"
for relative in "${ARCHIVE_PATHS[@]}"; do
    verify_staged_blob "$relative"
done
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

PENDING_DIR="/var/lib/verdaxis/systemd-units"
PENDING_STATE="$PENDING_DIR/${DEPLOY_ENVIRONMENT}.pending"
sudo install -d -m 0750 -o root -g root "$PENDING_DIR"
printf 'ENVIRONMENT=%s\nSOURCE_REF=%s\n' "$DEPLOY_ENVIRONMENT" "$SOURCE_REF" \
    | sudo tee "${PENDING_STATE}.tmp.$$" >/dev/null
sudo chmod 0644 "${PENDING_STATE}.tmp.$$"
sudo mv -- "${PENDING_STATE}.tmp.$$" "$PENDING_STATE"
APPLY_STARTED=1
sudo install -d -m 0700 -o root -g root "$STAGING_DIR/previous"

for index in "${!UNIT_NAMES[@]}"; do
    unit_name="${UNIT_NAMES[$index]}"
    source="${UNIT_SOURCES[$index]}"
    destination="$SYSTEMD_DIR/$unit_name"
    if sudo test -f "$destination" \
        && sudo test ! -L "$destination" \
        && sudo cmp -s "$source" "$destination"; then
        echo "$unit_name is already current"
        continue
    fi
    destination_temp="${destination}.verdaxis-pending.$$"
    DESTINATION_TEMPS+=("$destination_temp")
    if sudo test -e "$destination"; then
        sudo cp -a -- "$destination" "$STAGING_DIR/previous/$unit_name"
        existed=1
    else
        existed=0
    fi
    sudo install -m 0644 -o root -g root "$source" "$destination_temp"
    APPLIED_DESTINATIONS+=("$destination")
    APPLIED_UNIT_NAMES+=("$unit_name")
    APPLIED_EXISTED+=("$existed")
done
for index in "${!APPLIED_DESTINATIONS[@]}"; do
    sudo mv -- "${DESTINATION_TEMPS[$index]}" "${APPLIED_DESTINATIONS[$index]}"
done
REPLACEMENTS_COMMITTED=1
sudo systemctl daemon-reload
sudo rm -f -- "$PENDING_STATE"
echo "Unit files installed; services and timers were left unchanged."

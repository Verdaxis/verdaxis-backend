#!/usr/bin/env bash
# Stage 1: fixture adapter only. Live collection needs separate reviewed authority.
set -euo pipefail
root=$(git rev-parse --show-toplevel)

# REVIEWED HOST COMMAND ALLOWLIST (argument vectors, not shell fragments).
# None is executed in stage 1. No remote transport or generic command runner exists.
# systemctl list-unit-files --no-pager --no-legend
# systemctl list-timers --all --no-pager --no-legend
# systemctl cat --no-pager -- UNIT
# crontab -l -u USER
# ps -eo pid=,user=,comm=  (no argv or environment: these can contain credentials)
# find /usr/local/sbin /usr/local/libexec -xdev -type f -perm /111 -print
# dpkg-query -W -f='${binary:Package}\t${Version}\n'
# stat --format='%U:%G %a %s' -- SCRIPT
# sha256sum -- SCRIPT
# Bounded static script/import reads require a separately reviewed no-follow reader.
# UNIT, USER and SCRIPT must be exact operator-reviewed identifiers, not discovery output.
# No environment/key files or artifact-provenance paths may be read, even for a digest.
# systemctl cat can expose inline secrets: stage 2 must redact before transport/storage.

if [[ ${1:-} != --fixture ]]; then
    printf '%s\n' 'stage 1: live host collection is disabled' >&2
    exit 2
fi
python=${HOST_INVENTORY_TEST_PYTHON:-python3}
exec "$python" "$root/tests/rust_contract/phase10/host_inventory/inventory.py" collect "$@"

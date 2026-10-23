#!/usr/bin/env bash
# Fixture CLI only. Reader/transport activation needs separate live authority.
set -euo pipefail
root=$(git rev-parse --show-toplevel)

# The single reviewed host argv allowlist is COMMANDS + allowed() in
# deploy/tests/host-inventory-reader.py, the exact script sent on SSH stdin.
# No parallel shell allowlist: metadata/digests use bounded no-follow OS reads.
# tests/rust_contract/phase10/host_inventory/transport.py fixes both SSH targets.
# Stage 2a tests inject commands/SSH; this CLI still cannot activate live reads.

if [[ ${1:-} != --fixture ]]; then
    printf '%s\n' 'stage 1: live host collection is disabled' >&2
    exit 2
fi
python=${HOST_INVENTORY_TEST_PYTHON:-python3}
exec "$python" "$root/tests/rust_contract/phase10/host_inventory/inventory.py" collect "$@"

#!/usr/bin/env bash
# Fixture acceptance only; Task 31 owns installed evidence and signing.
set -euo pipefail
root=$(git rev-parse --show-toplevel)
if [[ ${1:-} != --fixture || $# != 2 ]]; then
    printf '%s\n' 'stage 1: use --fixture INVENTORY.json; installed acceptance is disabled' >&2
    exit 2
fi
python=${HOST_INVENTORY_TEST_PYTHON:-python3}
exec "$python" "$root/tests/rust_contract/phase10/host_inventory/inventory.py" verify "$@"

#!/usr/bin/env bash
# Fixture acceptance only; Task 31 owns installed evidence and signing.
set -euo pipefail
root=$(git rev-parse --show-toplevel)
if [[ ${1:-} != --fixture || $# -gt 2 ]]; then
    printf '%s\n' 'use --fixture [INVENTORY.json]; installed acceptance is disabled' >&2
    exit 2
fi
python=${HOST_INVENTORY_TEST_PYTHON:-python3}
if [[ $# == 1 ]]; then
    # The verifier rejects symlink ancestors, including macOS /var and /tmp.
    TMPDIR=$(cd "${TMPDIR:-/tmp}" && pwd -P)
    export TMPDIR
    exec "$python" "$root/tests/tooling/phase10/host_inventory/contract.py"
fi
exec "$python" "$root/tests/tooling/phase10/host_inventory/inventory.py" verify "$@"

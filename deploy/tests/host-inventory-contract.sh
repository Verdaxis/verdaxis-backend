#!/usr/bin/env bash
# Sanitized, disposable fixtures only. No installed-host inventory is collected.
set -euo pipefail
root=$(git rev-parse --show-toplevel)
cd "$root"
python=${HOST_INVENTORY_TEST_PYTHON:-python3}
"$python" tests/rust_contract/phase10/host_inventory/contract.py

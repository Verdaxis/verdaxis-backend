#!/usr/bin/env bash
# Sanitized, disposable fixtures only. No installed-host inventory is collected.
set -euo pipefail
root=$(git rev-parse --show-toplevel)
cd "$root"
python=${HOST_INVENTORY_TEST_PYTHON:-python3}
"$python" tests/rust_contract/phase10/host_inventory/contract.py
"$python" tests/rust_contract/phase10/host_inventory/reader_contract.py
"$python" tests/rust_contract/phase10/host_inventory/review_contract.py
"$python" tests/rust_contract/phase10/host_inventory/round2_contract.py
"$python" tests/rust_contract/phase10/host_inventory/round3_contract.py
"$python" tests/rust_contract/phase10/host_inventory/round4_contract.py
"$python" tests/rust_contract/phase10/host_inventory/round5_contract.py

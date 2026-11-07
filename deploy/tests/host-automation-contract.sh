#!/usr/bin/env bash
# Off-host register fixtures only. Does not run or change host automation.
set -euo pipefail
root=$(git rev-parse --show-toplevel)
cd "$root"
python=${HOST_INVENTORY_TEST_PYTHON:-python3}
check_register() {
    "$python" tests/tooling/phase10/host_inventory/automation_contract.py --check "$1"
}
required_verdaxis_python_has_no_exception() { check_register "${FUNCNAME[0]}"; }
only_superseded_exact_cron_entries_are_removed() { check_register "${FUNCNAME[0]}"; }
backup_restore_preserves_inventory_and_row_counts() { check_register "${FUNCNAME[0]}"; }
staging_pull_preserves_target_identity_and_schedule() { check_register "${FUNCNAME[0]}"; }
owner_acceptance_covers_each_required_replacement() { check_register "${FUNCNAME[0]}"; }

required_verdaxis_python_has_no_exception
only_superseded_exact_cron_entries_are_removed
backup_restore_preserves_inventory_and_row_counts
staging_pull_preserves_target_identity_and_schedule
owner_acceptance_covers_each_required_replacement

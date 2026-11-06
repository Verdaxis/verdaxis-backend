#!/usr/bin/env bash
# Task 32 deletion contract, fixture mode only.
#
# Runs the host-cleanup checks over a synthetic fixture host inventory plus
# the repository-side deletion contract. No host is contacted and nothing is
# deleted; applying these checks to the real post-W5 tree is a separate,
# authorized step (Tasks 33 and 35).
#
# Usage: bash deploy/tests/deletion-contract.sh --fixture DIR
#        (DIR holds host/host-inventory.json; default:
#         tests/rust_contract/phase10/deletion-fixtures/pre)
#
# Exit 0 only when every check passes: the pre fixture (Python still present)
# must fail, the post fixture (the W5 end state) must pass.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
FIXTURE="$ROOT/tests/rust_contract/phase10/deletion-fixtures/pre"
PYTHON=${DELETION_TEST_PYTHON:-python3}

usage() {
    printf '%s\n' "usage: bash deploy/tests/deletion-contract.sh --fixture [DIR]" >&2
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --fixture)
            if [[ $# -ge 2 && "$2" != --* ]]; then
                FIXTURE=$2
                shift
            fi
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            usage
            exit 2
            ;;
    esac
    shift
done

INVENTORY="$FIXTURE/host/host-inventory.json"
if [[ ! -f "$INVENTORY" ]]; then
    printf '%s\n' "deletion-contract: no fixture inventory at $INVENTORY" >&2
    exit 2
fi

# Units whose removal Task 32 owns, from the repository removal ledger.
DELETED_UNITS=$("$PYTHON" - "$ROOT" <<'PY'
import json
import sys

root = sys.argv[1]
ledger = json.load(open(f"{root}/tests/rust_contract/phase10/removal-ledger.json", encoding="utf-8"))
rows = [row for row in ledger["artifacts"]
        if row["kind"] in {"unit", "timer"} and row["end_state"] == "gone"
        and row.get("deleting_task") == 32]
print("\n".join(sorted(row["source"] for row in rows)))
PY
)

run_check() {
    local name=$1
    # One unit name per line; word splitting here is the intended list form.
    # shellcheck disable=SC2086
    if "$PYTHON" "$ROOT/scripts/rust_contract/deletion_host_contract.py" \
        --check "$name" --inventory "$INVENTORY" --delete-units $DELETED_UNITS >/dev/null 2>&1; then
        printf 'PASS %s\n' "$name"
    else
        printf 'FAIL %s\n' "$name"
        FAILED=true
    fi
}

FAILED=false

# Host-side checks, one PASS/FAIL line each.
run_check no_installed_reference_to_a_deleted_python_path
run_check host_python_cleanup_removes_only_verdaxis_owned_paths
run_check staging_is_cleaned_before_production
run_check old_timer_and_new_job_do_not_duplicate
run_check previous_release_artifact_still_starts
run_check daily_backup_and_staging_pull_still_run

# The retained backup and staging-pull entries keep their exact schedules.
while IFS="	" read -r name expression; do
    if grep -F -- "$expression" "$INVENTORY" >/dev/null; then
        printf 'PASS daily_backup_and_staging_pull_still_run (%s schedule)\n' "$name"
    else
        printf 'FAIL daily_backup_and_staging_pull_still_run (%s schedule)\n' "$name"
        FAILED=true
    fi
done <<SCHEDULES
nightly-backup	30 2 * * *
staging-pull	0 3 * * *
SCHEDULES

# Staging is cleaned before production: read the recorded completion order.
if "$PYTHON" - "$INVENTORY" <<'PY'
import json
import sys

document = json.load(open(sys.argv[1], encoding="utf-8"))
staging = document["hosts"]["staging"]["cleanup"]
production = document["hosts"]["production"]["cleanup"]
assert staging["status"] == "complete", "staging cleanup is not complete"
assert production["status"] == "complete", "production cleanup is not complete"
assert staging["completed_at"] < production["completed_at"], "staging must be cleaned first"
PY
then
    printf 'PASS staging_is_cleaned_before_production (order)\n'
else
    printf 'FAIL staging_is_cleaned_before_production (order)\n'
    FAILED=true
fi

if [[ "$FAILED" == true ]]; then
    exit 1
fi

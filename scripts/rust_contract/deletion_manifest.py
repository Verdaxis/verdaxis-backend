#!/usr/bin/env python3
"""Print the exact repository paths the W5 release must delete or keep.

Reads ``tests/rust_contract/phase10/removal-ledger.json`` as data (the ledger
is never imported) and partitions the Task 32 rows:

- delete: rows with end_state ``gone`` owned by Task 32;
- keep:   the paths the plan retains through W5 -- retained Alembic history,
          data-only CI fixtures, ``tests/rust_contract/test_phase10_inventory.py``
          and the named off-host bundle.

The keep list is explicit: anything the W5 release deletes that is not on the
delete list, or that appears on both lists, is a partition error.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
LEDGER = REPO / "tests/rust_contract/phase10/removal-ledger.json"
DELETING_TASK = 32

# Retained through W5 by the Task 32 plan. Each entry explains why it stays.
KEEP_PATHS = {
    # alembic/versions/* stay as inert migration history (audit evidence).
    "alembic/versions": "retained Alembic revision history, never executed after W5",
    # Data-only CI fixtures carry committed market data, no application code.
    "tests/fixtures": "data-only CI fixtures",
    # The source inventory keeps enforcing ledger membership after the removal.
    "tests/rust_contract/test_phase10_inventory.py":
        "post-removal inventory contract, retained by name",
    # The off-host bundle lives outside the repository by design.
    "private-evidence-root:offhost-tools/offhost-tools-bundle.tar":
        "named off-host bundle (private evidence root, never under deploy/)",
}


def load_rows(ledger_path=LEDGER):
    document = json.loads(Path(ledger_path).read_text(encoding="utf-8"))
    return document["artifacts"]


def delete_paths(rows):
    return sorted({row["source"] for row in rows
                   if row["end_state"] == "gone" and row.get("deleting_task") == DELETING_TASK})


def keep_conflicts(rows, delete_list):
    """Keep paths that a delete row shadows, which the partition forbids."""
    conflicts = []
    for keep in KEEP_PATHS:
        if keep.startswith("private-evidence-root:"):
            continue
        for deleted in delete_list:
            if deleted == keep or deleted.startswith(keep.rstrip("/") + "/"):
                conflicts.append((keep, deleted))
    return sorted(conflicts)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true",
                        help="verify every listed path exists today and the partition is clean")
    parser.add_argument("--ledger", type=Path, default=LEDGER,
                        help="alternate removal ledger to read")
    args = parser.parse_args()

    rows = load_rows(args.ledger)
    delete_list = delete_paths(rows)

    if args.check:
        errors = []
        for relative in delete_list:
            if not (REPO / relative).exists():
                errors.append(f"listed for deletion but absent today: {relative}")
        for keep, deleted in keep_conflicts(rows, delete_list):
            errors.append(f"kept path {keep} is shadowed by a deletion: {deleted}")
        for keep, reason in sorted(KEEP_PATHS.items()):
            if keep.startswith("private-evidence-root:"):
                continue
            if not (REPO / keep).exists():
                errors.append(f"kept path missing today: {keep} ({reason})")
        if errors:
            for error in errors:
                print(f"ERROR {error}")
            raise SystemExit(1)
        print(f"OK {len(delete_list)} delete paths exist, "
              f"{len(KEEP_PATHS)} keep paths exist, partition clean")
        return

    print("# W5 delete (removal-ledger rows with end_state gone owned by Task 32):")
    for relative in delete_list:
        print(f"DELETE {relative}")
    print("# W5 keep (retained by the Task 32 plan):")
    for keep, reason in sorted(KEEP_PATHS.items()):
        print(f"KEEP {keep}  # {reason}")


if __name__ == "__main__":
    main()

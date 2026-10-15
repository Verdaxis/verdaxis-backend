"""Assertion migration ledger: source discovery and ledger checking.

The Rust port must transfer behavior, not Python mocks. This module scans the
existing Python tests that cover each ported surface, records one ledger row
per discovered assertion, and proves the ledger accounts for all of them. A row
has a target contract group, or an explicit exclusion with a reviewer. Router
tasks fill `behavior` and narrow `target_id` to real cases as they land.
"""

from __future__ import annotations

import argparse
import ast
import hashlib
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LEDGER_PATH = ROOT / "tests" / "rust_contract" / "assertions.json"

# Scanned test file -> (owning route task, contract group or groups that must
# carry the behavior). Split files name both owners.
SOURCE_FILES: dict[str, tuple[str, str]] = {
    "tests/unit/test_public_market_read_admission.py": ("3", "health"),
    "tests/unit/test_public_market_catalog_contract.py": ("5/8", "simple_reads"),
    "tests/unit/test_catalog_models.py": ("5/8", "simple_reads"),
    "tests/unit/test_news_feed.py": ("5/8", "simple_reads"),
    "tests/unit/test_public_owner_admission.py": ("6/6a", "live_benchmarks"),
    "tests/unit/test_market_evidence_policy.py": ("6/6a", "live_benchmarks"),
    "tests/unit/test_orderbook_live_benchmarks.py": (
        "6a/7a",
        "live_benchmarks,orderbook_snapshot",
    ),
    "tests/unit/test_orderbook_marketplace_filters.py": (
        "7b/7c",
        "orderbook_lists,orderbook_map",
    ),
    "tests/unit/test_orderbook_crossing_filters.py": ("7b/7c", "orderbook_lists"),
    "tests/unit/test_orderbook_schemas.py": ("7b/7c", "orderbook_lists"),
    "tests/unit/test_ci_orderbook_fields.py": ("7b/7c", "orderbook_lists"),
    "tests/unit/test_orderbook_map_compact.py": ("7b/7c", "orderbook_map"),
    "tests/unit/test_orderbook_router.py": ("7b/7c", "orderbook_lists,orderbook_map"),
    "tests/unit/test_price_discovery_router.py": ("9", "prices"),
    "tests/unit/test_reference_price_export.py": ("9", "prices"),
    "tests/unit/test_trade_tape.py": ("9", "prices"),
    "tests/unit/test_curves.py": ("10a/10b", "curves_legacy,curves_canonical"),
    "tests/unit/test_forward_curve_market_slices.py": ("10b", "curves_canonical"),
    "tests/unit/test_mcp_server.py": ("11", "mcp"),
    "tests/unit/test_voyage_prices.py": ("11", "mcp"),
    "tests/unit/test_voyage_fuel_plan.py": ("11", "mcp"),
    "tests/unit/test_sea_distances.py": ("11", "mcp"),
}


def _test_names(tree: ast.Module) -> list[str]:
    names: list[str] = []
    for node in tree.body:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            if node.name.startswith("test_"):
                names.append(node.name)
        elif isinstance(node, ast.ClassDef):
            for child in node.body:
                if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    if child.name.startswith("test_"):
                        names.append(f"{node.name}::{child.name}")
    return names


def discover(root: Path = ROOT) -> list[dict[str, str]]:
    """Enumerate every ledger-eligible assertion in the scanned files."""
    found: list[dict[str, str]] = []
    for relative, (owner_task, target) in sorted(SOURCE_FILES.items()):
        for name in _test_names(ast.parse((root / relative).read_text())):
            found.append(
                {
                    "source_id": f"{relative}::{name}",
                    "source_file": relative,
                    "owner_task": owner_task,
                    "target_id": target,
                }
            )
    return found


def file_digests(root: Path = ROOT) -> dict[str, str]:
    """Digest of each scanned file, so source drift is visible."""
    return {
        relative: hashlib.sha256((root / relative).read_bytes()).hexdigest()
        for relative in sorted(SOURCE_FILES)
    }


def _source_sha(root: Path) -> str:
    try:
        return subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=root,
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def build_ledger(root: Path = ROOT) -> dict:
    """Build a fresh ledger. Rows start pending until a route task fills them."""
    digests = file_digests(root)
    return {
        "schema_revision": 1,
        "source_sha": _source_sha(root),
        "sources": {
            relative: {
                "digest": digests[relative],
                "owner_task": owner,
                "target_id": target,
            }
            for relative, (owner, target) in sorted(SOURCE_FILES.items())
        },
        "rows": [
            {
                "source_id": entry["source_id"],
                "source_file": entry["source_file"],
                "behavior": "",
                "owner_task": entry["owner_task"],
                "target_id": entry["target_id"],
                "status": "pending",
                "exclusion_reason": "",
                "reviewed_by": "",
            }
            for entry in discover(root)
        ],
    }


def check(ledger: dict, discovered: list[dict[str, str]]) -> list[str]:
    """Return ledger problems. An empty list means the ledger is complete."""
    problems: list[str] = []
    rows = ledger.get("rows", [])
    discovered_ids = {entry["source_id"] for entry in discovered}
    row_ids = [row.get("source_id") for row in rows]
    if len(set(row_ids)) != len(row_ids):
        problems.append("ledger has duplicate source ids")
    missing = discovered_ids - set(row_ids)
    if missing:
        problems.append(f"{len(missing)} discovered assertion(s) are not in the ledger")
    extra = set(row_ids) - discovered_ids
    if extra:
        problems.append(f"{len(extra)} ledger row(s) are no longer discovered")
    for row in rows:
        if not (row.get("target_id") or (row.get("exclusion_reason") and row.get("reviewed_by"))):
            problems.append(f"{row.get('source_id')} has no target and no reviewed exclusion")
    return problems


def check_targets(ledger: dict, passed_groups: set[str]) -> list[str]:
    """Return rows whose target group has not passed yet.

    A pending row is allowed before its route task runs. A row marked complete
    requires every group it targets to appear in the passed-groups set.
    """
    problems: list[str] = []
    for row in ledger.get("rows", []):
        if row.get("status") != "complete":
            continue
        targets = [part for part in str(row.get("target_id", "")).split(",") if part]
        unmet = [target for target in targets if target not in passed_groups]
        if unmet:
            problems.append(f"{row['source_id']} target(s) not passing: {', '.join(unmet)}")
    return problems


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", help="regenerate the ledger")
    parser.add_argument("--ledger", type=Path, default=LEDGER_PATH)
    parser.add_argument("--passed-groups", type=Path)
    args = parser.parse_args(argv)

    if args.write:
        ledger = build_ledger()
        args.ledger.write_text(json.dumps(ledger, indent=2) + "\n")
        print(f"wrote {len(ledger['rows'])} row(s)")
        return 0

    ledger = json.loads(args.ledger.read_text())
    problems = check(ledger, discover())
    if args.passed_groups:
        passed = set(json.loads(args.passed_groups.read_text()))
        problems += check_targets(ledger, passed)
    for problem in problems:
        print(problem, file=sys.stderr)
    if problems:
        print(f"assertion ledger failed: {len(problems)} problem(s)", file=sys.stderr)
        return 1
    print("assertion ledger OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

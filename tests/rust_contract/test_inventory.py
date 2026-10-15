"""Corpus inventory and assertion-ledger meta-tests.

These tests keep the recorded corpus honest: it must cover every planned public
method/path pair, keep unique case ids inside known groups, leave the ordered
object path list empty until a named consumer is added, and account for every
discovered Python assertion.
"""

import json
from pathlib import Path

from scripts.rust_contract import check_assertions

ROOT = Path(__file__).resolve().parents[2]
CASES_PATH = ROOT / "tests" / "rust_contract" / "cases.json"
ORDERED_PATH = ROOT / "tests" / "rust_contract" / "ordered-object-paths.json"
LEDGER_PATH = ROOT / "tests" / "rust_contract" / "assertions.json"

# The groups the disposable runner can dispatch. `echo` is Task 4's own
# test-only projection; the rest map to the route and health tasks.
ROUTE_GROUPS = {
    "health",
    "echo",
    "simple_reads",
    "live_benchmarks",  # Task 6a's test-only helper projection.
    "orderbook_snapshot",
    "orderbook_lists",
    "orderbook_map",
    "prices",
    "curves_legacy",
    "curves_canonical",
    "mcp",
}

# The 33 planned public method/path pairs from the route inventory.
PLANNED = (
    ("R01", "GET", "/api/ports"),
    ("R02", "GET", "/api/ports/{port_id}"),
    ("R03", "GET", "/api/catalog/products"),
    ("R04", "GET", "/api/catalog/delivery-points"),
    ("R05", "GET", "/api/news"),
    ("R06", "GET", "/api/trade-tape"),
    ("R07", "GET", "/api/curves/forward"),
    ("R08", "GET", "/api/curves/forward/export"),
    ("R09", "GET", "/api/curves/forward/table"),
    ("R10", "GET", "/api/curves/forward/slice"),
    ("R11", "GET", "/api/curves/forward/board"),
    ("R12", "GET", "/api/prices"),
    ("R13", "GET", "/api/prices/reference"),
    ("R14", "GET", "/api/prices/reference/export"),
    ("R15", "GET", "/api/orderbook/snapshot"),
    ("R16", "GET", "/api/orderbook/bids"),
    ("R17", "GET", "/api/orderbook/asks"),
    ("R18", "GET", "/api/orderbook/with-ci"),
    ("R19", "GET", "/api/orderbook/aggregated"),
    ("R20", "GET", "/api/orderbook/product-counts"),
    ("R21", "GET", "/api/orderbook/map-summary"),
    ("R22", "GET", "/api/orderbook/map-summary/compact"),
    ("R23", "GET", "/api/orderbook/products"),
    ("R24", "GET", "/api/orderbook/regions"),
    ("R25", "GET", "/api/orderbook/fuel-types"),
    ("R26", "GET", "/api/orderbook"),
    ("R27", "GET", "/api/listings"),
    ("H01", "GET", "/health/ready"),
    ("H02", "GET", "/health/live"),
    ("H03", "GET", "/health"),
    ("M01", "POST", "/mcp"),
    ("M02", "GET", "/mcp"),
    ("M03", "DELETE", "/mcp"),
)


def _load(path: Path):
    return json.loads(path.read_text())


def _covers(template: str, method: str, case: dict) -> bool:
    if case["method"] != method:
        return False
    wanted = template.strip("/").split("/")
    got = case["path"].strip("/").split("/")
    if len(wanted) != len(got):
        return False
    return all(
        want == entry or (want.startswith("{") and want.endswith("}"))
        for want, entry in zip(wanted, got)
    )


def _cases() -> list[dict]:
    return [case for group in _load(CASES_PATH)["groups"].values() for case in group]


def test_corpus_covers_the_planned_route_inventory():
    cases = _cases()
    uncovered = [
        inventory_id
        for inventory_id, method, template in PLANNED
        if not any(_covers(template, method, case) for case in cases)
    ]
    assert uncovered == []


def test_case_ids_are_unique_and_groups_known():
    document = _load(CASES_PATH)
    ids = [case["id"] for group in document["groups"].values() for case in group]
    assert len(ids) == len(set(ids))
    assert set(document["groups"]) == ROUTE_GROUPS


def test_ordered_object_paths_start_empty():
    assert _load(ORDERED_PATH) == []


def test_every_discovered_assertion_is_accounted_for():
    ledger = _load(LEDGER_PATH)["rows"]
    discovered = {entry["source_id"] for entry in check_assertions.discover()}
    assert discovered == {row["source_id"] for row in ledger}
    for row in ledger:
        assert row["target_id"] or (row["exclusion_reason"] and row["reviewed_by"])

"""Keep the successor inventory complete without changing the byte corpus."""

import copy
import json
from pathlib import Path

import pytest

from tests.rust_contract.test_inventory import PLANNED, ROUTE_GROUPS

ROOT = Path(__file__).resolve().parents[2]
CORPUS = ROOT / "tests/rust_contract"
INVENTORY = CORPUS / "phase1_meaning/routes.json"


def _load(path):
    return json.loads(path.read_text())


def _validate(document):
    cases = _load(CORPUS / "cases.json")["groups"]
    workload = _load(CORPUS / "workload.json")
    assert document["version"] == 1
    assert document["activation"] == "successor-only; founder approval required"
    routes = document["routes"]
    assert len(routes) == 27
    assert len({route["id"] for route in routes}) == 27
    assert {(route["id"], route["method"], route["path"]) for route in routes} == {
        entry for entry in PLANNED if entry[0].startswith("R")
    }
    assert {tool["id"] for tool in document["tools"]} == {
        tool["id"] for tool in workload["tools"]
    }
    assert len(document["tools"]) == 3
    assert set(document["groups"]) == ROUTE_GROUPS
    for group, requests in cases.items():
        scenarios = document["groups"][group]
        assert len({case["request"]["id"] for case in scenarios}) == len(scenarios)
        # Compare whole requests, including malformed bodies and middleware headers.
        assert [case["request"] for case in scenarios] == requests
        for case in scenarios:
            assert case["route_id"] in document["comparison_ids"]
            assert case["default_comparison"] == "strict"
            assert case["default_cache"] == "none"
    for route in routes + document["tools"]:
        assert route["comparator_route_id"] in document["comparison_ids"]
        assert route["codec"] in {"json", "bytes", "mcp-json"}
        assert route["cache_class"] in {"none", "reference-60s"}
        for field in ("decimal_paths", "datetime_paths", "order_keys", "policy_dependencies"):
            assert isinstance(route[field], list)
        assert route["caddy_group"] in {"simple", "prices", "curves", "orderbook", "mcp"}
    assert {route["id"] for route in routes if route["cache_class"] == "reference-60s"} == {
        "R01", "R02", "R03", "R04", "R05"
    }
    assert document["trade_tape"]["order"] == ["confirmed_at DESC", "full trade UUID DESC"]
    assert document["trade_tape"]["oracle"] == "complete-python-relation-before-pagination"
    assert document["accepted_differences"]["decisions"] == [84, 85]
    assert document["measurements"] == "not authorized in Tasks 1–6 (decision 96)"


def test_inventory_cannot_drop_a_route_or_scenario():
    _validate(_load(INVENTORY))


@pytest.mark.parametrize("mutation", ["route", "tool", "negative", "duplicate", "unknown-path"])
def test_inventory_rejects_missing_or_changed_coverage(mutation):
    document = copy.deepcopy(_load(INVENTORY))
    if mutation == "route":
        document["routes"].pop()
    elif mutation == "tool":
        document["tools"].pop()
    elif mutation == "negative":
        document["groups"]["mcp"] = [
            case for case in document["groups"]["mcp"]
            if case["request"]["id"] != "M15-bad-decimal"
        ]
    elif mutation == "duplicate":
        document["groups"]["health"].append(document["groups"]["health"][0])
    else:
        document["routes"][0]["path"] = "/api/not-a-route"
    with pytest.raises(AssertionError):
        _validate(document)

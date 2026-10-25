"""Phase 9 requirements, source inventory and in-process write-path contract."""

import ast
import copy
import json
import re
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
LEDGER = ROOT / "tests/rust_contract/phase9/required-scenarios.json"
ROUTES = {
    "O1": ("POST", "/api/orderbook", 19),
    "O2": ("PUT", "/api/orderbook/{order_id}", 20),
    "I5": ("POST", "/api/inventory/{item_id}/publish", 24),
    "T1": ("POST", "/api/trades/", 21),
    "T2": ("GET", "/api/trades/summary", 21),
    "T3": ("GET", "/api/trades/my", 21),
    "T4": ("PUT", "/api/trades/{trade_id}/confirm", 22),
    "T5": ("PUT", "/api/trades/{trade_id}/decline", 22),
    "T6": ("PUT", "/api/trades/{trade_id}/deliver", 23),
    "T7": ("POST", "/api/trades/{trade_id}/pay", 23),
    "M1": ("GET", "/api/matchmaking/suggestions", 25),
    "M2": ("PATCH", "/api/matchmaking/suggestions/{order_id}/dismiss", 25),
    "F1": ("POST", "/api/admin/trades/{trade_id}/freeze", 26),
    "F2": ("POST", "/api/admin/trades/{trade_id}/unfreeze", 26),
    "F3": ("GET", "/api/admin/trades/frozen", 26),
}
# Independent requirements: removing a ledger row cannot reduce the required set.
PROPERTIES = {
    "exact_slice": {16}, "opposite_side": {16}, "price_crossing": {16},
    "provenance_isolation": {17}, "exact_demo_pair": {17}, "self_trade": {16, 17},
    "expiry": {4, 16, 24}, "admission_revalidation": {3, 17, 19, 20, 21, 22, 24, 29},
    "price_priority": {16}, "acceptance_priority": {16, 20}, "uuid_tie_break": {16},
    "resting_price": {16}, "partial_fill": {16, 18}, "quantity_status_version": {16, 20},
    "amend_priority": {20}, "scalar_qualification_priority": {20},
    "qualification": {17}, "fame_compatibility": {17}, "lng_compatibility": {17},
    "bound_atomicity": {16, 19, 20, 24}, "post_only": {17, 19},
    "stock_conservation": {4, 13, 18, 21, 22, 24, 29},
    "manual_and_auto_confirmation": {18, 21, 22}, "settlement": {22, 23, 28},
    "fee_snapshot": {18, 21, 23}, "snapshot_immutability": {18, 21},
    "retry_replay": {3, 6, 19, 20, 21, 22, 23, 24, 29},
    "atomic_effects": {5, 18, 19, 20, 21, 22, 23, 24},
    "projection_privacy": {18, 21, 25}, "both_benchmarks": {5, 18, 19, 20, 21, 22, 24},
    "pin_display": {5, 18, 20, 22, 24, 29}, "scoped_outbox": {5, 18, 19, 20, 21, 22, 23, 24},
    "suggestion_public_relation": {25}, "recorded_party_discovery": {21, 29, 30},
    "assisted_cleanup": {3, 21, 22, 23, 29}, "freeze_invariant": {26, 27, 28, 29, 30, 31},
    "residual_python_writers": {12, 27, 29},
}
KYC_FIELDS = {"code", "message", "kyc_status", "grace_expires_at"}


def load_ledger(path=LEDGER):
    if not path.exists():
        return {"routes": [], "matching_properties": [], "client_changes": []}
    return json.loads(path.read_text())


def validate_requirements(ledger):
    properties = ledger["matching_properties"]
    ids = [row["property"] for row in properties]
    missing = set(PROPERTIES) - set(ids)
    assert not missing, f"missing matching properties: {sorted(missing)}"
    assert len(ids) == len(set(ids)) and set(ids) == set(PROPERTIES)
    for row in properties:
        name = row["property"]
        assert set(row["owning_tasks"]) == PROPERTIES[name], f"{name}: owning tasks"
        assert row["cases"] and row["expected"] and row["source"], name
        assert row["integration_owner"] == "integration", name
    routes = ledger["routes"]
    assert len(routes) == len(ROUTES)
    assert {row["route_id"] for row in routes} == set(ROUTES)
    for row in routes:
        route = row["route_id"]
        method, path, owner = ROUTES[route]
        assert (row["method"], row["path"], row["owning_task"]) == (method, path, owner)
        for field in ("auth_class", "request", "response", "refusals", "scenarios"):
            assert row[field], f"{route}: missing {field}"
        assert {"success", "admission", "empty_values"} <= set(row["scenarios"]), route
        if route in {"O1", "O2", "I5", "T1", "T4", "T5", "T6", "T7", "F1", "F2"}:
            assert row["effect_owners"] == [5, owner], f"{route}: effect owners"
    assert ledger["error_contract"]["kyc_refusal"] == {
        "status": 403, "code": "KYC_REQUIRED", "fields": sorted(KYC_FIELDS),
        "kyc_status": "current status; assisted customer refusal uses PENDING",
        "grace_expires_at": "ISO timestamp or null; assisted customer refusal uses null",
        "owning_tasks": [3, 17, 19, 20, 21, 22, 24],
        "integration_owner": "integration",
    }
    changes = ledger["client_changes"]
    assert len(changes) == 3 and {row["change"] for row in changes} == {1, 2, 3}
    assert {row["change"]: row["owning_tasks"] for row in changes} == {
        1: [21, 26, 28, 30], 2: [21, 22, 23, 30], 3: [26, 27],
    }
    assert all(row["test"] and row["contract"] for row in changes)
    assert ledger["freeze_schema"]["active_index"] == "uq_trade_freezes_one_active"
    assert ledger["freeze_schema"]["owning_tasks"] == [27, 31]
    by_route = {row["route_id"]: row for row in routes}
    assert by_route["O1"]["response"]["status"] == 201
    assert all(by_route[route]["response"]["status"] == 200 for route in ("O2", "I5", "T1", "T2", "T3", "T4", "T5", "T6", "T7", "M1", "M2", "F2", "F3"))
    assert by_route["F1"]["response"]["statuses"] == [201, 200]
    assert by_route["I5"]["response"]["fields"] == {"status": "published", "listing_id": "UUID"}
    assert by_route["T2"]["response"]["empty"] == {
        "total_count": 0, "action_required_count": 0,
        "awaiting_counterparty_count": 0, "confirmed_count": 0,
    }
    assert by_route["T3"]["response"]["empty"] == {"items": [], "total": 0, "skip": 0, "limit": 20}
    assert by_route["M1"]["response"]["empty"] == []
    assert by_route["M2"]["effect_owners"] == [25]
    assert by_route["M2"]["response"]["fields"] == {"status": ["dismissed", "already_dismissed"]}
    assert ledger["write_path"] == {
        "matching": "in_process_caller_transaction", "cross_stack_http": False,
        "python_process": False, "deferred_or_noop_matching": False,
        "owning_tasks": [16, 19, 20, 24], "integration_owner": "integration",
    }


def test_phase9_scenarios_cover_every_matching_property():
    validate_requirements(load_ledger())


def source_routes(path):
    tree = ast.parse(path.read_text())
    prefix = ""
    for node in tree.body:
        if isinstance(node, ast.Assign) and isinstance(node.value, ast.Call):
            if ast.unparse(node.value.func) == "APIRouter":
                prefix = next((ast.literal_eval(k.value) for k in node.value.keywords if k.arg == "prefix"), "")
    routes = {}
    for node in tree.body:
        if not isinstance(node, ast.AsyncFunctionDef):
            continue
        for decorator in node.decorator_list:
            if not isinstance(decorator, ast.Call) or not isinstance(decorator.func, ast.Attribute):
                continue
            if ast.unparse(decorator.func.value) != "router":
                continue
            method = decorator.func.attr.upper()
            if method not in {"GET", "POST", "PUT", "PATCH", "DELETE"}:
                continue
            pair = (method, "/api" + prefix + ast.literal_eval(decorator.args[0]))
            assert pair not in routes
            routes[pair] = {
                "path": str(path.relative_to(ROOT)), "handler": node.name,
                "annotations": {arg.arg: ast.unparse(arg.annotation) for arg in node.args.args if arg.annotation},
                "defaults": [ast.unparse(value) for value in node.args.defaults],
                "response_model": next((ast.unparse(k.value) for k in decorator.keywords if k.arg == "response_model"), None),
                "status": next((ast.unparse(k.value) for k in decorator.keywords if k.arg == "status_code"), "200"),
            }
    return routes


def schema_fields(path, names):
    tree = ast.parse((ROOT / path).read_text())
    return {
        node.name: {
            "bases": [ast.unparse(base) for base in node.bases],
            "fields": {field.target.id: {
                "type": ast.unparse(field.annotation),
                "default": ast.unparse(field.value) if field.value is not None else None,
            } for field in node.body if isinstance(field, ast.AnnAssign) and isinstance(field.target, ast.Name)},
        }
        for node in tree.body if isinstance(node, ast.ClassDef) and node.name in names
    }


def refusal_fields(path, function):
    tree = ast.parse((ROOT / path).read_text())
    node = next(node for node in tree.body if isinstance(node, ast.AsyncFunctionDef) and node.name == function)
    return [
        {key.value for key in keyword.value.keys}
        for call in ast.walk(node) if isinstance(call, ast.Call) and ast.unparse(call.func) == "HTTPException"
        for keyword in call.keywords if keyword.arg == "detail" and isinstance(keyword.value, ast.Dict)
    ]


def test_phase9_route_inventory_matches_source():
    ledger = load_ledger()
    assert len(ledger["routes"]) == 15, "missing route requirements"
    inventory = {}
    for module in ("orderbook", "inventory", "trades", "matchmaking"):
        inventory.update(source_routes(ROOT / f"app/routers/{module}.py"))
    for row in ledger["routes"]:
        pair = (row["method"], row["path"])
        if row["route_id"].startswith("F"):
            assert row["source"] is None and row["new_route"] is True
            assert pair not in inventory
        else:
            assert pair in inventory, f"missing source route: {pair}"
            assert row["source"] == inventory[pair], f"source contract changed: {pair}"
    assert set(source_routes(ROOT / "app/routers/trades.py")) == {
        (row["method"], row["path"]) for row in ledger["routes"] if row["route_id"].startswith("T")
    }
    assert set(source_routes(ROOT / "app/routers/matchmaking.py")) == {
        ROUTES[route][:2] for route in ("M1", "M2")
    }
    assert {source["path"] for source in ledger["schemas"]} == {
        "app/schemas/orderbook.py", "app/schemas/pagination.py",
    }
    required_schemas = {
        "SupplierListingMetadataMixin", "MarketSupportFinalConfirmation", "OrderCreate", "OrderUpdate",
        "OrderResponse", "TradeCreate", "TradeResponse", "TradeSummaryResponse", "TradeDeliverPayload",
        "PaginatedResponse",
    }
    assert {name for source in ledger["schemas"] for name in source["classes"]} == required_schemas
    for source in ledger["schemas"]:
        assert schema_fields(source["path"], source["classes"]) == source["classes"]
    assert refusal_fields("app/services/kyc_execution.py", "require_kyc_execution") == [KYC_FIELDS]
    assert refusal_fields("app/services/execution_policy.py", "require_customer_organization_execution") == [KYC_FIELDS]


# The production write boundary has no HTTP or child-process dependency. The
# existing PostgreSQL transport fixture is test-only and is checked separately.
TRANSPORT = re.compile(
    r"\b(?:reqwest|hyper|ureq|surf)\s*::|\b(?:std|tokio)\s*::\s*process\b|"
    r"\bCommand\s*::|\b(?:TcpStream|UnixStream|UdpSocket)\b|"
    r"\b(?:python3?|matching_engine)\b|https?://|\binclude!"
)


def transport_calls(text):
    # Match endpoint literals before removing comments, so // in a URL cannot
    # remove the transport evidence. Comments may not authorize a transport.
    endpoints = re.findall(r'"https?://[^"\n]*"', text)
    text = re.sub(r"/\*.*?\*/|//[^\n]*", "", text, flags=re.DOTALL)
    return sorted(set(endpoints + TRANSPORT.findall(text)))


def test_no_cross_stack_call_in_the_write_path():
    root = ROOT / "rs/read/src/order_write"
    assert root.is_dir()
    files = sorted(root.rglob("*.rs"))
    files += sorted((ROOT / "rs/read/src/routes/market_write").rglob("*.rs"))
    assembly = ROOT / "rs/read/src/routes/market_write.rs"
    if assembly.exists():
        files.append(assembly)
    assert files, "write-path source is empty"
    fixture = root / "ordered_contract.rs"
    transaction = (root / "transaction.rs").read_text()
    if fixture.exists():
        assert '#[cfg(all(test, feature = "contract-tests"))]\n#[path = "ordered_contract.rs"]\nmod ordered_contract;' in transaction
        # A second declaration could make the same fixture a production module.
        declarations = sum(len(re.findall(r"\bmod\s+ordered_contract\s*;", path.read_text())) for path in files)
        assert declarations == 1
    violations = {
        str(path.relative_to(ROOT)): transport_calls(path.read_text())
        for path in files if path != fixture and transport_calls(path.read_text())
    }
    assert not violations, f"write-path transport or process dependency: {violations}"


def test_missing_ledger_reports_matching_requirements(tmp_path):
    with pytest.raises(AssertionError, match="missing matching properties:.*exact_slice"):
        validate_requirements(load_ledger(tmp_path / "absent.json"))


@pytest.mark.parametrize("change", ["property", "owner", "client_change", "effect_owner", "kyc_field"])
def test_phase9_ledger_requires_each_contract_family(change):
    ledger = copy.deepcopy(load_ledger())
    if change == "property":
        ledger["matching_properties"] = [row for row in ledger["matching_properties"] if row["property"] != "resting_price"]
    elif change == "owner":
        ledger["matching_properties"][0]["owning_tasks"] = []
    elif change == "client_change":
        ledger["client_changes"].pop()
    elif change == "effect_owner":
        ledger["routes"][0]["effect_owners"] = [19]
    else:
        ledger["error_contract"]["kyc_refusal"]["fields"].remove("grace_expires_at")
    with pytest.raises(AssertionError):
        validate_requirements(ledger)


@pytest.mark.parametrize("source", [
    'reqwest::Client::new().post(endpoint).send().await;',
    'std::process::Command::new("python3").arg("matching_engine").spawn();',
    'use tokio::net::TcpStream;',
])
def test_write_path_requires_in_process_execution(source):
    assert transport_calls(source)

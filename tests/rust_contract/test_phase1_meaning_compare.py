"""Phase 1 uses the reviewed engine without relaxing business meaning."""

import copy
import hashlib
import json
from pathlib import Path

import pytest

from scripts.rust_contract import compare
from tests.rust_contract.test_phase2_compare import STAMP, response

RULES = Path(__file__).parent / "phase1_meaning/normalization.json"


def differences(left, right, route="P1-R06", **options):
    assert RULES.is_file(), "missing reviewed Phase 1 ledger"
    return compare.compare_responses(
        left, right, profile="phase1-meaning-v1", route_id=route,
        rules_sha256=hashlib.sha256(RULES.read_bytes()).hexdigest(), **options,
    )


def tape():
    return {"items": [{"id": "a", "quantity_mt": "100.00", "price_per_mt_usd": "750.00", "total_usd": "75000.00", "confirmed_at": STAMP}], "total": 1, "market_hours": True}


def test_approved_scalar_spelling_changes_match():
    left = tape()
    right = copy.deepcopy(left)
    right["items"][0].update(quantity_mt="1e2", price_per_mt_usd="7.5e2", confirmed_at=STAMP.replace("Z", "+00:00"))
    assert differences(response(left), response(right)) == []
    assert differences(response(b'{"n":1,"s":"\\u0061"}'), response(b' {"s":"a","n":1e0} '), "P1-health") == []
    assert compare.compare_responses(response(left), response(right))


@pytest.mark.parametrize("mutation", ["price", "type", "null", "absence", "private", "status", "header", "member", "total", "boolean"])
def test_meaning_rejects_business_drift(mutation):
    left = tape()
    right = copy.deepcopy(left)
    if mutation == "price":
        right["items"][0]["price_per_mt_usd"] = "750.01"
    elif mutation == "type":
        right["items"][0]["quantity_mt"] = 100
    elif mutation == "null":
        right["items"][0]["quantity_mt"] = None
    elif mutation == "absence":
        del right["items"][0]["quantity_mt"]
    elif mutation == "private":
        right["items"][0]["supplier_name"] = "private"
    elif mutation == "member":
        right["items"][0]["id"] = "b"
    elif mutation == "total":
        right["total"] = 2
    elif mutation == "boolean":
        right["market_hours"] = 1
    actual = response(right, status=201 if mutation == "status" else 200,
                      headers={"Retry-After": "1"} if mutation == "header" else None)
    assert differences(response(left), actual)


def test_tape_is_strictly_ordered_even_inside_timestamp_ties():
    left = tape()
    left["items"].append({**left["items"][0], "id": "b"})
    assert differences(response(left), response({**left, "items": list(reversed(left["items"]))}))
    assert differences(response(left), response({**left, "items": [left["items"][0]] * 2}))


def test_news_ties_and_price_collections_preserve_full_item_multisets():
    a = {"id": "a", "published_at": STAMP, "fetched_at": STAMP}
    b = {**a, "id": "b"}
    assert differences(response([a, b]), response([b, a]), "P1-R05") == []
    assert differences(response([a, b]), response([a, a]), "P1-R05")
    assert differences(response([a, b]), response([{**b, "published_at": "2026-10-17T10:00:00Z"}, a]), "P1-R05")
    prices = {"summaries": [{"fuel_type": "a", "last_price": "750.00"}, {"fuel_type": "b", "last_price": "800"}]}
    assert differences(response(prices), response({"summaries": list(reversed(prices["summaries"]))}), "P1-R12") == []


@pytest.mark.parametrize("value", ["NaN", "Infinity", " 100 ", "not-a-number"])
def test_malformed_declared_decimal_is_invalid_evidence(value):
    body = tape()
    body["items"][0]["quantity_mt"] = value
    with pytest.raises(compare.ContractError):
        differences(response(body), response(body))


def test_only_reviewed_paths_normalize_and_errors_stay_exact():
    assert differences(response({"note": "750.00"}), response({"note": "750"}))
    assert differences(response({"detail": "750.00"}, status=422), response({"detail": "750"}, status=422))
    assert differences(response({"detail": "ordinary error a"}, status=500), response({"detail": "ordinary error b"}, status=500))


def test_csv_and_non_json_auxiliary_bodies_remain_exact():
    left = response(b'price\n750.00\n', headers={"content-type": "text/csv"})
    assert differences(left, left, "P1-R14") == []
    assert differences(left, {**left, "body": b'price\n750\n'}, "P1-R14")
    plain = response(b'Method Not Allowed', status=405, headers={"content-type": "text/plain"})
    assert differences(plain, plain, "P1-strict-auxiliary") == []


def mcp(payload):
    return {"jsonrpc": "2.0", "id": 8, "result": {"content": [{"type": "text", "text": json.dumps(payload)}], "structuredContent": payload, "isError": False}}


def test_mcp_inner_json_uses_the_same_typed_rules_not_text_sorting():
    left = mcp({"bid_quantity_mt": "100.00", "observed_at": STAMP})
    right = mcp({"observed_at": STAMP.replace("Z", "+00:00"), "bid_quantity_mt": "1e2"})
    assert differences(response(left), response(right), "P1-tool-get_market_slice") == []
    right["result"]["content"][0]["text"] = json.dumps({"bid_quantity_mt": "101", "observed_at": STAMP})
    assert differences(response(left), response(right), "P1-tool-get_market_slice")


def test_mcp_diagnostics_preserve_kind_location_input_and_envelope():
    from pydantic import ValidationError

    from app.mcp_server import build_mcp_server

    arguments = {"market_product": 42, "delivery_point": "Singapore", "availability_window": "SPOT"}
    metadata = build_mcp_server(read_limiter=None)._tool_manager.get_tool("get_market_slice").fn_metadata
    with pytest.raises(ValidationError) as caught:
        metadata.arg_model.model_validate(metadata.pre_parse_json(arguments))
    errors = json.loads(caught.value.json(include_url=False, include_context=False))
    text = "Error executing tool get_market_slice: " + str(caught.value)
    stable = "Validation failed: " + json.dumps(errors)
    left = {"jsonrpc": "2.0", "id": 8, "result": {"content": [{"type": "text", "text": text}], "isError": True}}
    right = copy.deepcopy(left)
    right["result"]["content"][0]["text"] = stable
    context = {"tool": "get_market_slice", "arguments": arguments}
    assert differences(response(left), response(right), "P1-tool-get_market_slice", diagnostic_context=context) == []
    for field, value in [("type", "missing"), ("loc", ["delivery_point"]), ("input", 43)]:
        changed = copy.deepcopy(right)
        mutated = copy.deepcopy(errors)
        mutated[0][field] = value
        changed["result"]["content"][0]["text"] = "Validation failed: " + json.dumps(mutated)
        assert differences(response(left), response(changed), "P1-tool-get_market_slice", diagnostic_context=context)
    # A diagnostic presented as successful machine-readable tool data is
    # invalid evidence, not an equal result or an ordinary JSON difference.
    with pytest.raises(compare.ContractError):
        differences(response(left), response({**right, "result": {**right["result"], "isError": False}}), "P1-tool-get_market_slice", diagnostic_context=context)
    changed_envelope = copy.deepcopy(right)
    changed_envelope["id"] = 9
    assert differences(response(left), response(changed_envelope), "P1-tool-get_market_slice", diagnostic_context=context)


def test_unknown_profile_route_digest_and_ledger_refuse(tmp_path):
    for options in ({"route_id": "unknown"}, {"rules_sha256": "0" * 64}):
        args = {"profile": "phase1-meaning-v1", "route_id": "P1-R06", "rules_sha256": hashlib.sha256(RULES.read_bytes()).hexdigest() if RULES.exists() else ""}
        args.update(options)
        with pytest.raises(compare.ContractError):
            compare.compare_responses(response({}), response({}), **args)
    if not RULES.exists():
        pytest.fail("missing immutable Phase 1 ledger")
    path = tmp_path / "rules.json"
    path.write_text(RULES.read_text().replace("P1-R06", "P1-widened"))
    with pytest.raises(compare.ContractError):
        differences(response({}), response({}), rules_path=path)


def test_inventory_scalar_paths_match_the_reviewed_ledger():
    assert RULES.exists(), "missing reviewed ledger"
    rules = json.loads(RULES.read_text())["routes"]
    inventory = json.loads((RULES.parent / "routes.json").read_text())
    for route in inventory["routes"] + inventory["tools"]:
        reviewed = rules[route["comparator_route_id"]]
        for field in ("decimal_paths", "datetime_paths"):
            assert route[field] == reviewed[field]
        assert "scalar_rule_status" not in route

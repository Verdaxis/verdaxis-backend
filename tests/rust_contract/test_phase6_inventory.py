"""Validate the stream route, client and required-evidence inventories."""

from __future__ import annotations

import copy
import json
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/rust_contract/phase6"
ROUTES = [
    ("S61", "/api/stream/prices", "public"),
    ("S62", "/api/stream/orderbook", "public"),
    ("S63", "/api/stream/trades", "stream_token"),
]
TERMINALS = [
    ("T01", "reset", "subscriber_overflow", True, "keep"),
    ("T02", "reset", "replay_retention", True, "clear"),
    ("T03", "reset", "cursor_ahead", True, "clear"),
    ("T04", "auth_expired", "token_expired", False, "keep"),
    ("T05", "auth_revoked", "authorization_changed", True, "clear"),
    ("T06", "error", "stream_unavailable", True, "keep"),
    ("T07", "error", "connection_capacity", False, "keep"),
    ("T08", "reset", "event_unavailable", True, "clear"),
    ("T09", "reset", "service_restarting", True, "clear"),
    ("T10", "auth_revoked", "authorization_unavailable", True, "clear"),
]
TARGETS = {
    "streams_store", "streams_bus", "streams_auth", "streams_election",
    "streams_dispatch", "streams_replay", "streams_http", "streams_runtime",
    "streams_watchlist_guard",
}


def load(name):
    path = FIXTURES / name
    assert path.is_file(), f"missing stream contract: {name}"
    return json.loads(path.read_text())


def validate_required(document):
    cases = document["scenarios"]
    ids = [case["id"] for case in cases]
    assert len(ids) == len(set(ids))
    expected_ids = (
        {f"R{index:02}" for index in range(1, 16)}
        | {f"T{index:02}" for index in range(1, 11)}
        | {f"J{index}" for index in range(61, 69)}
        | {f"E{index:03}" for index in range(1, 202)}
    )
    assert set(ids) == expected_ids
    assert {case["task"] for case in cases} == {
        "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11",
        "12", "13", "14a", "14b", "15a", "15b",
    }
    for case in cases:
        assert case["required"] is True
        assert case["scopes"]
        assert set(case["scopes"]) <= {"candidate", "cutover-rehearsal"}
        assert case["test"] and case["target"] and case["description"]
    assert set(document["rust_targets"]) == TARGETS
    import tomllib

    manifest = tomllib.loads((ROOT / "rs/read/Cargo.toml").read_text())
    registered = {target["name"] for target in manifest["test"]}
    assert TARGETS <= registered
    for target in TARGETS:
        assert (ROOT / f"rs/read/tests/{target}.rs").is_file()
    assert document["result_policy"] == {
        "missing": "fail", "skipped": "fail", "compiled_only": "fail",
        "missing_exit": "fail", "cleanup_failure": "fail",
    }


def test_phase6_inventory_has_three_get_routes():
    inventory = load("routes.json")
    assert [(r["id"], r["path"], r["auth"]) for r in inventory["routes"]] == ROUTES
    for route in inventory["routes"]:
        assert route["method"] == "GET"
        assert route["aliases"] == [route["path"] + "/"]
        assert route["success_status"] == 200
        assert route["content_type"] == "text/event-stream"
        assert route["cutover_group"] == "streams"
    assert inventory["implicit_head"] is False


def test_activity_is_not_a_rust_route():
    inventory = load("routes.json")
    assert inventory["excluded"]["/api/stream/activity"] == {
        "owner": "python", "rust_handler": False, "rust_alias": False,
    }
    assert all("activity" not in r["path"] for r in inventory["routes"])


def test_issuer_is_phase3_owned():
    inventory = load("routes.json")
    assert inventory["excluded"]["/api/auth/stream-token"] == {
        "owner": "phase3", "issued_lifetime_seconds": 60,
        "endpoint_per_minute": 30,
    }
    phase3 = json.loads((ROOT / "tests/rust_contract/phase3/routes.json").read_text())
    assert "/api/auth/stream-token" in json.dumps(phase3)


def test_scenario_inventory_complete():
    document = load("required-scenarios.json")
    validate_required(document)
    client = load("client-contract.json")
    assert [
        (t["id"], t["event"], t["reason"], t["resync_required"], t["cursor"])
        for t in client["terminals"]
    ] == TERMINALS
    assert all(t["id_field"] is False and t["frames"] == 1 and t["close"]
               for t in client["terminals"])
    assert client["error_precedence"] == [
        "cursor", "authorization_conflict", "token_required", "token_claims", "account",
    ]
    assert client["public_payload"] == {"schema_version": 1, "resync_required": True}
    assert client["cursor"]["header_precedes_query"] is True
    assert client["cursor"]["maximum"] == 9223372036854775807
    assert client["heartbeat_max_seconds"] == 30


def test_retention_scenarios_select_real_tests():
    import ast

    target = "tests/postgres/test_phase6_retention.py"
    source = ast.parse((ROOT / target).read_text())
    names = {
        node.name for node in source.body
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
    }
    cases = [case for case in load("required-scenarios.json")["scenarios"] if case["task"] == "2"]
    assert len(cases) == 25
    for case in cases:
        assert case["target"] == target
        assert case["test"] in names
        assert case["test"].startswith("test_")


def test_accepted_scenarios_bind_distinct_source_tests():
    import ast
    import re

    accepted_tasks = {"1", "2", "3", "4", "5", "10", "15a"}
    identities = set()
    for case in load("required-scenarios.json")["scenarios"]:
        if case["task"] not in accepted_tasks:
            continue
        assert case["repository"] == "backend"
        assert case["runner"] in {"cargo", "pytest"}
        assert case["red"]
        path = case["test_path"]
        name = case["test_name"]
        identity = (path, name)
        assert identity not in identities
        identities.add(identity)
        source = (ROOT / path).read_text()
        if case["runner"] == "pytest":
            names = {
                node.name for node in ast.parse(source).body
                if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
            }
        else:
            names = set(re.findall(
                r"#\[(?:tokio::)?test(?:\([^\n]*\))?\]\s*(?:async\s+)?fn\s+(\w+)",
                source,
            ))
        assert name in names, identity
        for support in case.get("support_paths", []):
            assert (ROOT / support).is_file()
        if "suite" in case:
            assert case["suite"] in load("required-scenarios.json")["postgres_suites"]
        if "missing_interface" in case["red"]:
            assert case["missing_interface"]["code"] == "E0432"
            assert case["missing_interface"]["symbol"]


def test_required_scenarios_are_not_optional():
    document = load("required-scenarios.json")
    validate_required(document)
    for index in range(len(document["scenarios"])):
        altered = copy.deepcopy(document)
        altered["scenarios"][index]["required"] = False
        with pytest.raises(AssertionError):
            validate_required(altered)
    for case in document["scenarios"]:
        altered = copy.deepcopy(document)
        altered["scenarios"] = [c for c in altered["scenarios"] if c["id"] != case["id"]]
        with pytest.raises(AssertionError):
            validate_required(altered)


def test_no_trusted_producer_activation():
    inventory = load("routes.json")
    assert inventory["watchlist"] == {
        "producer_owner": "phase8", "producer_enabled": False,
        "feed_and_metrics_guard": "preserved",
    }
    streams = ROOT / "rs/read/src/streams/mod.rs"
    assert streams.is_file(), "stream module declaration is required"
    assert "pub mod streams;" in (ROOT / "rs/read/src/lib.rs").read_text()
    assert "start_streams(" not in (ROOT / "rs/read/src/main.rs").read_text()

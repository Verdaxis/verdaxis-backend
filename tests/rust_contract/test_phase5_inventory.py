"""Static inventory checks for the next read-route group, not runtime gates."""

import ast
import json
import re
import subprocess
from collections import Counter
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
CONTRACT = ROOT / "tests/rust_contract/phase5"
ROUTES = (
    ("M01", "GET", "/api/benchmarks", "public", "B"),
    ("M02", "GET", "/api/availability", "public", "A"),
    ("M03", "GET", "/api/demand", "public", "D"),
    ("M04", "GET", "/api/fleet-intelligence", "public", "F"),
    ("M05", "GET", "/api/producers", "public", "P"),
    ("C01", "GET", "/api/compliance/fleet", "current_user", "V"),
    ("C02", "GET", "/api/compliance/vessels/{vessel_id}/score", "current_user", "V"),
    ("C03", "GET", "/api/compliance/fuels", "public", "C-score"),
    ("C04", "POST", "/api/compliance/pricing-overlay", "current_user", "O"),
    ("C05", "POST", "/api/compliance/scenario", "current_user", "V"),
)


def load(name):
    path = CONTRACT / name
    assert path.is_file(), f"missing read contract: {name}"
    return json.loads(path.read_text())


def validate_inventory(document):
    assert document["schema_version"] == 1
    assert document["stage"] == "inventory"
    for key in ("backend_sha", "frontend_sha", "canonical_frontend_sha"):
        assert re.fullmatch(r"[0-9a-f]{40}", document[key])
    rows = document["routes"]
    assert [tuple(row[key] for key in ("id", "method", "path", "auth", "owner")) for row in rows] == list(ROUTES)
    assert Counter(row["method"] for row in rows) == {"GET": 8, "POST": 2}
    assert Counter(row["auth"] for row in rows) == {"public": 6, "current_user": 4}
    assert all(row["business_writes"] is False for row in rows)
    assert all(row["status"] == "planned" for row in rows)
    assert document["operator_tasks"] == []
    excluded = document["excluded_routes"]
    assert len(excluded) == 27
    assert len({row["path"] for row in excluded}) == 27
    assert {row["id"] for row in excluded} == {f"R{i:02}" for i in range(1, 28)}
    assert all(row["method"] == "GET" for row in excluded)
    tree = ast.parse((ROOT / "tests/rust_contract/test_inventory.py").read_text())
    planned = next(ast.literal_eval(node.value) for node in tree.body if isinstance(node, ast.Assign) and any(isinstance(target, ast.Name) and target.id == "PLANNED" for target in node.targets))
    assert [(row["id"], row["method"], row["path"]) for row in excluded] == [row for row in planned if row[0].startswith("R")]
    assert not ({row["path"] for row in excluded} & {row["path"] for row in rows})
    ownership = document["file_owners"]
    assert ownership and len({row["path"] for row in ownership}) == len(ownership)
    assert all(row["owner"] and isinstance(row["owner"], str) for row in ownership)
    assert all(".." not in Path(row["path"]).parts for row in ownership)
    by_path = {row["path"]: row["owner"] for row in ownership}
    for filename in ("routes.json", "client-contract.json", "python-rules-checklist.md", "evidence-manifest.schema.json", "evidence-manifest.json"):
        assert by_path[f"tests/rust_contract/phase5/{filename}"] == "I"
    assert by_path["tests/rust_contract/phase5/fixtures/**"] == "Q"
    assert by_path["rs/read/src/auth/transaction.rs"] == "I"
    assert document["bounds"] == {
        "availability_groups": 512, "demand_output": 512,
        "demand_candidate_groups": 32768, "producer_rows": 5000,
        "fleet_vessels": 2000, "overlay_ids": 100, "body_bytes": 65536,
        "mix_keys": 32, "mix_key_bytes": 64, "coefficient_digits": 18,
        "scale": 12, "exponent_min": -12, "exponent_max": 0,
        "label_scalars": 128, "label_bytes": 512, "year_min": 2025,
        "year_max": 9999, "artifact_bytes": 262144, "artifact_entries": 32,
        "artifact_sources": 16, "source_bytes": 2048, "fuel_bytes": 64,
        "color_bytes": 32, "count_max": 2147483647,
        "artifact_deadline_seconds": 2, "statement_timeout_ms": 5000,
        "actor_lock_timeout_ms": 500,
    }
    assert document["schema_policy"] == {
        "migration_count": 1, "kind": "additive_indexes",
        "parent": "single_accepted_head_at_implementation",
        "indexes": {"ix_p5_vessels_org_id": ["organization_id", "id"],
                    "ix_p5_producers_cod_name_id": ["cod_year", "name", "id"],
                    "ix_p5_producers_status_cod_id": ["status", "cod_year", "id"]},
    }


def test_phase5_inventory_cases():
    validate_inventory(load("routes.json"))


@pytest.mark.parametrize("change", ["route", "auth", "operator", "bound", "owner", "schema"])
def test_phase5_inventory_rejects_incomplete_contract(change):
    document = load("routes.json")
    if change == "route":
        document["routes"].pop()
    elif change == "auth":
        document["routes"][-1]["auth"] = "public"
    elif change == "operator":
        document["operator_tasks"] = ["import"]
    elif change == "bound":
        document["bounds"]["producer_rows"] += 1
    elif change == "owner":
        document["file_owners"].append(document["file_owners"][0])
    else:
        document["schema_policy"]["migration_count"] = 2
    with pytest.raises(AssertionError):
        validate_inventory(document)


def test_phase5_client_contract_cases():
    document = load("client-contract.json")
    assert document["schema_version"] == 1
    assert set(document["routes"]) == {row[0] for row in ROUTES}
    models = document["models"]
    for route in document["routes"].values():
        assert route["response_model"] in models
        assert route["success_status"] == 200
        assert route["consumer_sources"]
    for name, fields in models.items():
        assert fields, name
        assert all(isinstance(value, str) and value for value in fields.values())
    assert models["Availability"]["total_stock_mt"] == "decimal_string|null"
    assert models["Demand"]["volume_mt"] == "decimal_string|null"
    assert models["Producer"]["cod_year"] == "integer|null"
    assert models["EUETS"]["estimated_cost_eur"] == "decimal_string|null"
    assert models["OverlayResponse"]["overlays"] == "map<uuid,ListingOverlay|null>"
    assert document["error"] == {"detail": {"code": "UPPER_SNAKE_CODE", "message": "string"}}
    assert set(document["required_client_scenarios"]) == {
        "market_read_wrapper_cases", "compliance_marketplace_cases",
        "compliance_dashboard_cases", "fleet_display_states", "scenario_label_cases",
    }


def test_phase5_rule_checklist_cases():
    path = CONTRACT / "python-rules-checklist.md"
    assert path.is_file(), "missing scoped rule checklist"
    text = path.read_text()
    rows = [
        [cell.strip() for cell in line.strip("|").split("|")]
        for line in text.splitlines() if re.match(r"^\| P5-\d+ \|", line)
    ]
    assert rows
    assert len({row[0] for row in rows}) == len(rows)
    assert all(row[3] and row[4] in {"check", "reference"} for row in rows)
    for source, expected_count in (("test_compliance_scoring.py", 40), ("test_compliance_pricing.py", 19)):
        tree = ast.parse((ROOT / "tests/unit" / source).read_text())
        definitions = [node for node in ast.walk(tree) if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name.startswith("test_")]
        mapped = [row for row in rows if f"tests/unit/{source}:" in row[1]]
        assert len(definitions) == expected_count == len(mapped)
        assert {int(row[1].rsplit(":", 1)[1]) for row in mapped} == {node.lineno for node in definitions}
    assert "306" not in text


def test_phase5_source_identity_cases():
    document = load("routes.json")
    subprocess.run(["git", "cat-file", "-e", document["backend_sha"] + "^{commit}"], cwd=ROOT, check=True)
    for route in document["routes"]:
        assert (ROOT / route["source"]).is_file()
    actual = set()
    for source in {row["source"] for row in document["routes"]}:
        tree = ast.parse((ROOT / source).read_text())
        for node in ast.walk(tree):
            if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            for decorator in node.decorator_list:
                if isinstance(decorator, ast.Call) and isinstance(decorator.func, ast.Attribute) and decorator.func.attr in {"get", "post"}:
                    if decorator.args and isinstance(decorator.args[0], ast.Constant):
                        path = decorator.args[0].value
                        mount = next(row["path"] for row in document["routes"] if row["source"] == source)
                        prefix = "/api/compliance" if source.endswith("compliance_api.py") else mount
                        actual.add((decorator.func.attr.upper(), prefix + path))
    assert actual == {(row[1], row[2]) for row in ROUTES}

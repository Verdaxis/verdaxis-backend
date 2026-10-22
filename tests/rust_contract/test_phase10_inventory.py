"""Phase 10 source inventory. Parse source as data; never import the application."""

import ast
import hashlib
import json
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/rust_contract/phase10"
METHODS = {"get", "post", "put", "patch", "delete", "head", "options"}
UNIT_ROOTS = ("deploy/systemd", "deploy/monitor", "deploy/external_monitor/systemd")
HELPERS = {
    "verify_migration_revision.py", "preflight_runtime.py", "apply_migration_checkpoint.py",
    "converge_runtime_acls.py", "verify_deploy_start_guard.py", "validate_health_response.py",
    "verify_systemd_source.py", "verify_rust_artifact.py", "prepare_rust_budget.py",
    "prepare_rust_runtime.py", "verify_rust_start.py", "rust_cache_authority.py",
    "rust_caddy_candidate.py", "rust_passive_no_harm.py",
}


def document(name):
    path = FIXTURES / f"{name}.json"
    assert path.is_file(), f"missing disposition/evidence inventory: {name}"
    return json.loads(path.read_text())


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def literal(node):
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    if isinstance(node, ast.Attribute) and ast.unparse(node) == "settings.API_V1_STR":
        return "/api"
    raise AssertionError(f"unresolved static route expression: {ast.unparse(node)}")


def keyword(call, name, default=""):
    return next((literal(item.value) for item in call.keywords if item.arg == name), default)


def source_routes(root=ROOT):
    """Resolve only routers actually mounted by main, including call-style registrations."""
    main_path = root / "app/main.py"
    main = ast.parse(main_path.read_text())
    imports = {
        alias.asname or alias.name: item.module.replace(".", "/") + ".py"
        for item in main.body if isinstance(item, ast.ImportFrom)
        and item.module and item.module.startswith("app.routers.")
        for alias in item.names if alias.name == "router"
    }
    mounts = []
    for node in ast.walk(main):
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute):
            if ast.unparse(node.func) == "app.include_router":
                mounts.append((imports[node.args[0].id], keyword(node, "prefix")))
    assert len(mounts) == 38, "mounted router inventory changed"
    result = []
    for source, mount_prefix in [("app/main.py", ""), *mounts]:
        tree = ast.parse((root / source).read_text())
        router_prefix = ""
        for node in ast.walk(tree):
            if isinstance(node, ast.Call) and ast.unparse(node.func) == "APIRouter":
                router_prefix = keyword(node, "prefix")
        for node in ast.walk(tree):
            if not isinstance(node, ast.Call) or not isinstance(node.func, ast.Attribute):
                continue
            owner = ast.unparse(node.func.value)
            if owner not in {"router", "app"} or node.func.attr not in METHODS:
                continue
            path = mount_prefix + router_prefix + literal(node.args[0])
            method = node.func.attr.upper()
            result.append({"id": f"route.{method} {path}", "source": source,
                           "method": method, "path": path, "line": node.lineno})
    return sorted(result, key=lambda row: row["id"])


def source_units(root=ROOT):
    return sorted(str(path.relative_to(root)) for base in UNIT_ROOTS
                  for path in (root / base).iterdir() if path.suffix in {".service", ".timer"})


def source_scripts(root=ROOT):
    return sorted(str(path.relative_to(root)) for path in (root / "scripts").iterdir()
                  if path.suffix in {".py", ".sh"})


def unique_rows(rows):
    indexed = {row["id"]: row for row in rows}
    assert len(indexed) == len(rows), "duplicate artifact id"
    return indexed


def validate_dispositions(rows, scenarios):
    unique_rows(rows)
    for row in rows:
        assert row["owner"].strip(), f"ownerless: {row['id']}"
        assert row["end_state"] in {"rust", "shell", "gone", "retired-offhost"}
        assert row["status"] in {"planned", "accepted"}
        assert row["source_sha256"] and len(row["source_sha256"]) == 64
        assert row["rollback"] and row["installed_check"]
        if row["end_state"] in {"rust", "shell"}:
            assert row["evidence"], f"untested: {row['id']}"
            assert set(row["evidence"]) <= scenarios, f"unknown scenario: {row['id']}"
        elif row["end_state"] == "gone":
            assert row["deleting_task"] in {20, 26, 32}
            assert row["last_reference_change"], f"no removal seam: {row['id']}"
        else:
            assert row["offhost_location"]


def validate_source(snapshot, ledger, root=ROOT):
    """Keep the snapshot after deletion; require explicit accepted removal evidence."""
    indexed = unique_rows(ledger["artifacts"])
    for row in snapshot:
        item = indexed[row["id"]]
        path = root / row["source"]
        if ledger["phase"] == "pre-removal":
            assert path.is_file(), f"missing source before removal: {path}"
            assert digest(path) == row["source_sha256"], f"source drift: {path}"
        else:
            assert ledger["phase"] == "post-removal"
            assert item["status"] == "accepted"
            proof = item.get("deletion_evidence")
            assert proof, f"missing deletion evidence: {row['id']}"
            evidence = root / proof["path"]
            assert evidence.is_file() and digest(evidence) == proof["sha256"]
            if item["end_state"] == "gone":
                assert not path.exists(), f"deleted artifact remains: {path}"
                for reference in item.get("reference_tokens", [row["source"]]):
                    for base in ("deploy",):
                        for candidate in (root / base).rglob("*"):
                            if candidate.is_file():
                                assert reference not in candidate.read_text(errors="replace"), (
                                    f"removed artifact still referenced: {candidate}"
                                )


def test_phase10_routes_are_exactly_188():
    rows = document("routes")["artifacts"]
    assert len(rows) == 188, "missing route disposition rows"
    assert len(unique_rows(rows)) == 188
    assert sum(row["source"] == "app/main.py" for row in rows) == 4
    assert sum(row["source"] == "app/routers/alerts.py" for row in rows) == 3
    ledger = document("removal-ledger")
    if ledger["phase"] == "pre-removal":
        expected = source_routes()
        assert [{key: row[key] for key in expected[0]} for row in rows] == expected
    assert sum(row["deleting_task"] == 20 for row in rows) == 20
    validate_source(rows, ledger)


def test_phase10_units_are_exactly_41():
    rows = document("units")["artifacts"]
    assert len(rows) == 41, "missing unit disposition rows"
    assert [sum(row["source"].startswith(base + "/") for row in rows)
            for base in UNIT_ROOTS] == [20, 11, 10]
    ledger = document("removal-ledger")
    if ledger["phase"] == "pre-removal":
        assert sorted(row["source"] for row in rows) == source_units()
    validate_source(rows, ledger)


def test_phase10_scripts_are_exactly_40():
    rows = document("scripts")["artifacts"]
    assert len(rows) == 40, "missing script disposition rows"
    assert sum(row["source"].endswith(".py") for row in rows) == 35
    ledger = document("removal-ledger")
    if ledger["phase"] == "pre-removal":
        assert sorted(row["source"] for row in rows) == source_scripts()
    validate_source(rows, ledger)


def test_phase10_helpers_are_exactly_14():
    rows = [row for row in document("removal-ledger")["artifacts"] if row["kind"] == "helper"]
    assert len(rows) == 14, "missing helper disposition rows"
    assert {Path(row["source"]).name for row in rows} == HELPERS


def test_phase10_no_row_is_ownerless_or_untested():
    ledger = document("removal-ledger")
    manifest = document("evidence-manifest")
    scenarios = manifest["required_scenarios"]
    assert scenarios and len(set(scenarios)) == len(scenarios)
    validate_dispositions(ledger["artifacts"], set(scenarios))
    indexed = unique_rows(ledger["artifacts"])
    for name in ("routes", "units", "scripts"):
        for row in document(name)["artifacts"]:
            assert indexed[row["id"]] == row
    assert sum(row["kind"] == "cli" for row in indexed.values()) == 5
    validate_source(list(indexed.values()), ledger)
    assert manifest["phase7_decision"] == "retire-routes-preserve-tables"
    assert manifest["results"] == [], "Task 1 is an evidence plan, not gate acceptance"


def test_phase10_inventory_rejects_missing_and_duplicate_dispositions():
    rows = document("removal-ledger")["artifacts"]
    assert rows, "missing disposition rows"
    with pytest.raises(AssertionError, match="duplicate"):
        unique_rows([rows[0], rows[0]])
    broken = dict(rows[0], owner="")
    with pytest.raises(AssertionError, match="ownerless"):
        validate_dispositions([broken], set())


def test_phase10_post_removal_requires_evidence(tmp_path):
    row = {"id": "script.example", "source": "scripts/example.py", "source_sha256": "0" * 64}
    ledger = {"phase": "post-removal", "artifacts": [dict(row, status="accepted", end_state="gone")]}
    with pytest.raises(AssertionError, match="missing deletion evidence"):
        validate_source([row], ledger, tmp_path)

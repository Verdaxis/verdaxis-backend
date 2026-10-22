"""Requirements for the market workspace; no later implementation is needed here."""

import ast
import copy
import json
import os
import re
import subprocess
import textwrap
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
LEDGER = ROOT / "tests/rust_contract/phase8/required-scenarios.json"
BASE = "274bd3ab737621339d158ab4795578009de5bf4f"
ROUTE_IDS = ["O3", "O4", "O5", "O6", "I1", "I2", "I3", "I4", "I7", "S1", "S2", "S3", "S4", "S5", "S6", "S7", "S8", "S9", "S10", "S11", "S12", "S13", "S14", "S15", "S16", "C1", "C2", "C3"]
# Requirements are independent of test discovery and of the ledger being checked.
# The W prefix keeps these requirement IDs distinct from the plan's R1-R28 race IDs.
REQUIRED = {
    route: {f"W{index:02d}-success", f"W{index:02d}-admission"}
    for index, route in enumerate(ROUTE_IDS, 1)
}
REQUIRED.update({
    "C1": REQUIRED["C1"] | {"W26-empty", "W26-1001", "W26-legacy-only"},
    "O3": REQUIRED["O3"] | {"W01-replay", "W01-etag", "W01-reservation", "W01-context-scope", "W01-admin-without-context"},
    "O4": REQUIRED["O4"] | {"W02-reason", "W02-shared-cancel", "W02-context-invalid", "W02-admin-without-context"},
    "O5": REQUIRED["O5"] | {"W03-context-invalid"},
    "O6": REQUIRED["O6"] | {"W04-context-invalid"},
    "I7": REQUIRED["I7"] | {"W09-no-organization"},
    "S6": REQUIRED["S6"] | {"W15-reuse", "W15-replacement", "W15-expiry"},
    "S7": REQUIRED["S7"] | {"W16-stale-expiry"},
    "S8": REQUIRED["S8"] | {"W17-stale-expiry"},
})
DEFERRED_REQUIREMENTS = {
    f"W{index:02d}": {f"W{index:02d}-invariant"} for index in range(29, 41)
}
# Source anchors the support-context refusal contract depends on (M2 fix round).
REQUIRED_GUARDS = {
    "O3": {"_reject_admin_legacy_workspace_mutation", "MARKET_SUPPORT_CONTEXT_HEADER", "resolve_request_party"},
    "O4": {"_reject_admin_legacy_workspace_mutation", "MARKET_SUPPORT_CONTEXT_HEADER", "resolve_request_party"},
    "O5": {"MARKET_SUPPORT_CONTEXT_HEADER", "resolve_request_party"},
    "O6": {"MARKET_SUPPORT_CONTEXT_HEADER", "resolve_request_party"},
}
# Coded refusals the pinned client contract reads by exact code (M1 fix round).
REQUIRED_REFUSAL_CODES = {
    "S6": {"MARKET_SUPPORT_CONTEXT_REPLACEMENT_REQUIRED", "ORGANIZATION_NOT_APPROVED", "ORGANIZATION_NOT_REAL"},
    "O3": {"MARKET_SUPPORT_MUTATION_NOT_ALLOWED"},
    "O4": {"MARKET_SUPPORT_CONTEXT_INVALID", "MARKET_SUPPORT_LEGACY_MUTATION_RETIRED"},
    "O5": {"MARKET_SUPPORT_CONTEXT_INVALID"},
    "O6": {"MARKET_SUPPORT_CONTEXT_INVALID"},
}
CONTEXT_CLEARING_HEADERS = ["X-Verdaxis-Market-Support-Context-Invalid", "X-Verdaxis-Market-Support-Context-Code"]


def load_ledger():
    if not LEDGER.exists():
        return {"routes": [], "invariants": []}
    return json.loads(LEDGER.read_text())


def validate_refusals(ledger):
    for row in ledger["routes"]:
        seen = set()
        for refusal in row["refusals"]:
            key = (refusal["status"], refusal["code"], refusal["condition"])
            assert key not in seen, f"{row['route_id']}: duplicate refusal {key}"
            seen.add(key)
            code = refusal["code"]
            if code is not None:
                assert re.fullmatch(r"[A-Z][A-Z0-9_]*", code), f"{row['route_id']}: malformed code {code!r}"
            if code is not None and code.startswith("MARKET_SUPPORT_CONTEXT_"):
                # app/main.py: every MARKET_SUPPORT_CONTEXT_* code carries both headers.
                assert refusal.get("clearing_headers") == CONTEXT_CLEARING_HEADERS, (
                    f"{row['route_id']}: {code} must carry both clearing headers"
                )
            else:
                assert "clearing_headers" not in refusal, f"{row['route_id']}: unexpected clearing headers"
        required_codes = REQUIRED_REFUSAL_CODES.get(row["route_id"], set())
        assert required_codes <= {refusal["code"] for refusal in row["refusals"] if refusal["code"]}, (
            f"{row['route_id']}: missing refusal codes {sorted(required_codes)}"
        )


def validate_scenarios(ledger):
    routes = ledger["routes"]
    ids = [row["route_id"] for row in routes]
    assert len(ids) == len(set(ids)), "duplicate route IDs"
    assert set(ids) == set(REQUIRED), f"missing route IDs: {sorted(set(REQUIRED) - set(ids))}; unexpected: {sorted(set(ids) - set(REQUIRED))}"
    for row in routes:
        missing = REQUIRED[row["route_id"]] - set(row["required_scenarios"])
        assert not missing, f"{row['route_id']} missing scenario IDs: {sorted(missing)}"
        assert row["owning_task"] in {7, 8, 9, 10, 11}
    invariants = ledger["invariants"]
    assert {row["scenario_id"] for row in invariants} == set(DEFERRED_REQUIREMENTS)
    assert len(invariants) == len(DEFERRED_REQUIREMENTS)
    for row in invariants:
        assert set(row["required_scenarios"]) == DEFERRED_REQUIREMENTS[row["scenario_id"]]
        assert row["owning_tasks"] and row["description"]
    validate_refusals(ledger)


def test_phase8_scenario_ledger_covers_every_route():
    validate_scenarios(load_ledger())


def test_phase8_ledger_detects_a_removed_requirement():
    ledger = copy.deepcopy(load_ledger())
    ledger["routes"][0]["required_scenarios"].remove("W01-reservation")
    with pytest.raises(AssertionError, match="W01-reservation"):
        validate_scenarios(ledger)


def test_phase8_ledger_detects_a_dropped_clearing_header():
    ledger = copy.deepcopy(load_ledger())
    row = next(item for item in ledger["routes"] if item["route_id"] == "O4")
    refusal = next(item for item in row["refusals"] if item["code"] == "MARKET_SUPPORT_CONTEXT_INVALID")
    del refusal["clearing_headers"]
    with pytest.raises(AssertionError, match="clearing headers"):
        validate_scenarios(ledger)


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
            if method not in {"GET", "POST", "PATCH", "PUT", "DELETE"}:
                continue
            key = (method, "/api" + prefix + ast.literal_eval(decorator.args[0]))
            assert key not in routes
            routes[key] = (node, decorator)
    return routes


def test_phase8_route_inventory_matches_source():
    ledger = load_ledger()
    inventory = {}
    for module in ("orderbook", "inventory", "market_support", "orders"):
        inventory.update(source_routes(ROOT / f"app/routers/{module}.py"))
    pairs = set()
    for row in ledger["routes"]:
        pair = (row["method"], row["path"])
        assert pair not in pairs
        pairs.add(pair)
        assert pair in inventory, f"unregistered source pair: {pair}"
        node, decorator = inventory[pair]
        source = row["source"]
        assert source["handler"] == node.name
        assert source["line"] == node.lineno
        assert source["path"] == f"app/routers/{row['source_module']}.py"
        dependencies = {
            ast.unparse(call.args[0])
            for call in ast.walk(node.args)
            if isinstance(call, ast.Call) and ast.unparse(call.func) == "Depends" and call.args
        }
        assert row["auth_dependency"] in dependencies
        for guard in row["source_guards"]:
            assert guard in ast.unparse(node)
        assert REQUIRED_GUARDS.get(row["route_id"], set()) <= set(row["source_guards"]), (
            f"{row['route_id']}: missing required source anchors {sorted(REQUIRED_GUARDS.get(row['route_id'], set()) - set(row['source_guards']))}"
        )
        for field in ("auth_class", "request_schema", "response_schema", "refusals", "required_scenarios"):
            assert row[field], f"{row['route_id']}: empty {field}"
        response = next((ast.unparse(k.value) for k in decorator.keywords if k.arg == "response_model"), None)
        assert response == row["source_response_model"]
        annotations = {
            argument.arg: ast.unparse(argument.annotation)
            for argument in node.args.args + node.args.kwonlyargs
            if argument.annotation is not None
        }
        assert annotations == row["source_request_annotations"]
        assert set(row["scenario_descriptions"]) == set(row["required_scenarios"])
        assert all(row["scenario_descriptions"].values())
    assert len(pairs) == 28
    # All workspace and commission routes are in this phase, not just a selected subset.
    for module in ("market_support", "orders"):
        assert set(source_routes(ROOT / f"app/routers/{module}.py")) <= pairs


def test_phase8_expected_schema_manifest():
    manifest = load_ledger()["schema_change_set"]
    assert manifest["baseline_sha"] == BASE
    assert manifest["added_migrations"] == ["alembic/versions/p89_20261022_watchlist_producer.py"]
    assert manifest["modified_existing_migrations"] == []
    assert set(manifest["expected_modified_paths"]) == {
        "deploy/postgres/app_acl_policy.sql", "deploy/migration-checkpoints.tsv",
        "deploy/rust/allowed-revisions.json", "rs/read/src/runtime/artifact.rs",
        "rs/read/src/runtime/config.rs", "scripts/prepare_rust_runtime.py",
    }
    assert set(manifest["columns"]) == {
        "watchlist_events.producer", "watchlist_events.source_event_id",
        "watchlist_events.source_stream_seq", "watchlist_events.scope_attested_at",
        "watchlist_events.derived_event_key", "market_event_outbox.producer_evidence",
    }
    assert set(manifest["tables"]) == {"watchlist_producer_state"}
    assert set(manifest["constraints"]) == {"ck_watchlist_events_producer_identity"}
    assert set(manifest["indexes"]) == {"uq_watchlist_events_producer_source_target_derived"}
    permitted = manifest["permitted_migration_operations"]
    # Non-DDL calls are reviewed one by one: only the head check and the Task 5a
    # backfill statement are allowed, never op.execute in general.
    assert permitted["ddl"] == ["add_column", "create_table", "create_check_constraint", "create_index"]
    assert set(permitted["reviewed_calls"]) == {"op.get_bind", "op.get_context"}
    assert permitted["reviewed_statements"] == ["UPDATE watchlist_events SET producer = 'legacy-python'"]
    assert set(permitted["inline_checks"]) == {"watchlist_events.producer"}


def registered_rust_pairs(text):
    # Only literal axum route declarations are accepted. An unfamiliar assembly
    # form must extend this parser explicitly, not silently disappear from coverage.
    routes = re.findall(r'\.route\(\s*"([^"\n]+)"\s*,\s*(.*?)\)\s*[,)]', text, re.DOTALL)
    assert len(routes) == len(re.findall(r"\.route\s*\(", text)), "unparsed route registration"
    assert not re.search(r"\.(?:fallback|fallback_service|nest_service)\s*\(", text), "catch-all registration"
    pairs = []
    for path, methods in routes:
        assert "*" not in path, "catch-all path"
        verbs = re.findall(r"\b(get|post|put|patch|delete)\s*\(", methods)
        assert verbs, f"unparsed methods for {path}"
        pairs.extend((verb.upper(), path) for verb in verbs)
    assert len(pairs) == len(set(pairs)), "duplicate registration"
    return set(pairs)


def test_phase8_router_parser_rejects_extra_pairs_and_catchalls():
    assert registered_rust_pairs('Router::new().route("/x", get(read).post(write))') == {("GET", "/x"), ("POST", "/x")}
    with pytest.raises(AssertionError, match="catch-all"):
        registered_rust_pairs('Router::new().route("/{*tail}", get(read))')
    with pytest.raises(AssertionError, match="unparsed"):
        registered_rust_pairs('Router::new().route(PATH, get(read))')


@pytest.mark.skipif(os.environ.get("PHASE8_COMPLETENESS") != "1", reason="Task 14 gate: router and Task 5a schema are not Task 1 prerequisites")
def test_phase8_router_exposes_exact_pairs():
    paths = [ROOT / "rs/read/src/routes/market_write.rs"]
    paths.extend(sorted((ROOT / "rs/read/src/routes/market_write").glob("*.rs")))
    assert paths[0].is_file(), "market workspace assembly missing"
    actual = registered_rust_pairs("\n".join(path.read_text() for path in paths))
    expected = {(row["method"], row["path"]) for row in load_ledger()["routes"]}
    assert actual == expected, f"missing: {expected - actual}; extra: {actual - expected}"


def normalize_statement(statement):
    return re.sub(r"\s+", " ", statement).strip().rstrip(";").strip()


def executed_statement_text(call):
    """Literal SQL text of an op.execute argument, or None when not a literal."""
    argument = call.args[0] if call.args else None
    while (
        isinstance(argument, ast.Call)
        and getattr(argument.func, "attr", getattr(argument.func, "id", "")) in {"sa.text", "text"}
        and argument.args
    ):
        argument = argument.args[0]
    if isinstance(argument, ast.Constant) and isinstance(argument.value, str):
        return normalize_statement(argument.value)
    return None


def review_migration_operations(upgrade, manifest):
    """Refuse every op.* call the reviewed change set does not name exactly."""
    permitted = manifest["permitted_migration_operations"]
    reviewed_calls = {name.split(".", 1)[-1] for name in permitted["reviewed_calls"]}
    reviewed_statements = {normalize_statement(statement) for statement in permitted["reviewed_statements"]}
    violations = []
    for node in ast.walk(upgrade):
        if not (isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and ast.unparse(node.func.value) == "op"):
            continue
        operation = node.func.attr
        if operation in permitted["ddl"] or operation in reviewed_calls:
            continue
        if operation == "execute":
            statement = executed_statement_text(node)
            if statement not in reviewed_statements:
                violations.append(f"unreviewed op.execute: {statement!r}")
            continue
        violations.append(f"unreviewed migration operation: op.{operation}")
    return violations


def test_phase8_migration_reviewer_refuses_unreviewed_operations():
    manifest = load_ledger()["schema_change_set"]

    def upgrade_of(source):
        return next(
            node for node in ast.parse(textwrap.dedent(source)).body
            if isinstance(node, ast.FunctionDef) and node.name == "upgrade"
        )

    reviewed = '''\
        def upgrade():
            op.execute("UPDATE watchlist_events SET producer = 'legacy-python'")
        '''
    assert review_migration_operations(upgrade_of(reviewed), manifest) == []
    wrapped = '''\
        def upgrade():
            op.execute(sa.text("UPDATE watchlist_events SET producer = 'legacy-python'"))
        '''
    assert review_migration_operations(upgrade_of(wrapped), manifest) == []
    padded = '''\
        def upgrade():
            op.execute("UPDATE   watchlist_events SET producer = 'legacy-python' ;")
        '''
    assert review_migration_operations(upgrade_of(padded), manifest) == []
    reads = '''\
        def upgrade():
            bind = op.get_bind()
            op.get_context()
        '''
    assert review_migration_operations(upgrade_of(reads), manifest) == []
    foreign_delete = '''\
        def upgrade():
            op.execute(sa.text("DELETE FROM watchlist_events"))
        '''
    assert review_migration_operations(upgrade_of(foreign_delete), manifest) == ["unreviewed op.execute: 'DELETE FROM watchlist_events'"]
    foreign_update = '''\
        def upgrade():
            op.execute("UPDATE watchlist_events SET producer = 'attacker'")
        '''
    assert review_migration_operations(upgrade_of(foreign_update), manifest)
    not_literal = '''\
        def upgrade():
            op.execute(statement)
        '''
    assert review_migration_operations(upgrade_of(not_literal), manifest) == ["unreviewed op.execute: None"]
    destructive = '''\
        def upgrade():
            op.drop_column("watchlist_events", "producer")
        '''
    assert review_migration_operations(upgrade_of(destructive), manifest) == ["unreviewed migration operation: op.drop_column"]


@pytest.mark.skipif(os.environ.get("PHASE8_COMPLETENESS") != "1", reason="Task 14 gate: requires the integrated Task 5a revision")
def test_phase8_change_set_is_one_additive_revision():
    manifest = load_ledger()["schema_change_set"]
    schema_base = os.environ.get("PHASE8_SCHEMA_BASE", "")
    assert re.fullmatch(r"[0-9a-f]{40}", schema_base), "Task 5a must record the integrated predecessor SHA in PHASE8_SCHEMA_BASE"
    status_lines = subprocess.check_output(["git", "diff", "--name-status", schema_base, "HEAD", "--", "alembic/versions"], cwd=ROOT, text=True).splitlines()
    added = [line.split("\t")[-1] for line in status_lines if line.startswith("A")]
    # Other phases may add their own revisions after the recorded base; this gate
    # owns only the Phase 8 revision and refuses touching pre-existing files.
    assert added.count(manifest["added_migrations"][0]) == 1, "the Phase 8 revision must be added exactly once"
    assert not [line for line in status_lines if not line.startswith("A")], "pre-existing migrations must not be modified or removed"
    changed = set(subprocess.check_output(["git", "diff", "--name-only", BASE, "HEAD"], cwd=ROOT, text=True).splitlines())
    assert set(manifest["expected_modified_paths"]) <= changed
    tree = ast.parse((ROOT / manifest["added_migrations"][0]).read_text())
    upgrade = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "upgrade")
    assert review_migration_operations(upgrade, manifest) == []
    calls = [n for n in ast.walk(upgrade) if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute) and ast.unparse(n.func.value) == "op"]
    assert calls, "no additive operations found"
    columns = {ast.literal_eval(call.args[0]) + "." + ast.literal_eval(call.args[1].args[0]) for call in calls if call.func.attr == "add_column"}
    assert columns == set(manifest["columns"])
    for operation, field in (("create_table", "tables"), ("create_check_constraint", "constraints"), ("create_index", "indexes")):
        assert {ast.literal_eval(call.args[0]) for call in calls if call.func.attr == operation} == set(manifest[field])
    # The producer column carries its reviewed inline value check.
    producer_calls = [
        call for call in calls
        if call.func.attr == "add_column"
        and ast.literal_eval(call.args[0]) == "watchlist_events"
        and ast.literal_eval(call.args[1].args[0]) == "producer"
    ]
    assert len(producer_calls) == 1, "exactly one producer column addition"
    checks = [
        node for node in ast.walk(producer_calls[0])
        if isinstance(node, ast.Call) and getattr(node.func, "attr", getattr(node.func, "id", "")) == "CheckConstraint"
    ]
    assert checks, "the producer column must carry an inline value check"
    check_text = " ".join(ast.unparse(node) for node in checks)
    assert "legacy-python" in check_text and "rust-market-v1" in check_text, "the inline check must name both reviewed producer values"


@pytest.mark.asyncio
@pytest.mark.parametrize("row_count", [0, 1001])
@pytest.mark.parametrize("backend", ["sqlite", "postgresql"])
async def test_phase8_commission_array_is_unbounded(row_count, backend):
    from datetime import datetime, timezone
    from decimal import Decimal
    from types import SimpleNamespace
    from uuid import UUID

    from fastapi import FastAPI
    from httpx import ASGITransport, AsyncClient
    from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

    from app.database import get_db
    from app.models.orders import Commission
    from app.models.user import UserRole
    from app.routers.auth_simple import get_current_user
    from app.routers.orders import router

    schema = None
    if backend == "postgresql":
        from uuid import uuid4

        from sqlalchemy.engine import make_url

        raw_url = os.environ.get("PHASE8_DISPOSABLE_DATABASE_URL")
        if not raw_url:
            pytest.skip("PostgreSQL fixture requires an explicitly started disposable database on the Linux test host")
        url = make_url(raw_url)
        assert url.drivername == "postgresql+asyncpg"
        assert url.host == "127.0.0.1" and url.port not in {None, 5432, 5433}
        assert url.database.startswith("phase8_disposable_")
        assert os.environ.get("PHASE8_DISPOSABLE_ACK") == "isolated-container"
        schema = "commission_" + uuid4().hex
        engine = create_async_engine(url, connect_args={"server_settings": {"search_path": schema}})
    else:
        engine = create_async_engine("sqlite+aiosqlite:///:memory:")
    try:
        async with engine.begin() as connection:
            if schema:
                await connection.exec_driver_sql(f'CREATE SCHEMA "{schema}"')
                await connection.exec_driver_sql("CREATE TABLE trades (id uuid PRIMARY KEY)")
            await connection.run_sync(Commission.__table__.create)
        sessions = async_sessionmaker(engine, expire_on_commit=False)
        now = datetime(2026, 10, 22, tzinfo=timezone.utc)
        async with sessions() as session:
            session.add_all([
                Commission(id=UUID(int=index + 1), match_id=UUID(int=index + 2001), amount_usd=Decimal("1.25"), created_at=now, updated_at=now)
                for index in range(row_count)
            ])
            await session.commit()
        async def database():
            async with sessions() as session:
                yield session
        app = FastAPI()
        app.include_router(router, prefix="/api")
        app.dependency_overrides[get_db] = database
        app.dependency_overrides[get_current_user] = lambda: SimpleNamespace(role=UserRole.ADMIN)
        async with AsyncClient(transport=ASGITransport(app=app), base_url="http://test") as client:
            response = await client.get("/api/orders/admin/commissions")
        assert response.status_code == 200
        body = response.json()
        assert isinstance(body, list)
        assert len(body) == row_count
        assert len({row["id"] for row in body}) == row_count
        assert {row["match_id"] for row in body} == {str(UUID(int=index + 2001)) for index in range(row_count)}
        assert all("trade_id" not in row for row in body)
    finally:
        if schema:
            async with engine.begin() as connection:
                await connection.exec_driver_sql(f'DROP SCHEMA IF EXISTS "{schema}" CASCADE')
        await engine.dispose()

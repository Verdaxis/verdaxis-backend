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
# Source anchors the support-context refusal contract depends on.
REQUIRED_GUARDS = {
    "O3": {"_reject_admin_legacy_workspace_mutation", "MARKET_SUPPORT_CONTEXT_HEADER", "resolve_request_party"},
    "O4": {"_reject_admin_legacy_workspace_mutation", "MARKET_SUPPORT_CONTEXT_HEADER", "resolve_request_party"},
    "O5": {"MARKET_SUPPORT_CONTEXT_HEADER", "resolve_request_party"},
    "O6": {"MARKET_SUPPORT_CONTEXT_HEADER", "resolve_request_party"},
}
# Coded refusals the pinned client contract reads by exact code.
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
    scenarios = ledger["routes"][0]["required_scenarios"]
    assert "W01-reservation" in scenarios, "reservation requirement missing from fixture"
    scenarios.remove("W01-reservation")
    with pytest.raises(AssertionError, match="W01-reservation"):
        validate_scenarios(ledger)


def test_phase8_ledger_detects_a_dropped_clearing_header():
    ledger = copy.deepcopy(load_ledger())
    row = next(item for item in ledger["routes"] if item["route_id"] == "O4")
    refusals = [item for item in row["refusals"] if item["code"] == "MARKET_SUPPORT_CONTEXT_INVALID"]
    assert refusals, "O4 context-invalid refusal missing from fixture"
    assert "clearing_headers" in refusals[0], "clearing headers missing from fixture"
    del refusals[0]["clearing_headers"]
    with pytest.raises(AssertionError, match="clearing headers"):
        validate_scenarios(ledger)


def test_phase8_initial_capability_absence_is_status_only():
    for row in load_ledger()["routes"]:
        if row["route_id"] not in {"O4", "O5", "O6"}:
            continue
        refusals = [item for item in row["refusals"] if "capability absent" in item["condition"]]
        assert len(refusals) == 1, f"{row['route_id']}: initial capability refusal missing"
        assert refusals[0]["status"] == 404
        assert refusals[0]["code"] is None
        assert "clearing_headers" not in refusals[0]


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
    assert "permitted_migration_operations" in manifest, "reviewed migration operations missing from fixture"
    permitted = manifest["permitted_migration_operations"]
    # Non-DDL calls are reviewed one by one: only the head check and the Task 5a
    # backfill statement are allowed, never connection access in general.
    assert permitted["ddl"] == ["add_column", "create_table", "create_check_constraint", "create_index"]
    assert permitted["reviewed_calls"] == []
    assert permitted.get("module_imports") == ["from alembic import op", "import sqlalchemy as sa"]
    assert permitted.get("revision_names") == ["revision", "down_revision", "branch_labels", "depends_on"]
    assert permitted["head_check"] == {
        "form": "heads = op.get_bind().execute(sa.text(statement)).scalars().all()",
        "statement": "SELECT version_num FROM alembic_version",
    }
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


def review_migration_operations(module, manifest):
    """Default deny the complete module; accept only the reviewed static grammar."""
    if not isinstance(module, ast.Module):
        return ["expected complete migration module"]
    permitted = manifest["permitted_migration_operations"]
    imports = {ast.dump(ast.parse(source).body[0]) for source in permitted["module_imports"]}
    revision_names = set(permitted["revision_names"])
    constructors = {
        "Column", "Text", "String", "Integer", "BigInteger", "Boolean",
        "DateTime", "Numeric", "UUID", "JSON", "CheckConstraint",
        "PrimaryKeyConstraint", "UniqueConstraint", "ForeignKey", "text",
    }
    head_read = ast.parse(
        f'heads = op.get_bind().execute(sa.text({permitted["head_check"]["statement"]!r})).scalars().all()'
    ).body[0]
    empty_arguments = ast.dump(ast.parse("def f():\n    pass").body[0].args)
    violations = []
    seen_revisions = set()
    seen_functions = set()

    def literal(node):
        try:
            ast.literal_eval(node)
            return True
        except (ValueError, TypeError):
            return False

    def direct_attribute(node, receiver):
        return isinstance(node, ast.Attribute) and isinstance(node.value, ast.Name) and node.value.id == receiver

    def argument_reason(node):
        if literal(node):
            return None
        if not isinstance(node, ast.Call):
            return f"argument {type(node).__name__} is not allowed"
        if not direct_attribute(node.func, "sa") or node.func.attr not in constructors:
            return "only direct sa constructors are allowed"
        for keyword in node.keywords:
            if keyword.arg is None:
                return "expanded keyword arguments are not allowed"
        for child in node.args + [keyword.value for keyword in node.keywords]:
            reason = argument_reason(child)
            if reason:
                return reason
        return None

    def statement_reason(node):
        if ast.dump(node) == ast.dump(head_read):
            return None
        if not isinstance(node, ast.Expr):
            return f"statement {type(node).__name__} is not allowed"
        call = node.value
        if not isinstance(call, ast.Call) or not direct_attribute(call.func, "op"):
            return "only direct reviewed op calls are allowed"
        operation = call.func.attr
        if operation == "execute":
            # SQL is a separate exact statement allowance, not a DDL argument.
            if len(call.args) != 1 or call.keywords:
                return "unreviewed execute statement"
            sql = call.args[0]
            if isinstance(sql, ast.Call):
                if not (direct_attribute(sql.func, "sa") and sql.func.attr == "text" and len(sql.args) == 1 and not sql.keywords):
                    return "unreviewed execute statement"
                sql = sql.args[0]
            if not isinstance(sql, ast.Constant) or not isinstance(sql.value, str):
                return "unreviewed execute statement"
            if normalize_statement(sql.value) not in permitted["reviewed_statements"]:
                return "unreviewed execute statement"
            return None
        if operation not in permitted["ddl"]:
            return f"unreviewed operation op.{operation}"
        if any(keyword.arg is None for keyword in call.keywords):
            return "expanded keyword arguments are not allowed"
        for argument in call.args + [keyword.value for keyword in call.keywords]:
            reason = argument_reason(argument)
            if reason:
                return reason
        return None

    for index, node in enumerate(module.body):
        if index == 0 and isinstance(node, ast.Expr) and isinstance(node.value, ast.Constant) and isinstance(node.value.value, str):
            continue
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            if ast.dump(node) not in imports:
                violations.append("module: unreviewed import")
        elif isinstance(node, ast.Assign):
            if len(node.targets) != 1 or not isinstance(node.targets[0], ast.Name) or node.targets[0].id not in revision_names:
                violations.append("module: unreviewed module assignment")
            elif not literal(node.value):
                violations.append("module: revision value must be literal")
            elif node.targets[0].id in seen_revisions:
                violations.append("module: duplicate revision assignment")
            else:
                seen_revisions.add(node.targets[0].id)
        elif isinstance(node, ast.FunctionDef):
            if node.name not in {"upgrade", "downgrade"}:
                violations.append(f"module: unreviewed function {node.name}")
            elif ast.dump(node.args) != empty_arguments or node.decorator_list or node.returns is not None or node.type_comment or getattr(node, "type_params", []):
                violations.append("module: function signature is not allowed")
            elif node.name in seen_functions:
                violations.append(f"module: duplicate function {node.name}")
            else:
                seen_functions.add(node.name)
                for statement in node.body:
                    reason = statement_reason(statement)
                    if reason:
                        violations.append(f"{node.name}: {reason}")
        else:
            violations.append(f"module: module statement {type(node).__name__} is not allowed")
    if seen_revisions != revision_names:
        violations.append("module: required revision assignments missing")
    if seen_functions != {"upgrade", "downgrade"}:
        violations.append("module: required migration functions missing")
    return violations


def migration_module(body, *, module_prefix="", function="upgrade"):
    """A complete static migration fixture; snippets are parsed, never executed."""
    source = '''\
from alembic import op
import sqlalchemy as sa
revision = "p89_20261022_watchlist_producer"
down_revision = "reviewed_predecessor"
branch_labels = None
depends_on = None
'''
    source += module_prefix + "\n"
    other = "downgrade" if function == "upgrade" else "upgrade"
    source += f"def {function}():\n" + textwrap.indent(body, "    ") + "\n"
    source += f"def {other}():\n    op.create_table('watchlist_producer_state', sa.Column('id', sa.Integer()))\n"
    return ast.parse(source)


def test_phase8_migration_reviewer_refuses_unreviewed_operations():
    manifest = load_ledger()["schema_change_set"]
    assert "permitted_migration_operations" in manifest, "reviewed migration operations missing from fixture"

    def upgrade_of(source):
        parsed = ast.parse(textwrap.dedent(source))
        body = parsed.body[0].body
        return migration_module("\n".join(ast.unparse(statement) for statement in body))

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
    head_check = '''\
        def upgrade():
            heads = op.get_bind().execute(sa.text("SELECT version_num FROM alembic_version")).scalars().all()
        '''
    assert review_migration_operations(upgrade_of(head_check), manifest) == []
    before_and_after = '''\
        def upgrade():
            heads = op.get_bind().execute(sa.text("SELECT version_num FROM alembic_version")).scalars().all()
            op.execute("UPDATE watchlist_events SET producer = 'legacy-python'")
            heads = op.get_bind().execute(sa.text("SELECT version_num FROM alembic_version")).scalars().all()
        '''
    assert review_migration_operations(upgrade_of(before_and_after), manifest) == []
    foreign_delete = '''\
        def upgrade():
            op.execute(sa.text("DELETE FROM watchlist_events"))
        '''
    assert review_migration_operations(upgrade_of(foreign_delete), manifest) == ["upgrade: unreviewed execute statement"]
    foreign_update = '''\
        def upgrade():
            op.execute("UPDATE watchlist_events SET producer = 'other-producer'")
        '''
    assert review_migration_operations(upgrade_of(foreign_update), manifest)
    not_literal = '''\
        def upgrade():
            op.execute(statement)
        '''
    assert review_migration_operations(upgrade_of(not_literal), manifest) == ["upgrade: unreviewed execute statement"]
    destructive = '''\
        def upgrade():
            op.drop_column("watchlist_events", "producer")
        '''
    assert review_migration_operations(upgrade_of(destructive), manifest) == ["upgrade: unreviewed operation op.drop_column"]


@pytest.mark.parametrize("statement", [
    'sa.text("DELETE FROM watchlist_events")',
    'sa.delete(watchlist_events)',
    'sa.text("UPDATE watchlist_events SET producer = \'legacy-python\'")',
    'statement',
])
@pytest.mark.parametrize("access", ["chained", "assigned"])
def test_phase8_migration_reviewer_refuses_unreviewed_bind_execution(statement, access):
    manifest = load_ledger()["schema_change_set"]
    if access == "chained":
        source = f"def upgrade():\n    op.get_bind().execute({statement})\n"
    else:
        source = f"def upgrade():\n    bind = op.get_bind()\n    bind.execute({statement})\n"
    upgrade = migration_module("\n".join(ast.unparse(node) for node in ast.parse(source).body[0].body))
    assert review_migration_operations(upgrade, manifest), f"unreviewed {access} bind execution accepted"


@pytest.mark.parametrize("operation", [
    "bind = op.get_bind()",
    "consume(op.get_bind())",
    "op.get_context()",
    'op.get_bind().execute(sa.text("SELECT version_num FROM alembic_version"), parameters)',
])
def test_phase8_migration_reviewer_refuses_unreviewed_connection_access(operation):
    upgrade = migration_module(operation)
    assert review_migration_operations(upgrade, load_ledger()["schema_change_set"])


@pytest.mark.parametrize("body", [
    'op.get_bind().execute(sa.text("SELECT version_num FROM alembic_version")).connection.exec_driver_sql("DELETE FROM watchlist_events")',
    'connection = op.get_bind().execute(sa.text("SELECT version_num FROM alembic_version")).connection\nconnection.exec_driver_sql("DELETE FROM watchlist_events")',
    'result = op.get_bind().execute(sa.text("SELECT version_num FROM alembic_version"))\nconnection = result.connection\nconnection.exec_driver_sql("DELETE FROM watchlist_events")',
    'result = op.get_bind().execute(sa.text("SELECT version_num FROM alembic_version"))\nresult.connection.execute(sa.text("DELETE FROM watchlist_events"))',
    'bind = op.get_bind()\nalias = bind\nalias.exec_driver_sql("DELETE FROM watchlist_events")',
    'connection = op.get_context().connection\nconnection.execute(sa.text("DELETE FROM watchlist_events"))',
    'op.get_bind().exec_driver_sql("SELECT version_num FROM alembic_version")',
    'op.get_bind().execute(text("DELETE FROM watchlist_events"))',
    'heads = op.get_bind().execute(sa.text("SELECT version_num FROM alembic_version")).connection',
    'consume(op.get_bind().execute(sa.text("SELECT version_num FROM alembic_version")))',
    'result = op.get_bind().execute(sa.text("SELECT version_num FROM alembic_version"))',
    'heads = op.get_bind().execute(sa.text("SELECT version_num FROM alembic_version")).scalars().all().connection',
])
def test_phase8_migration_reviewer_refuses_head_result_connection_access(body):
    upgrade = migration_module(body)
    assert review_migration_operations(upgrade, load_ledger()["schema_change_set"]), (
        f"unreviewed head-result or connection access accepted: {body}"
    )


@pytest.mark.parametrize("body, reason", [
    ('getattr(op, "get_bind")()', 'only direct reviewed op calls are allowed'),
    ('getattr(bind, "exec_driver_sql")("DELETE FROM watchlist_events")', 'only direct reviewed op calls are allowed'),
    ('getattr(op, "execute")("DELETE FROM watchlist_events")', 'only direct reviewed op calls are allowed'),
    ('get_bind = getattr(op, "get_bind")', 'statement Assign is not allowed'),
    ('run_sql = getattr(bind, "exec_driver_sql")\nrun_sql("DELETE FROM watchlist_events")', 'statement Assign is not allowed'),
    ('getattr(op, "get_context")().connection', 'only direct reviewed op calls are allowed'),
    ('__import__("sqlalchemy")', 'only direct reviewed op calls are allowed'),
    ('import importlib\nimportlib.import_module("sqlalchemy")', 'statement Import is not allowed'),
    ('extra_sql()', 'only direct reviewed op calls are allowed'),
    ('x = 1', 'statement Assign is not allowed'),
    ('x: int = 1', 'statement AnnAssign is not allowed'),
    ('op.execute', 'only direct reviewed op calls are allowed'),
    ('(lambda: 1)()', 'only direct reviewed op calls are allowed'),
    ('[x for x in []]', 'only direct reviewed op calls are allowed'),
    ('with context():\n    pass', 'statement With is not allowed'),
    ('for x in []:\n    pass', 'statement For is not allowed'),
    ('while False:\n    pass', 'statement While is not allowed'),
    ('try:\n    pass\nexcept Exception:\n    pass', 'statement Try is not allowed'),
    ('if True:\n    pass', 'statement If is not allowed'),
    ('exec("pass")', 'only direct reviewed op calls are allowed'),
    ('eval("1")', 'only direct reviewed op calls are allowed'),
    ('def nested():\n    pass', 'statement FunctionDef is not allowed'),
    ('class Nested:\n    pass', 'statement ClassDef is not allowed'),
    ('op.create_table(name)', 'argument Name is not allowed'),
    ('op.create_table(other.name)', 'argument Attribute is not allowed'),
    ('op.create_table(sa.__dict__)', 'argument Attribute is not allowed'),
    ('op.create_table(sa.Column.__call__("x"))', 'only direct sa constructors are allowed'),
    ('op.create_table(getattr(sa, "Column")("x"))', 'only direct sa constructors are allowed'),
    ('op.create_table(sa.__getattribute__("Column"))', 'only direct sa constructors are allowed'),
    ('op.create_table(f"table_{1}")', 'argument JoinedStr is not allowed'),
    ('op.create_table("a" + "b")', 'argument BinOp is not allowed'),
    ('op.create_table("%s" % "a")', 'argument BinOp is not allowed'),
    ('op.create_table("{}".format("a"))', 'only direct sa constructors are allowed'),
    ('op.create_table(*["x"])', 'argument Starred is not allowed'),
    ('op.create_table(**{})', 'expanded keyword arguments are not allowed'),
])
@pytest.mark.parametrize("function", ["upgrade", "downgrade"])
def test_phase8_module_reviewer_refuses_unaccepted_function_syntax(body, reason, function):
    violations = review_migration_operations(migration_module(body, function=function), load_ledger()["schema_change_set"])
    assert any(reason in violation for violation in violations), (reason, violations)


@pytest.mark.parametrize("prefix, reason", [
    ('def extra_sql():\n    op.get_bind().exec_driver_sql("DELETE FROM watchlist_events")', 'unreviewed function extra_sql'),
    ('class Helper:\n    pass', 'module statement ClassDef is not allowed'),
    ('import importlib', 'unreviewed import'),
    ('from sqlalchemy import text', 'unreviewed import'),
    ('import sqlalchemy as other', 'unreviewed import'),
    ('sa = __import__("sqlalchemy")', 'unreviewed module assignment'),
    ('alias = op.execute', 'unreviewed module assignment'),
    ('op.execute("DELETE FROM watchlist_events")', 'module statement Expr is not allowed'),
    ('revision = "a" + "b"', 'revision value must be literal'),
    ('revision: str = "x"', 'module statement AnnAssign is not allowed'),
    ('@decorator\ndef helper():\n    pass', 'unreviewed function helper'),
    ('def upgrade(value=exec("pass")):\n    pass', 'function signature is not allowed'),
    ('@decorator\ndef upgrade():\n    pass', 'function signature is not allowed'),
    ('async def helper():\n    pass', 'module statement AsyncFunctionDef is not allowed'),
])
def test_phase8_module_reviewer_refuses_unaccepted_module_syntax(prefix, reason):
    tree = migration_module('extra_sql()', module_prefix=prefix)
    violations = review_migration_operations(tree, load_ledger()["schema_change_set"])
    assert any(reason in violation for violation in violations), (reason, violations)


def test_phase8_module_reviewer_accepts_static_ddl_and_head_reads():
    tree = migration_module('''heads = op.get_bind().execute(sa.text("SELECT version_num FROM alembic_version")).scalars().all()
op.add_column("watchlist_events", sa.Column("producer", sa.Text(), nullable=False, server_default="legacy-python"))
op.create_table("watchlist_producer_state", sa.Column("id", sa.Integer(), primary_key=True))
op.create_check_constraint("ck_watchlist_events_producer_identity", "watchlist_events", "producer IS NOT NULL")
op.create_index("uq_watchlist_events_producer_source_target_derived", "watchlist_events", ["producer"], unique=True, postgresql_where=sa.text("producer = 'rust-market-v1'"))
op.execute(sa.text("UPDATE watchlist_events SET producer = 'legacy-python'"))''')
    assert review_migration_operations(tree, load_ledger()["schema_change_set"]) == []


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
    assert review_migration_operations(tree, manifest) == []
    upgrade = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "upgrade")
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

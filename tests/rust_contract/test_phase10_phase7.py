"""Phase 7 retirement contract: founder decision, exact ownership and retention.

These tests parse the reviewed fixtures and the Caddy candidate as data. They
never import the application, contact a host or open a database. The separate,
authorized staging and production application of Caddy ownership is not claimed
here; the fixtures must say so until that evidence is recorded.
"""

import copy
import hashlib
import json
import re
from collections import Counter
from datetime import date
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/rust_contract/phase10"
CADDY_SNIPPET = ROOT / "deploy/caddy/phase10-phase7-retire.caddy"
ROUTE_MANIFEST = FIXTURES / "routes.json"
DECISION = FIXTURES / "phase7-decision.json"
PHASE7_ROUTES = FIXTURES / "phase7-routes.json"
ACCEPTANCE_SCRIPT = ROOT / "deploy/tests/phase7-acceptance.sh"

TASK_FILES = {
    "deploy/caddy/phase10-phase7-retire.caddy",
    "deploy/tests/phase7-acceptance.sh",
    "tests/rust_contract/phase10/phase7-decision.json",
    "tests/rust_contract/phase10/phase7-routes.json",
    "tests/rust_contract/test_phase10_phase7.py",
}
RETAINED_TABLES = {
    "negotiation_rounds",
    "negotiations",
    "rfq_quotes",
    "rfqs",
    "supplier_offers",
}
OWNER_COUNTS = {
    "app/routers/rfq.py": 8,
    "app/routers/negotiations.py": 6,
    "app/routers/supplier_offers.py": 6,
}
# Every method a framework or proxy may serve for a retired path. A retired
# operation takes its own method plus HEAD (implicit for a GET route) and
# OPTIONS (CORS preflight); every other method stays on the fallback.
METHODS = ("GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "TRACE")
UUID_CANONICAL = "11111111-1111-4111-8111-111111111111"
# Starlette routes any single non-empty segment to a typed path parameter and
# then answers 422 for a bad id. A UUID-only matcher would therefore fail open
# to Python for braced, URN or percent-encoded ids.
ID_SEGMENT = r"[^/]+"
PLACEHOLDER = re.compile(r"\{[a-z_]+\}")
DESTRUCTIVE_SQL = re.compile(
    r"\b(?:DROP\s+TABLE|TRUNCATE|DELETE\s+FROM|ALTER\s+TABLE|CREATE\s+TABLE)\b", re.IGNORECASE
)


def document(name):
    path = FIXTURES / f"{name}.json"
    assert path.is_file(), f"missing Phase 7 fixture: {name}"
    return json.loads(path.read_text())


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def operation_rows():
    return document("phase7-routes")["operations"]


def source_rows():
    return sorted(document("routes")["artifacts"], key=lambda row: row["id"])


def task_20_rows():
    return [row for row in source_rows() if row.get("task") == 20]


def source_identity_sha():
    fields = ("id", "source", "source_sha256", "method", "path", "line")
    projection = sorted(tuple(row[field] for field in fields) for row in task_20_rows())
    return hashlib.sha256(json.dumps(projection, separators=(",", ":")).encode()).hexdigest()


def canonical(path, uuid=UUID_CANONICAL):
    return PLACEHOLDER.sub(uuid, path)


def parse_caddy(text):
    """Parse the reviewed snippet shape; refuse anything that can proxy or catch all."""
    matchers = {}
    handles = {}
    current = None
    block = None
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        assert "reverse_proxy" not in line, "upstream directive in the retirement snippet"
        if current is None:
            match = re.fullmatch(r"@([A-Za-z0-9_]+)\s*\{", line)
            if match:
                name = match.group(1)
                assert name not in matchers, f"duplicate matcher: {name}"
                matchers[name] = {"methods": [], "path": None, "regex": None, "exclude": []}
                current, block = name, "matcher"
                continue
            match = re.fullmatch(r"handle\s+@([A-Za-z0-9_]+)\s*\{", line)
            if match:
                name = match.group(1)
                assert name not in handles, f"duplicate handler: {name}"
                handles[name] = []
                current, block = name, "handle"
                continue
            assert not re.match(r"handle\s*\{", line), "catch-all handle in the retirement snippet"
            raise AssertionError(f"unowned Caddy directive: {line}")
        if line == "}":
            current = block = None
            continue
        if block == "matcher":
            match = re.fullmatch(r"(not\s+)?(method|path|path_regexp)\s+(.+)", line)
            assert match, f"unowned matcher directive: {line}"
            negated, kind, value = match.groups()
            if negated:
                assert kind == "path", f"only a path may be negated: {current}"
                assert value not in matchers[current]["exclude"], f"duplicate not path: {current}"
                matchers[current]["exclude"].append(value)
                continue
            if kind == "method":
                matchers[current]["methods"].extend(value.split())
            elif kind == "path":
                assert matchers[current]["path"] is None, f"duplicate path: {current}"
                assert matchers[current]["regex"] is None, f"path and regex: {current}"
                matchers[current]["path"] = value
            else:
                assert matchers[current]["regex"] is None, f"duplicate regex: {current}"
                assert matchers[current]["path"] is None, f"path and regex: {current}"
                name, _, expression = value.partition(" ")
                assert name == current, f"regexp name differs from its matcher: {current}"
                assert expression.startswith("^") and expression.endswith("$"), (
                    f"regexp must anchor both ends: {current}"
                )
                matchers[current]["regex"] = expression
        else:
            handles[current].append(line)
    assert current is None, "unclosed Caddy block"
    assert matchers, "the retirement snippet owns no matcher"
    assert set(matchers) == set(handles), "every matcher has exactly one handler"
    for name, matcher in matchers.items():
        assert matcher["methods"], f"matcher without a method: {name}"
        assert (matcher["path"] is None) != (matcher["regex"] is None), (
            f"matcher needs one path or regexp: {name}"
        )
        assert handles[name] == ["respond 404"], f"handler must return 404 only: {name}"
        assert not matcher["exclude"] or matcher["regex"], (
            f"only a regexp matcher may exclude a static sibling: {name}"
        )
    return [dict(name=name, **matcher) for name, matcher in matchers.items()]


def caddy_matchers():
    assert CADDY_SNIPPET.is_file(), "missing deploy/caddy/phase10-phase7-retire.caddy"
    return parse_caddy(CADDY_SNIPPET.read_text())


def matches(matcher, method, path):
    """Match the Caddy view: the query string is not part of the path."""
    if method not in matcher["methods"]:
        return False
    path = path.partition("?")[0]
    if path in matcher["exclude"]:
        return False
    if matcher["path"] is not None:
        return path == matcher["path"]
    return re.fullmatch(matcher["regex"], path) is not None


def check_digest(relative, expected):
    path = ROOT / relative
    assert path.is_file(), f"missing evidence file: {relative}"
    assert digest(path) == expected, f"digest drift: {relative}"


def validate_retention(retention):
    assert sorted(retention["retained_tables"]) == sorted(RETAINED_TABLES)
    for field in ("migration", "table_drop", "data_cleanup", "schema_change"):
        assert retention[field] == "none", f"Path B must not perform {field}"
    change_set = retention["change_set"]
    assert change_set == sorted(change_set), "change set must be sorted"
    assert set(change_set) == TASK_FILES, "Task 20 changes only its five files"
    for relative in change_set:
        path = ROOT / relative
        assert path.is_file(), f"missing change-set file: {relative}"
        assert not relative.startswith(("alembic/", "rs/read/migrations/")), (
            "Path B adds no migration"
        )
        assert path.suffix in {".json", ".py", ".sh", ".caddy"}, relative
    for relative in (
        "deploy/caddy/phase10-phase7-retire.caddy",
        "tests/rust_contract/phase10/phase7-decision.json",
        "tests/rust_contract/phase10/phase7-routes.json",
    ):
        assert not DESTRUCTIVE_SQL.search((ROOT / relative).read_text()), (
            f"destructive SQL text in {relative}"
        )
    capture = retention["live_capture"]
    assert capture["status"] in {"not-run", "accepted"}
    if capture["status"] == "not-run":
        assert capture["authorization_ref"] is None
        assert capture["reason"].strip(), "a skipped live capture needs a reason"
        return
    assert capture["authorization_ref"], "accepted capture needs an authorization reference"
    before, after = capture["before"], capture["after"]
    for block in (before, after):
        assert set(block["row_counts"]) == RETAINED_TABLES
        assert all(count >= 0 for count in block["row_counts"].values())
        assert re.fullmatch(r"[0-9a-f]{64}", block["logical_digest"]), "logical digest is SHA-256"
    assert before["row_counts"] == after["row_counts"], "row counts changed"
    assert before["logical_digest"] == after["logical_digest"], "logical digest changed"


def validate_decision(doc):
    assert doc["schema_version"] == 1
    decision = doc["decision"]
    assert decision["authority"] == "founder"
    assert decision["id"] == "122"
    assert date.fromisoformat(decision["date"]) == date(2026, 10, 22)
    assert decision["statement"] == "Retire"
    paths = doc["paths"]
    assert set(paths) == {"A", "B"}
    selected = [key for key in ("A", "B") if paths[key]["selected"]]
    assert len(selected) == 1, "the decision must select exactly one path"
    assert doc["merge_only_is_acceptance"] is False, "a merged plan is not acceptance"

    path_a = paths["A"]
    assert path_a["name"] == "port"
    if path_a["selected"]:
        assert decision["selected"] == "A"
    if path_a["selected"] or path_a["accepted"]:
        evidence = path_a.get("evidence", {})
        live = evidence.get("live_native_acceptance", {})
        assert live.get("status") == "accepted", "Path A requires live native acceptance"
        assert live.get("authorization_ref"), "Path A live acceptance requires authorization"
        assert evidence.get("schema_producer_closure", {}).get("closed") is True, (
            "Path A requires schema-producer closure"
        )
        assert evidence.get("http_and_rollback_corpus"), (
            "Path A requires the HTTP and rollback corpus"
        )
        assert path_a["merge_only_is_acceptance"] is False

    path_b = paths["B"]
    assert path_b["name"] == "retire"
    if path_b["selected"]:
        assert path_a["selected"] is False and path_a["accepted"] is False
        assert decision["selected"] == "B"
        assert decision["selected_name"] == "retire-routes-preserve-tables"
        retirement = path_b["retirement"]
        assert retirement["acceptance_source"] == "live-http-and-retention", (
            "a merged plan is not live acceptance"
        )
        assert retirement["ownership_state"] == "accepted"
        assert retirement["ownership_scope"].strip()
        assert retirement["operations"] == 20
        assert retirement["status_code"] == 404
        assert retirement["upstream"] is None
        assert retirement["live_state"] in {"not-applied", "applied"}
        assert retirement["caddy_file"] == "deploy/caddy/phase10-phase7-retire.caddy"
        assert retirement["route_manifest"] == "tests/rust_contract/phase10/phase7-routes.json"
        check_digest(retirement["caddy_file"], retirement["caddy_sha256"])
        check_digest(retirement["route_manifest"], retirement["route_manifest_sha256"])
        rollback = path_b["rollback"]
        assert rollback["kind"] == "one-change"
        assert rollback["scope_operations"] == 20
        assert rollback["target"] == "python-upstream"
        assert rollback["rehearsed"] == "fixture"
        validate_retention(path_b["retention"])
        live = path_b["live_acceptance"]
        assert live["required"] is True
        assert live["completion"] in {"blocked-separately-authorized", "accepted"}
        if live["authorized"]:
            assert live["authorization_ref"], "live acceptance requires an authorization reference"
        else:
            assert live["authorization_ref"] is None
        for environment in ("staging", "production"):
            cell = live[environment]
            assert cell["status"] in {"not-run", "accepted"}
            if cell["status"] == "accepted":
                assert live["authorized"] is True, "live acceptance requires authorization"
                assert cell["authorization_ref"], (
                    "live acceptance requires an authorization reference"
                )
                assert re.fullmatch(r"[0-9a-f]{64}", cell["evidence_sha256"] or ""), (
                    "live acceptance requires evidence"
                )
            else:
                assert cell["authorization_ref"] is None
                assert cell["evidence_sha256"] is None
        if live["completion"] == "accepted":
            assert live["authorized"] is True
            assert all(
                live[environment]["status"] == "accepted"
                for environment in ("staging", "production")
            )
        else:
            assert live["authorized"] is False
            assert all(
                live[environment]["status"] == "not-run"
                for environment in ("staging", "production")
            )

    schema = doc["schema_producers"]
    assert schema["task_20_acceptance_requires_closure"] is True
    acceptance = doc["acceptance"]
    assert acceptance["state"] in {"candidate", "accepted"}
    if acceptance["state"] == "accepted":
        assert schema["closed"] is True, "acceptance requires schema-producer closure"
        assert not acceptance["blockers"]
        live = path_b["live_acceptance"]
        assert live["completion"] == "accepted" and live["authorized"] is True
    else:
        assert acceptance["blockers"], "a candidate acceptance must name its blockers"
    return doc


def retired_methods(operation):
    return [operation["method"]] + (["HEAD"] if operation["method"] == "GET" else []) + ["OPTIONS"]


def encoded_variants(operation):
    """Spellings of a dynamic id that Starlette still routes to the same path."""
    for placeholder in PLACEHOLDER.findall(operation["path"]):
        for variant in (
            UUID_CANONICAL,
            UUID_CANONICAL.replace("-", ""),
            "{" + UUID_CANONICAL + "}",
            "urn:uuid:" + UUID_CANONICAL,
            UUID_CANONICAL.upper(),
            "%7B" + UUID_CANONICAL + "%7D",
            "not-a-uuid",
            "00000000-0000-0000-0000-00000000000",
            "gggggggg-gggg-gggg-gggg-gggggggggggg",
        ):
            yield operation["path"].replace(placeholder, variant, 1)


def owned_paths(operation):
    return {canonical(operation["path"]), *encoded_variants(operation)}


def retired_cases():
    """Every method/path the retirement must own, including encoded id spellings."""
    return {
        (method, path)
        for op in operation_rows()
        for method in op["methods"]
        for path in owned_paths(op)
    }


def boundary_cases():
    """Cases that must stay on the fallback: never captured by a retirement matcher."""
    cases = []
    for op in operation_rows():
        url = canonical(op["path"])
        path = op["path"]
        for method in retired_methods(op):
            cases.append((method, url + "/"))
            cases.append((method, url + "/extra"))
        for method in METHODS:
            if method not in retired_methods(op):
                cases.append((method, url))
        if path.endswith("/quote"):
            cases.append((op["method"], canonical(path) + "s"))
    cases.extend(
        (method, canonical(path))
        for path in (
            "/api/rfq2",
            "/api/rfqs",
            "/api/rfq//quote",
            "/api/rfq/{rfq_id}/quotes",
            "/api/rfq/{rfq_id}/accept",
            "/api/rfq/{rfq_id}/quotes/{quote_id}/withdraw/extra",
            "/api/negotiation",
            "/api/negotiations2",
            "/api/negotiations/{negotiation_id}/decline/extra",
            "/api/negotiations//counter",
            "/api/supplier-offer",
            "/api/supplier-offers2",
            "/api/supplier-offers/{offer_id}/withdraw/extra",
            "/api/rfq/",
            "/api/negotiations/",
            "/api/supplier-offers/",
        )
        for method in METHODS
    )
    owned = retired_cases()
    return sorted(case for case in set(cases) if case not in owned)


def test_phase7_decision_names_exactly_one_path():
    doc = validate_decision(document("phase7-decision"))
    assert doc["decision"]["selected"] == "B"
    assert sum(path["selected"] for path in doc["paths"].values()) == 1


def test_phase7_decision_rejects_zero_or_two_selected_paths():
    doc = document("phase7-decision")
    for first, second in ((False, False), (True, True)):
        broken = copy.deepcopy(doc)
        broken["paths"]["A"]["selected"] = first
        broken["paths"]["B"]["selected"] = second
        with pytest.raises(AssertionError, match="exactly one path"):
            validate_decision(broken)


def test_phase7_merge_only_acceptance_is_refused():
    broken = copy.deepcopy(document("phase7-decision"))
    broken["paths"]["B"]["retirement"]["acceptance_source"] = "merged-plan"
    with pytest.raises(AssertionError, match="not live acceptance"):
        validate_decision(broken)


def test_phase7_exact_twenty_operations_have_accepted_ownership():
    doc = validate_decision(document("phase7-decision"))
    operations = operation_rows()
    assert len(operations) == 20, "the retirement must own exactly 20 operations"
    assert len({op["id"] for op in operations}) == 20
    sources = task_20_rows()
    assert {op["id"]: (op["method"], op["path"]) for op in operations} == {
        row["id"]: (row["method"], row["path"]) for row in sources
    }
    carrier = document("phase7-routes")
    assert carrier["source_identity_fields"] == [
        "id",
        "source",
        "source_sha256",
        "method",
        "path",
        "line",
    ]
    assert carrier["source_identity_sha256"] == source_identity_sha()
    assert Counter(op["source"] for op in operations) == OWNER_COUNTS
    for op in operations:
        assert op["status_code"] == 404
        assert op["upstream"] is None
        assert op["owner"] == "deploy/caddy/phase10-phase7-retire.caddy"
        assert op["evidence"] == "phase7_retirement_exact_operations_return_404"
    assert digest(CADDY_SNIPPET) == doc["paths"]["B"]["retirement"]["caddy_sha256"]
    assert digest(PHASE7_ROUTES) == doc["paths"]["B"]["retirement"]["route_manifest_sha256"]
    matchers = {matcher["name"]: matcher for matcher in caddy_matchers()}
    assert len(matchers) == 20
    assert {op["matcher"] for op in operations} == set(matchers)
    assert carrier["boundaries"]["retired_methods"]["rule"].strip()
    for op in operations:
        assert op["methods"] == retired_methods(op), f"retired methods drifted: {op['id']}"
    for op in operations:
        matcher = matchers[op["matcher"]]
        assert matcher["methods"] == op["methods"], f"matcher methods differ: {op['id']}"
        if op["match"]["kind"] == "path":
            assert op["match"]["value"] == op["path"]
            assert matcher["path"] == op["match"]["value"]
            assert matcher["regex"] is None
        else:
            assert matcher["regex"] == op["match"]["value"]
            assert matcher["regex"] == "^" + PLACEHOLDER.sub(ID_SEGMENT, op["path"]) + "$"
            assert matcher["path"] is None


def test_phase7_retirement_exact_operations_return_404():
    operations = operation_rows()
    matchers = caddy_matchers()
    for op in operations:
        for method in op["methods"]:
            for path in owned_paths(op):
                expected = sorted(
                    other["matcher"]
                    for other in operations
                    if method in other["methods"] and path in owned_paths(other)
                )
                owners = sorted(m["name"] for m in matchers if matches(m, method, path))
                assert owners == expected, (
                    f"{method} {path} is not owned by exactly its operations "
                    f"({op['id']}): {owners} != {expected}"
                )
    for op in operations:
        for method in op["methods"]:
            query = canonical(op["path"]) + "?status=OPEN"
            assert any(
                m["name"] == op["matcher"] and matches(m, method, query) for m in matchers
            ), f"a query string escapes retirement for {method}: {op['id']}"


def test_phase7_path_b_exact_matchers_do_not_steal_adjacent_routes():
    matchers = caddy_matchers()
    for method, path in boundary_cases():
        stolen = [matcher["name"] for matcher in matchers if matches(matcher, method, path)]
        assert not stolen, f"{method} {path} is captured by {stolen}"
    for row in source_rows():
        if row.get("task") == 20:
            continue
        url = canonical(row["path"])
        for method in METHODS:
            stolen = [matcher["name"] for matcher in matchers if matches(matcher, method, url)]
            assert not stolen, f"adjacent source route {row['id']} is captured by {stolen}"


def test_phase7_retirement_preserves_table_and_row_identity():
    doc = validate_decision(document("phase7-decision"))
    capture = doc["paths"]["B"]["retention"]["live_capture"]
    if capture["status"] == "not-run":
        assert capture["authorization_ref"] is None
        assert capture["reason"].strip()
        assert "before" not in capture and "after" not in capture, (
            "a skipped capture must not carry invented counts"
        )
    else:
        assert capture["authorization_ref"]
        assert capture["before"] == capture["after"]


def test_phase7_retirement_rejects_changed_row_identity():
    broken = copy.deepcopy(document("phase7-decision"))
    capture = broken["paths"]["B"]["retention"]["live_capture"]
    counts = {table: 1 for table in RETAINED_TABLES}
    capture.update(
        {
            "status": "accepted",
            "authorization_ref": "staging-and-production-acceptance",
            "before": {"row_counts": counts, "logical_digest": "a" * 64},
            "after": {
                "row_counts": dict(counts, rfqs=2),
                "logical_digest": "b" * 64,
            },
        }
    )
    with pytest.raises(AssertionError, match="row counts"):
        validate_decision(broken)


def test_phase7_path_b_preserves_all_tables_and_rows():
    doc = validate_decision(document("phase7-decision"))
    retention = doc["paths"]["B"]["retention"]
    assert sorted(retention["retained_tables"]) == sorted(RETAINED_TABLES)
    for field in ("migration", "table_drop", "data_cleanup", "schema_change"):
        assert retention[field] == "none"
    assert retention["change_set"] == sorted(TASK_FILES)
    if doc["acceptance"]["state"] == "candidate":
        assert retention["live_capture"]["status"] == "not-run"
        assert doc["schema_producers"]["closed"] is False


def test_phase7_path_a_requires_live_native_acceptance_and_schema_closure():
    doc = validate_decision(document("phase7-decision"))
    path_a = doc["paths"]["A"]
    assert path_a["selected"] is False
    assert path_a["accepted"] is False
    assert set(path_a["acceptance_requires"]) == {
        "live-native-acceptance",
        "schema-producer-closure",
        "http-and-rollback-corpus",
    }
    assert path_a["merge_only_is_acceptance"] is False

    selected = copy.deepcopy(doc)
    selected["paths"]["A"]["selected"] = True
    selected["paths"]["B"]["selected"] = False
    selected["decision"]["selected"] = "A"
    with pytest.raises(AssertionError, match="live native acceptance"):
        validate_decision(selected)

    unclosed = copy.deepcopy(selected)
    unclosed["paths"]["A"]["evidence"] = {
        "live_native_acceptance": {
            "status": "accepted",
            "authorization_ref": "separate-phase-7-plan",
        },
        "schema_producer_closure": {"closed": False},
        "http_and_rollback_corpus": ["phase7_http", "phase7_races"],
    }
    with pytest.raises(AssertionError, match="schema-producer closure"):
        validate_decision(unclosed)


def test_phase7_live_retirement_claim_requires_authorization_and_evidence():
    claimed = copy.deepcopy(document("phase7-decision"))
    claimed["paths"]["B"]["live_acceptance"]["staging"]["status"] = "accepted"
    with pytest.raises(AssertionError, match="authorization"):
        validate_decision(claimed)

    authorized = copy.deepcopy(claimed)
    authorized["paths"]["B"]["live_acceptance"]["authorized"] = True
    authorized["paths"]["B"]["live_acceptance"]["authorization_ref"] = "separate-operator-authorization"
    authorized["paths"]["B"]["live_acceptance"]["staging"]["authorization_ref"] = (
        "separate-operator-authorization"
    )
    with pytest.raises(AssertionError, match="requires evidence"):
        validate_decision(authorized)


def test_phase7_snippet_rejects_a_catch_all_duplicate_or_upstream():
    text = CADDY_SNIPPET.read_text()
    with pytest.raises(AssertionError, match="catch-all"):
        parse_caddy(text + "\nhandle {\n\trespond 404\n}\n")
    with pytest.raises(AssertionError, match="upstream"):
        parse_caddy(text.replace("respond 404", "reverse_proxy 127.0.0.1:9", 1))
    with pytest.raises(AssertionError, match="duplicate matcher"):
        parse_caddy(text + "\n@phase7_rfq_create {\n\tmethod GET\n\tpath /x\n}\n")
    with pytest.raises(AssertionError, match="404 only"):
        parse_caddy(text.replace("\trespond 404\n", "\trespond 200\n", 1))


def test_phase7_acceptance_script_is_fixture_bounded():
    assert ACCEPTANCE_SCRIPT.is_file(), "missing deploy/tests/phase7-acceptance.sh"
    text = ACCEPTANCE_SCRIPT.read_text()
    assert text.startswith("#!/usr/bin/env bash\n")
    assert "set -euo pipefail" in text
    assert "--fixture" in text
    assert not re.search(r"https?://(?:api|www)\.verdaxis", text), (
        "the fixture script names no live Verdaxis host"
    )

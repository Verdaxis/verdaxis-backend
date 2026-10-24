#!/usr/bin/env bash
# Phase 10 Task 20: fixture acceptance for the Path B retirement of the 20
# Phase 7 indicative operations. This script contacts no live host. Applying
# Caddy ownership on staging or production is a separate, authorized step.
#
# Usage: bash deploy/tests/phase7-acceptance.sh --fixture
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
MODE=""

usage() {
    printf '%s\n' "usage: bash deploy/tests/phase7-acceptance.sh --fixture" >&2
}

for arg in "$@"; do
    case "$arg" in
        --fixture)
            MODE=fixture
            ;;
        --live | --live=*)
            printf '%s\n' \
                "phase7-acceptance: live acceptance needs separate authorization" >&2
            exit 2
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            usage
            exit 2
            ;;
    esac
done

if [[ "$MODE" != "fixture" ]]; then
    usage
    exit 2
fi

TEST_HOME=${VERDAXIS_TEST_HOME:-/home/verdaxis-test}
PYTHON=${PHASE7_PYTHON:-}
if [[ -z "$PYTHON" ]]; then
    for candidate in \
        "$TEST_HOME/venv312/bin/python" \
        "$TEST_HOME/venv/bin/python" \
        "$ROOT/venv312/bin/python" \
        "$ROOT/venv/bin/python"; do
        if [[ -x "$candidate" ]]; then
            PYTHON=$candidate
            break
        fi
    done
fi
if [[ -z "$PYTHON" ]]; then
    PYTHON=$(command -v python3 || true)
fi
if [[ -z "$PYTHON" ]]; then
    printf '%s\n' "phase7-acceptance: no Python interpreter found" >&2
    exit 3
fi

printf '%s\n' "phase7-acceptance: python=$PYTHON"
"$PYTHON" - "$ROOT" <<'PY'
import hashlib
import json
import re
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(sys.argv[1])
FIXTURES = ROOT / "tests/rust_contract/phase10"
CADDY = ROOT / "deploy/caddy/phase10-phase7-retire.caddy"
ROUTE_MANIFEST = FIXTURES / "routes.json"
DECISION = FIXTURES / "phase7-decision.json"
PHASE7_ROUTES = FIXTURES / "phase7-routes.json"
SCRIPT = ROOT / "deploy/tests/phase7-acceptance.sh"

RETAINED_TABLES = {
    "negotiation_rounds",
    "negotiations",
    "rfq_quotes",
    "rfqs",
    "supplier_offers",
}
TASK_FILES = {
    "deploy/caddy/phase10-phase7-retire.caddy",
    "deploy/tests/phase7-acceptance.sh",
    "tests/rust_contract/phase10/phase7-decision.json",
    "tests/rust_contract/phase10/phase7-routes.json",
    "tests/rust_contract/test_phase10_phase7.py",
}
OWNER_COUNTS = Counter(
    {
        "app/routers/rfq.py": 8,
        "app/routers/negotiations.py": 6,
        "app/routers/supplier_offers.py": 6,
    }
)
# Every method a framework or proxy may serve for a retired path.
METHODS = ("GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "TRACE")
# Starlette routes any single non-empty segment to a typed path parameter and
# then answers 422 for a bad id, so a UUID-only matcher would leak braced, URN
# and percent-encoded spellings back to Python.
UUID = "11111111-1111-4111-8111-111111111111"
ID_SEGMENT = "[^/]+"
PLACEHOLDER = re.compile(r"\{[a-z_]+\}")
DESTRUCTIVE = re.compile(
    r"\b(?:DROP\s+TABLE|TRUNCATE|DELETE\s+FROM|ALTER\s+TABLE|CREATE\s+TABLE)\b", re.I
)


def fail(message):
    raise SystemExit(f"phase7-acceptance: FAIL: {message}")


def load(path):
    if not path.is_file():
        fail(f"missing fixture: {path.relative_to(ROOT)}")
    return json.loads(path.read_text())


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def canonical(path, uuid=UUID):
    return PLACEHOLDER.sub(uuid, path)


def parse_caddy(text):
    matchers = {}
    handles = {}
    current = None
    block = None
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if "reverse_proxy" in line:
            fail("the retirement snippet contains an upstream")
        if current is None:
            match = re.fullmatch(r"@([A-Za-z0-9_]+)\s*\{", line)
            if match:
                name = match.group(1)
                if name in matchers:
                    fail(f"duplicate matcher: {name}")
                matchers[name] = {"methods": [], "path": None, "regex": None, "exclude": []}
                current, block = name, "matcher"
                continue
            match = re.fullmatch(r"handle\s+@([A-Za-z0-9_]+)\s*\{", line)
            if match:
                name = match.group(1)
                if name in handles:
                    fail(f"duplicate handler: {name}")
                handles[name] = []
                current, block = name, "handle"
                continue
            if re.match(r"handle\s*\{", line):
                fail("the retirement snippet contains a catch-all handle")
            fail(f"unowned Caddy directive: {line}")
        if line == "}":
            current = block = None
            continue
        if block == "matcher":
            match = re.fullmatch(r"(not\s+)?(method|path|path_regexp)\s+(.+)", line)
            if not match:
                fail(f"unowned matcher directive: {line}")
            negated, kind, value = match.groups()
            if negated:
                if kind != "path":
                    fail(f"only a path may be negated: {current}")
                if value in matchers[current]["exclude"]:
                    fail(f"duplicate not path: {current}")
                matchers[current]["exclude"].append(value)
                continue
            if kind == "method":
                matchers[current]["methods"].extend(value.split())
            elif kind == "path":
                if matchers[current]["path"] is not None or matchers[current]["regex"] is not None:
                    fail(f"duplicate path matcher: {current}")
                matchers[current]["path"] = value
            else:
                if matchers[current]["regex"] is not None or matchers[current]["path"] is not None:
                    fail(f"duplicate regexp matcher: {current}")
                name, _, expression = value.partition(" ")
                if name != current:
                    fail(f"regexp name differs from its matcher: {current}")
                if not (expression.startswith("^") and expression.endswith("$")):
                    fail(f"regexp must anchor both ends: {current}")
                matchers[current]["regex"] = expression
        else:
            handles[current].append(line)
    if current is not None:
        fail("unclosed Caddy block")
    if not matchers:
        fail("the retirement snippet owns no matcher")
    if set(matchers) != set(handles):
        fail("every matcher needs exactly one handler")
    for name, matcher in matchers.items():
        if not matcher["methods"]:
            fail(f"matcher without a method: {name}")
        if (matcher["path"] is None) == (matcher["regex"] is None):
            fail(f"matcher needs one path or regexp: {name}")
        if handles[name] != ["respond 404"]:
            fail(f"handler must return 404 only: {name}")
        if matcher["exclude"] and matcher["regex"] is None:
            fail(f"only a regexp matcher may exclude a static sibling: {name}")
    return [dict(name=name, **matcher) for name, matcher in matchers.items()]


def matches(matcher, method, path):
    if method not in matcher["methods"]:
        return False
    path = path.partition("?")[0]
    if path in matcher["exclude"]:
        return False
    if matcher["path"] is not None:
        return path == matcher["path"]
    return re.fullmatch(matcher["regex"], path) is not None


def owned_paths(operation):
    return {canonical(operation["path"]), *encoded_variants(operation)}


def encoded_variants(operation):
    """Spellings of a dynamic id that Starlette still routes to the same path."""
    for placeholder in PLACEHOLDER.findall(operation["path"]):
        for variant in (
            UUID,
            UUID.replace("-", ""),
            "{" + UUID + "}",
            "urn:uuid:" + UUID,
            UUID.upper(),
            "%7B" + UUID + "%7D",
            "not-a-uuid",
            "00000000-0000-0000-0000-00000000000",
            "gggggggg-gggg-gggg-gggg-gggggggggggg",
        ):
            yield operation["path"].replace(placeholder, variant, 1)


def main():
    decision = load(DECISION)
    routes = load(PHASE7_ROUTES)
    source = load(ROUTE_MANIFEST)

    if decision.get("schema_version") != 1:
        fail("decision schema_version is not 1")
    founder = decision["decision"]
    if founder["authority"] != "founder" or founder["id"] != "122":
        fail("the decision is not founder decision 122")
    if founder["date"] != "2026-10-22" or founder["statement"] != "Retire":
        fail("the decision date or statement is wrong")
    paths = decision["paths"]
    selected = [key for key in ("A", "B") if paths[key]["selected"]]
    if len(selected) != 1:
        fail("the decision must select exactly one path")
    if decision.get("merge_only_is_acceptance") is not False:
        fail("a merged plan is not acceptance")
    path_a = paths["A"]
    if path_a["selected"] or path_a["accepted"]:
        evidence = path_a.get("evidence", {})
        live = evidence.get("live_native_acceptance", {})
        if live.get("status") != "accepted" or not live.get("authorization_ref"):
            fail("Path A requires authorized live native acceptance")
        if evidence.get("schema_producer_closure", {}).get("closed") is not True:
            fail("Path A requires schema-producer closure")
        if not evidence.get("http_and_rollback_corpus"):
            fail("Path A requires the HTTP and rollback corpus")
    if selected != ["B"]:
        fail("founder decision 122 selects Path B")
    path_b = paths["B"]
    retirement = path_b["retirement"]
    if retirement["acceptance_source"] != "live-http-and-retention":
        fail("a merged plan is not live acceptance")
    if retirement["ownership_state"] != "accepted":
        fail("the exact ownership record is not accepted")
    if retirement["operations"] != 20 or retirement["status_code"] != 404:
        fail("the retirement must own 20 operations with 404")
    if retirement["upstream"] is not None:
        fail("a retired operation must not name an upstream")
    if not CADDY.is_file():
        fail("missing deploy/caddy/phase10-phase7-retire.caddy")
    if sha(CADDY) != retirement["caddy_sha256"]:
        fail("Caddy snippet digest drift")
    if sha(PHASE7_ROUTES) != retirement["route_manifest_sha256"]:
        fail("route fixture digest drift")

    operations = routes["operations"]
    if len(operations) != 20 or len({op["id"] for op in operations}) != 20:
        fail("the route fixture must contain 20 unique operations")
    task_rows = [row for row in source["artifacts"] if row.get("task") == 20]
    if len(task_rows) != 20:
        fail("the source manifest does not assign 20 operations to Task 20")
    expected = {row["id"]: (row["method"], row["path"]) for row in task_rows}
    actual = {op["id"]: (op["method"], op["path"]) for op in operations}
    if actual != expected:
        fail("the route fixture does not match the pinned source manifest")
    fields = ("id", "source", "source_sha256", "method", "path", "line")
    projection = sorted(tuple(row[field] for field in fields) for row in task_rows)
    identity = hashlib.sha256(json.dumps(projection, separators=(",", ":")).encode()).hexdigest()
    if routes.get("source_identity_fields") != list(fields):
        fail("the route fixture does not name its stable source identity fields")
    if routes.get("source_identity_sha256") != identity:
        fail("the Task 20 source identity changed")
    if Counter(op["source"] for op in operations) != OWNER_COUNTS:
        fail("the operation split is not 8 RFQ, 6 negotiations and 6 offers")
    for op in operations:
        if op["status_code"] != 404 or op["upstream"] is not None:
            fail(f"operation is not a 404 without an upstream: {op['id']}")
        if op["owner"] != "deploy/caddy/phase10-phase7-retire.caddy":
            fail(f"operation owner is wrong: {op['id']}")

    matchers = {matcher["name"]: matcher for matcher in parse_caddy(CADDY.read_text())}
    if len(matchers) != 20:
        fail("the Caddy snippet must own exactly 20 matchers")
    if {op["matcher"] for op in operations} != set(matchers):
        fail("the Caddy matcher set does not match the route fixture")
    for op in operations:
        retired = [op["method"]] + (["HEAD"] if op["method"] == "GET" else []) + ["OPTIONS"]
        if op.get("methods") != retired:
            fail(f"retired method set drifted: {op['id']}")
        matcher = matchers[op["matcher"]]
        if matcher["methods"] != retired:
            fail(f"matcher method differs: {op['matcher']}")
        if op["match"]["kind"] == "path":
            if matcher["path"] != op["path"] or matcher["regex"] is not None:
                fail(f"static matcher differs: {op['matcher']}")
        else:
            pattern = "^" + PLACEHOLDER.sub(ID_SEGMENT, op["path"]) + "$"
            if matcher["regex"] != pattern or matcher["path"] is not None:
                fail(f"regexp matcher differs: {op['matcher']}")
        url = canonical(op["path"])
        for path in owned_paths(op):
            for method in retired:
                expected = sorted(
                    other["matcher"]
                    for other in operations
                    if method in other["methods"] and path in owned_paths(other)
                )
                owners = sorted(
                    name for name, candidate in matchers.items() if matches(candidate, method, path)
                )
                if owners != expected:
                    fail(
                        f"{method} {path} is not owned by exactly its operations "
                        f"({op['id']}): {owners} != {expected}"
                    )
        for method in retired:
            if not matches(matcher, method, url + "?status=OPEN"):
                fail(f"a query string escapes retirement: {op['id']}")
            if matches(matcher, method, url + "/"):
                fail(f"a trailing slash is captured: {op['id']}")
        for other in METHODS:
            if other not in retired and matches(matcher, other, url):
                fail(f"a wrong method is captured: {op['id']}")

    owned = {
        (method, path)
        for candidate in operations
        for method in candidate["methods"]
        for path in owned_paths(candidate)
    }
    for row in source["artifacts"]:
        if row.get("task") == 20:
            continue
        url = canonical(row["path"])
        for method in METHODS:
            if (method, url) in owned:
                continue
            for name, matcher in matchers.items():
                if matches(matcher, method, url):
                    fail(f"adjacent route {row['id']} is captured by {name}")
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
    ):
        url = canonical(path)
        for method in METHODS:
            if (method, url) in owned:
                continue
            for name, matcher in matchers.items():
                if matches(matcher, method, url):
                    fail(f"adjacent path {method} {url} is captured by {name}")

    retention = path_b["retention"]
    if sorted(retention["retained_tables"]) != sorted(RETAINED_TABLES):
        fail("the retained table set changed")
    for field in ("migration", "table_drop", "data_cleanup", "schema_change"):
        if retention[field] != "none":
            fail(f"Path B must not perform {field}")
    if retention["change_set"] != sorted(TASK_FILES):
        fail("Task 20 changes only its five files")
    for relative in retention["change_set"]:
        if not (ROOT / relative).is_file():
            fail(f"missing change-set file: {relative}")
        if relative.startswith(("alembic/", "rs/read/migrations/")):
            fail("Path B adds no migration")
    for relative in (
        "deploy/caddy/phase10-phase7-retire.caddy",
        "tests/rust_contract/phase10/phase7-decision.json",
        "tests/rust_contract/phase10/phase7-routes.json",
    ):
        if DESTRUCTIVE.search((ROOT / relative).read_text()):
            fail(f"destructive SQL text in {relative}")
    capture = retention["live_capture"]
    if capture["status"] not in {"not-run", "accepted"}:
        fail("the retention capture status is unknown")
    if capture["status"] == "not-run":
        if capture["authorization_ref"] is not None or not capture["reason"].strip():
            fail("a skipped capture needs a reason and no authorization")
    else:
        if not capture["authorization_ref"]:
            fail("an accepted capture needs an authorization reference")
        before, after = capture["before"], capture["after"]
        for block in (before, after):
            if set(block["row_counts"]) != RETAINED_TABLES:
                fail("a capture block does not cover every retained table")
            if not re.fullmatch(r"[0-9a-f]{64}", block["logical_digest"]):
                fail("a capture logical digest is not SHA-256")
        if before["row_counts"] != after["row_counts"]:
            fail("retained row counts changed")
        if before["logical_digest"] != after["logical_digest"]:
            fail("the retained logical digest changed")

    rollback = path_b["rollback"]
    if rollback["kind"] != "one-change" or rollback["scope_operations"] != 20:
        fail("rollback is not one change for all 20 operations")
    if rollback["target"] != "python-upstream" or rollback["rehearsed"] != "fixture":
        fail("rollback target or rehearsal is wrong")

    live = path_b["live_acceptance"]
    if live["required"] is not True:
        fail("live acceptance is not required")
    if live["completion"] not in {"blocked-separately-authorized", "accepted"}:
        fail("live completion state is unknown")
    if live["authorized"]:
        if not live["authorization_ref"]:
            fail("live acceptance needs an authorization reference")
    elif live["authorization_ref"] is not None:
        fail("an unauthorized live acceptance names an authorization")
    for environment in ("staging", "production"):
        cell = live[environment]
        if cell["status"] not in {"not-run", "accepted"}:
            fail(f"{environment} live status is unknown")
        if cell["status"] == "accepted":
            if not live["authorized"] or not cell["authorization_ref"]:
                fail(f"{environment} live acceptance is not authorized")
            if not re.fullmatch(r"[0-9a-f]{64}", cell["evidence_sha256"] or ""):
                fail(f"{environment} live acceptance has no evidence")
        else:
            if cell["authorization_ref"] is not None or cell["evidence_sha256"] is not None:
                fail(f"{environment} live acceptance is not run but claims evidence")
    if live["completion"] == "accepted":
        if not live["authorized"] or any(
            live[environment]["status"] != "accepted" for environment in ("staging", "production")
        ):
            fail("an accepted completion needs authorized staging and production evidence")
    else:
        if live["authorized"] or any(
            live[environment]["status"] != "not-run" for environment in ("staging", "production")
        ):
            fail("a blocked completion must not claim live acceptance")

    schema = decision["schema_producers"]
    if schema["task_20_acceptance_requires_closure"] is not True:
        fail("Task 20 acceptance must require schema-producer closure")
    acceptance = decision["acceptance"]
    if acceptance["state"] not in {"candidate", "accepted"}:
        fail("the acceptance state is unknown")
    if acceptance["state"] == "accepted":
        if not schema["closed"] or acceptance["blockers"]:
            fail("an accepted state needs closed producers and no blockers")
        if live["completion"] != "accepted" or not live["authorized"]:
            fail("an accepted state needs authorized live acceptance")
    elif not acceptance["blockers"]:
        fail("a candidate acceptance must name its blockers")

    if not SCRIPT.is_file():
        fail("missing deploy/tests/phase7-acceptance.sh")
    script_text = SCRIPT.read_text()
    if "--fixture" not in script_text or "set -euo pipefail" not in script_text:
        fail("the acceptance script is not fixture-bounded")
    if re.search(r"https?://(?:api|www)\.verdaxis", script_text):
        fail("the fixture script names a live Verdaxis host")

    print("phase7-acceptance: 20 operations, exact matchers, retention and rollback record pass")


main()
PY

CADDY_BIN=${CADDY_BIN:-}
if [[ -z "$CADDY_BIN" ]]; then
    CADDY_BIN=$(command -v caddy || true)
fi
if [[ -z "$CADDY_BIN" || ! -x "$CADDY_BIN" ]]; then
    printf '%s\n' "phase7-acceptance: Caddy is required for the disposable HTTP corpus" >&2
    exit 3
fi
CADDY_VERSION=$("$CADDY_BIN" version)
printf '%s\n' "phase7-acceptance: caddy=$CADDY_VERSION"
"$PYTHON" - "$ROOT" "$CADDY_BIN" <<'PY'
import http.client
import http.server
import json
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(sys.argv[1])
CADDY = sys.argv[2]
FIXTURES = ROOT / "tests/rust_contract/phase10"
SNIPPET = ROOT / "deploy/caddy/phase10-phase7-retire.caddy"
UUID = "11111111-1111-4111-8111-111111111111"
PLACEHOLDER = __import__("re").compile(r"\{[a-z_]+\}")


def fail(message):
    raise SystemExit(f"phase7-acceptance: FAIL: {message}")


def free_port():
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        return reservation.getsockname()[1]


class Upstream(http.server.BaseHTTPRequestHandler):
    seen = []

    def respond(self):
        Upstream.seen.append((self.command, self.path))
        body = b'{"upstream":"python"}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("X-Route-Owner", "python")
        self.end_headers()
        self.wfile.write(body)

    do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = respond

    def do_HEAD(self):
        self.seen.append((self.command, self.path))
        self.send_response(200)
        self.send_header("X-Route-Owner", "python")
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_OPTIONS(self):
        self.seen.append((self.command, self.path))
        self.send_response(200)
        self.send_header("X-Route-Owner", "python")
        self.send_header("Content-Length", "0")
        self.end_headers()

    def log_message(self, *_args):
        pass


def request(port, method, path):
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
    try:
        connection.request(method, path)
        response = connection.getresponse()
        body = response.read()
        return response.status, dict(response.getheaders()), body
    finally:
        connection.close()


def start_caddy(directory, name, template, replacements):
    log_path = directory / f"{name}.log"
    for _attempt in range(3):
        port = free_port()
        config_path = directory / f"{name}.caddy"
        config_path.write_text(template.format(port=port, **replacements))
        validation = subprocess.run(
            [CADDY, "validate", "--config", str(config_path)],
            capture_output=True,
            text=True,
        )
        if validation.returncode != 0:
            fail(f"Caddy rejected {name}: {validation.stdout}{validation.stderr}")
        log = log_path.open("w")
        process = subprocess.Popen([CADDY, "run", "--config", str(config_path)], stdout=log, stderr=log)
        for _ in range(100):
            if process.poll() is not None:
                break
            try:
                status, _headers, _body = request(port, "GET", "/health/live")
            except OSError:
                time.sleep(0.05)
                continue
            if status == 200:
                return process, port, log
        process.terminate()
        process.wait(timeout=10)
        log.close()
    fail(f"disposable Caddy did not start for {name}: {log_path.read_text()}")


def stop(process, log):
    process.terminate()
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
    log.close()


def operations():
    return json.loads((FIXTURES / "phase7-routes.json").read_text())["operations"]


def canonical(path, uuid=UUID):
    return PLACEHOLDER.sub(uuid, path)


def encoded_variants(operation):
    for placeholder in PLACEHOLDER.findall(operation["path"]):
        for variant in (
            UUID,
            UUID.replace("-", ""),
            "{" + UUID + "}",
            "urn:uuid:" + UUID,
            UUID.upper(),
            "%7B" + UUID + "%7D",
        ):
            yield operation["path"].replace(placeholder, variant, 1)


def owned_paths(operation):
    return {canonical(operation["path"]), *encoded_variants(operation)}


def main():
    upstream = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Upstream)
    thread = threading.Thread(target=upstream.serve_forever, daemon=True)
    thread.start()
    upstream_url = f"127.0.0.1:{upstream.server_port}"
    template = """{{
    admin off
    auto_https off
}}
http://127.0.0.1:{port} {{
    route {{
{import}
        handle {{
            reverse_proxy {upstream}
        }}
    }}
}}
"""
    live_import = f"        import {SNIPPET}"
    operations = operations()
    port = None
    with tempfile.TemporaryDirectory(prefix="phase7-acceptance-") as temporary:
        directory = Path(temporary)
        process = None
        log = None
        try:
            process, port, log = start_caddy(
                directory, "live", template, {"import": live_import, "upstream": upstream_url}
            )
            for op in operations:
                methods = [op["method"]] + (["HEAD"] if op["method"] == "GET" else []) + ["OPTIONS"]
                for path in owned_paths(op):
                    for method in methods:
                        Upstream.seen.clear()
                        status, headers, body = request(port, method, path)
                        if status != 404:
                            fail(f"{method} {path} returned {status}, not 404")
                        if body:
                            fail(f"{method} {path} returned a body")
                        if "X-Route-Owner" in headers or Upstream.seen:
                            fail(f"{method} {path} reached an upstream")
            owned = {
                (method, path)
                for candidate in operations
                for method in (
                    [candidate["method"]]
                    + (["HEAD"] if candidate["method"] == "GET" else [])
                    + ["OPTIONS"]
                )
                for path in owned_paths(candidate)
            }
            checks = []
            for op in operations:
                url = canonical(op["path"])
                retired = [op["method"]] + (["HEAD"] if op["method"] == "GET" else []) + ["OPTIONS"]
                for method in retired:
                    for suffix in ("/", "/extra"):
                        if (method, url + suffix) not in owned:
                            checks.append((method, url + suffix))
                wrong = "PATCH" if op["method"] != "PATCH" else "DELETE"
                if (wrong, url) not in owned:
                    checks.append((wrong, url))
            checks.extend(
                (method, canonical(path))
                for path in (
                    "/api/rfq2",
                    "/api/rfqs",
                    "/api/negotiation",
                    "/api/negotiations2",
                    "/api/supplier-offer",
                )
                for method in ("GET", "POST")
            )
            checks = [case for case in checks if case not in owned]
            for method, path in checks:
                Upstream.seen.clear()
                status, headers, _body = request(port, method, path)
                if status != 200:
                    fail(f"adjacent {method} {path} returned {status}, not 200")
                if headers.get("X-Route-Owner") != "python" or not Upstream.seen:
                    fail(f"adjacent {method} {path} did not reach the fallback")
            for method, path in (
                ("GET", "/api/rfq?status=OPEN"),
                ("GET", "/api/supplier-offers/" + UUID.replace("-", "")),
            ):
                Upstream.seen.clear()
                status, _headers, _body = request(port, method, path)
                if status != 404 or Upstream.seen:
                    fail(f"{method} {path} escaped the retirement")
            stop(process, log)
            process = log = None

            process, port, log = start_caddy(
                directory, "rollback", template, {"import": "", "upstream": upstream_url}
            )
            for op in operations:
                url = canonical(op["path"])
                retired = [op["method"]] + (["HEAD"] if op["method"] == "GET" else []) + ["OPTIONS"]
                for method in retired:
                    Upstream.seen.clear()
                    status, headers, _body = request(port, method, url)
                    if status != 200 or headers.get("X-Route-Owner") != "python":
                        fail(f"rollback did not restore {method} {url}")
                    if not Upstream.seen:
                        fail(f"rollback did not reach Python for {method} {url}")
            print("phase7-acceptance: disposable Caddy 404 corpus and one-change rollback pass")
        finally:
            if process is not None and log is not None:
                stop(process, log)
            upstream.shutdown()
            upstream.server_close()
            thread.join()


main()
PY

printf '%s\n' "phase7-acceptance: fixture checks passed"

"""The market write switch: ledger, Caddy snippets and the remaining Python writers.

`tests/rust_contract/phase89/switch-routes.json` lists the 43 market
operations the switch moves to Rust at once. These checks are static: they
read the Caddy snippets, the Rust route components and the Python routers,
and need no Caddy binary, database or network. The running-Caddy check of the
same snippets belongs to the switch pack.
"""

from __future__ import annotations

import ast
import hashlib
import json
import re
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LEDGER = json.loads((ROOT / "tests/rust_contract/phase89/switch-routes.json").read_text())
OPERATIONS = LEDGER["operations"]
MARKET_SNIPPET = ROOT / "deploy/caddy/rust-market-write.caddy"
PHASE7_SNIPPET = ROOT / "deploy/caddy/phase10-phase7-retire.caddy"
PHASE7_LEDGER = ROOT / "tests/rust_contract/phase10/phase7-routes.json"
RELEASED_SNIPPETS = (
    "rust-auth.caddy",
    "rust-abilities.caddy",
    "rust-offboarding.caddy",
    "rust-crud.caddy",
    "rust-phase4.caddy",
    "rust-phase5.caddy",
    "rust-public-staging.caddy",
)
# The reviewed Phase 10 Task 20 bytes (commit f13e59cc0), copied unchanged.
PHASE7_SHA256 = {
    PHASE7_SNIPPET: "4c08d6433a64519a68c07d6c96687bf28288ea260a4b76aa7b6fb5b6e563b025",
    PHASE7_LEDGER: "45c644cb3a43d38d7a0f78d2c0e6160db9e13a823ac7530da134f2b76dbfb62a",
}
SAMPLE_ID = "00000000-0000-4000-8000-000000000001"
METHODS = ("GET", "HEAD", "OPTIONS", "POST", "PUT", "PATCH", "DELETE")
RUST_COMPONENT_DIR = ROOT / "rs/read/src/routes/market_write"
PYTHON_MARKET_ROUTERS = {
    "app/routers/orderbook.py": "/api/orderbook",
    "app/routers/trades.py": "/api/trades",
    "app/routers/inventory.py": "/api",
    "app/routers/orders.py": "/api/orders",
    "app/routers/matchmaking.py": "/api/matchmaking",
    "app/routers/market_support.py": "/api/admin/market-support",
}
# Python reads in those routers that are not market operations of the switch.
# They keep serving from Python (or from earlier Rust read groups).
PYTHON_READS_OUTSIDE_THE_SWITCH = {
    ("GET", "/api/orderbook/snapshot"),
    ("GET", "/api/orderbook/bids"),
    ("GET", "/api/orderbook/asks"),
    ("GET", "/api/orderbook/with-ci"),
    ("GET", "/api/orderbook/aggregated"),
    ("GET", "/api/orderbook/product-counts"),
    ("GET", "/api/orderbook/map-summary"),
    ("GET", "/api/orderbook/map-summary/compact"),
    ("GET", "/api/orderbook/products"),
    ("GET", "/api/orderbook/regions"),
    ("GET", "/api/orderbook/fuel-types"),
    ("GET", "/api/orderbook"),
    ("GET", "/api/listings"),
}


def concrete(pattern: str) -> str:
    return re.sub(r"\{[^}]+\}", SAMPLE_ID, pattern)


def pattern_regexp(pattern: str) -> str:
    return re.sub(r"\\\{[^}]+\\\}", "[^/]+", re.escape(pattern))


def ledger_paths(operation: dict) -> list[str]:
    return [operation["pattern"], *operation.get("also_paths", [])]


@dataclass
class Matcher:
    name: str
    methods: set[str]
    patterns: list[re.Pattern]
    excluded: list[str]
    gated: bool

    def matches(self, method: str, path: str) -> bool:
        return (
            method in self.methods
            and any(pattern.fullmatch(path) for pattern in self.patterns)
            and path not in self.excluded
        )


def _glob(path: str) -> re.Pattern:
    return re.compile("".join(".*" if part == "*" else re.escape(part) for part in re.split(r"(\*)", path)))


def matchers(source: str) -> list[Matcher]:
    """Named matchers that select an owner by method and path.

    A matcher with no method line (the shared pre-authentication gate and its
    helpers) owns no route and is skipped.
    """
    found = []
    for name, body in re.findall(r"@([A-Za-z0-9_]+) \{\n(.*?)\n\s*\}", source, flags=re.DOTALL):
        methods, patterns, excluded, gated = set(), [], [], False
        for line in body.splitlines():
            words = line.split()
            if not words:
                continue
            if words[0] == "method":
                methods.update(words[1:])
            elif words[0] == "path":
                patterns.extend(_glob(path) for path in words[1:])
            elif words[0] == "path_regexp":
                patterns.append(re.compile(words[-1]))
            elif words[:2] == ["not", "path"]:
                excluded.extend(words[2:])
            elif words[0] == "expression":
                gated = "'{args[0]}' != '{args[1]}'" in line
        if methods:
            found.append(Matcher(name, methods, patterns, excluded, gated))
    return found


def owned(snippet: list[Matcher], method: str, path: str) -> list[str]:
    return [matcher.name for matcher in snippet if matcher.matches(method, path)]


def test_switch_snippet_matches_the_route_ledger():
    source = MARKET_SNIPPET.read_text()
    code = "\n".join(line for line in source.splitlines() if not line.lstrip().startswith("#"))
    snippet = matchers(source)
    assert snippet and all(matcher.gated for matcher in snippet)
    assert "lb_try" not in code and "lb_policy" not in code and "handle {" not in code
    assert code.count("reverse_proxy {args[0]}") == len(snippet)
    assert "{args[1]}" not in code.replace("'{args[0]}' != '{args[1]}'", "")
    assert len(OPERATIONS) == LEDGER["count"] == 43
    assert len({(row["method"], row["pattern"]) for row in OPERATIONS}) == 43
    for operation in OPERATIONS:
        for path in ledger_paths(operation):
            sample = concrete(path)
            assert len(owned(snippet, operation["method"], sample)) == 1, (operation["id"], sample)
            # Another method on the same path belongs to Rust only when a
            # ledger operation of that method takes this path (an identifier
            # matches any segment, so PUT /api/orderbook/my is the amend route).
            for method in METHODS:
                if method != operation["method"] and not any(
                    row["method"] == method
                    and any(re.fullmatch(pattern_regexp(p), sample) for p in ledger_paths(row))
                    for row in OPERATIONS
                ):
                    assert not owned(snippet, method, sample), (method, sample)
            for neighbour in (sample + "/extra", sample.upper()):
                assert not owned(snippet, operation["method"], neighbour), neighbour
            if not path.endswith("/") and path != "/api/trades":
                assert not owned(snippet, operation["method"], sample + "/"), sample


def _rust_routes() -> set[tuple[str, str]]:
    routes = set()
    for source_file in sorted(RUST_COMPONENT_DIR.glob("*.rs")):
        source = source_file.read_text()
        constants = dict(re.findall(r'const ([A-Z_]+): &str = "([^"]+)";', source))
        for start in [match.end() for match in re.finditer(r"\.route\(", source)]:
            depth, index = 1, start
            while depth:
                depth += {"(": 1, ")": -1}.get(source[index], 0)
                index += 1
            call = source[start:index - 1]
            literal = re.match(r'\s*"([^"]+)"', call)
            path = literal.group(1) if literal else constants[re.match(r"\s*([A-Z_]+)", call).group(1)]
            for method in re.findall(r"\b(get|post|put|patch|delete)\(", call):
                routes.add((method.upper(), path))
    return routes


def test_switch_routes_equal_the_rust_router():
    expected = {
        (operation["method"], path)
        for operation in OPERATIONS
        for path in ledger_paths(operation)
    }
    assert _rust_routes() == expected
    for operation in OPERATIONS:
        component = ROOT / operation["rust_component"]
        assert f'"{operation["pattern"]}"' in component.read_text(), operation["id"]


def _python_routes() -> set[tuple[str, str, int]]:
    routes = set()
    for relative, prefix in PYTHON_MARKET_ROUTERS.items():
        tree = ast.parse((ROOT / relative).read_text())
        for node in ast.walk(tree):
            if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            for decorator in node.decorator_list:
                if (
                    isinstance(decorator, ast.Call)
                    and isinstance(decorator.func, ast.Attribute)
                    and isinstance(decorator.func.value, ast.Name)
                    and decorator.func.value.id == "router"
                    and decorator.func.attr in {"get", "post", "put", "patch", "delete"}
                ):
                    path = prefix + decorator.args[0].value
                    routes.add((decorator.func.attr.upper(), path, decorator.lineno))
    return routes


def _normal(path: str) -> str:
    return re.sub(r"\{[^}]+\}", "{}", path)


def test_no_python_market_write_route_remains():
    switched = {(row["method"], _normal(row["pattern"])) for row in OPERATIONS}
    python = _python_routes()
    writes = {(method, _normal(path)) for method, path, _ in python if method != "GET"}
    assert writes and writes <= switched, sorted(writes - switched)
    reads = {(method, path) for method, path, _ in python if method == "GET"}
    remaining = {(m, p) for m, p in reads if (m, _normal(p)) not in switched}
    assert remaining == PYTHON_READS_OUTSIDE_THE_SWITCH
    # Every ledger row names the decorator it replaces.
    lines = {(method, _normal(path)): line for method, path, line in python}
    for row in OPERATIONS:
        source = row["python_source"]
        if source is None:
            assert row["id"].startswith("freeze."), row["id"]
            continue
        assert lines[(row["method"], _normal(row["pattern"]))] == source["line"], row["id"]


def test_market_invalidation_callers_are_rust_owned():
    callers = []
    for path in sorted((ROOT / "app").rglob("*.py")):
        if path.name == "market_invalidation.py":
            continue
        text = path.read_text()
        if "app.services.market_invalidation" in text or "market_invalidation import" in text:
            callers.append(path.relative_to(ROOT).as_posix())
    assert callers == ["app/routers/auth_simple.py"]
    tree = ast.parse((ROOT / "app/routers/auth_simple.py").read_text())
    routes = []
    for node in ast.walk(tree):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and any(
            isinstance(call, ast.Call)
            and getattr(call.func, "id", None) == "invalidate_organization_market_access"
            for call in ast.walk(node)
        ):
            for decorator in node.decorator_list:
                if (
                    isinstance(decorator, ast.Call)
                    and isinstance(decorator.func, ast.Attribute)
                    and getattr(decorator.func.value, "id", None) == "router"
                ):
                    routes.append((decorator.func.attr.upper(), "/api/auth" + decorator.args[0].value))
    assert routes == [("PUT", "/api/auth/organization/{organization_id}/reject")]
    phase4 = matchers((ROOT / "deploy/caddy/rust-phase4.caddy").read_text())
    assert owned(phase4, "PUT", concrete(routes[0][1]))


def test_phase7_snippet_is_the_reviewed_copy():
    for path, digest in PHASE7_SHA256.items():
        assert hashlib.sha256(path.read_bytes()).hexdigest() == digest, path
    retired = json.loads(PHASE7_LEDGER.read_text())
    assert retired["counts"]["total"] == len(retired["operations"]) == 20


def test_market_and_phase7_matchers_do_not_overlap_released_snippets():
    released = []
    for name in RELEASED_SNIPPETS:
        released.extend(matchers((ROOT / "deploy/caddy" / name).read_text()))
    market = matchers(MARKET_SNIPPET.read_text())
    phase7 = matchers(PHASE7_SNIPPET.read_text())
    assert len(phase7) == 20
    for operation in OPERATIONS:
        for path in ledger_paths(operation):
            sample = concrete(path)
            assert not owned(released, operation["method"], sample), (operation["id"], owned(released, operation["method"], sample))
            assert not owned(phase7, operation["method"], sample), operation["id"]
    for operation in json.loads(PHASE7_LEDGER.read_text())["operations"]:
        sample = concrete(operation["path"])
        for method in operation["methods"]:
            assert not owned(released, method, sample), (method, sample)
            assert not owned(market, method, sample), (method, sample)


def test_python_answers_each_write_slash_variant_with_a_redirect():
    """The snippet leaves slash variants of the write paths on Python, which
    redirects each one to its canonical path rather than serving it."""
    from starlette.routing import Match

    from app.main import app

    canonical = {}
    for route in app.router.routes:
        for method in getattr(route, "methods", None) or ():
            canonical.setdefault(method, []).append(route)
    for operation in OPERATIONS:
        if (
            operation["method"] == "GET"
            or operation["pattern"].endswith("/")
            or operation["python_source"] is None
        ):
            continue
        sample = concrete(operation["pattern"])
        scope = {"type": "http", "method": operation["method"], "path": sample + "/"}
        full = [
            route
            for route in app.router.routes
            if route.matches(scope)[0] == Match.FULL
        ]
        assert not full, (operation["id"], [route.path for route in full])
        target = {"type": "http", "method": operation["method"], "path": sample}
        assert app.router.redirect_slashes
        assert any(route.matches(target)[0] == Match.FULL for route in app.router.routes), operation["id"]
    # Python's canonical order-taking path is the slash form; its slashless
    # form redirects there, and the snippet sends both forms to Rust.
    taking = {"type": "http", "method": "POST", "path": "/api/trades/"}
    assert any(route.matches(taking)[0] == Match.FULL for route in app.router.routes)

"""Exercise exact route ownership through a real disposable Caddy."""

import contextlib
import http.server
import os
import shutil
import socket
import subprocess
import threading
import time
from pathlib import Path

import httpx
import pytest

ROOT = Path(__file__).resolve().parents[2]
UUID = "11111111-1111-4111-8111-111111111111"
GROUPS = {
    "SIMPLE": ["/api/ports", "/api/ports/SGSIN", "/api/catalog/products",
               "/api/catalog/delivery-points", "/api/news"],
    "ORDERBOOK": ["/api/orderbook" + suffix for suffix in (
        "", "/snapshot", "/bids", "/asks", "/with-ci", "/aggregated",
        "/product-counts", "/map-summary", "/map-summary/compact", "/products",
        "/regions", "/fuel-types",
    )] + ["/api/listings"],
    "PRICES": ["/api/prices", "/api/prices/reference", "/api/prices/reference/export",
               "/api/trade-tape"],
    "CURVES": ["/api/curves/forward" + suffix for suffix in (
        "", "/export", "/table", "/slice", "/board",
    )],
    "MCP": ["/mcp"],
}

# Phase 2 Task 22: authenticated CRUD groups routed by exact method and path.
# Each entry is (method, path, expected owner group). Path parameters match any
# non-empty segment, as Python's route converter does, so a malformed value is
# the route's own 422 and not a routing decision.
CRUD_CASES = [
    ("GET", "/api/notifications", "NOTIFICATIONS"),
    ("GET", "/api/notifications/unread-count", "NOTIFICATIONS"),
    ("PATCH", f"/api/notifications/{UUID}/read", "NOTIFICATIONS"),
    ("PATCH", "/api/notifications/read-all", "NOTIFICATIONS"),
    ("GET", "/api/users/me/preferences", "PREFERENCES"),
    ("PUT", "/api/users/me/preferences/market", "PREFERENCES"),
    ("POST", "/api/feedback", "FEEDBACK"),
    ("GET", "/api/admin/feedback", "FEEDBACK"),
    ("GET", "/api/watchlists", "WATCHLISTS"),
    ("POST", "/api/watchlists", "WATCHLISTS"),
    ("POST", "/api/watchlists/me", "WATCHLISTS"),
    ("GET", f"/api/watchlists/{UUID}", "WATCHLISTS"),
    # This operation-shaped match reaches Rust's 405 refusal for the static
    # POST-only /me route rather than falling through to the UUID handler.
    ("GET", "/api/watchlists/me", "WATCHLISTS"),
    ("GET", f"/api/watchlists/{UUID}/events", "WATCHLISTS"),
    ("POST", f"/api/watchlists/{UUID}/targets", "WATCHLISTS"),
    ("PATCH", f"/api/watchlists/{UUID}/events/{UUID}", "WATCHLISTS"),
    ("DELETE", f"/api/watchlists/{UUID}", "WATCHLISTS"),
    ("DELETE", f"/api/watchlists/{UUID}/targets/{UUID}", "WATCHLISTS"),
    ("GET", "/api/alerts", "ALERTS"),
    ("POST", "/api/alerts", "ALERTS"),
    ("DELETE", f"/api/alerts/{UUID}", "ALERTS"),
    ("GET", "/api/referrals/my-referrals", "REFERRALS"),
    ("GET", "/api/referrals/leaderboard", "REFERRALS"),
    ("GET", "/api/referrals/resolve/CODE123", "REFERRALS"),
    ("POST", "/api/referrals/my-code", "REFERRALS"),
    ("POST", "/api/referrals/invite", "REFERRALS"),
]

CRUD_GROUPS = ("NOTIFICATIONS", "PREFERENCES", "FEEDBACK", "WATCHLISTS",
               "ALERTS", "REFERRALS")

# Paths that must stay on Python whatever the CRUD upstreams are set to.
PYTHON_OWNED = [
    # Legacy entry adapters W09/W10 stay on Python until Python is removed.
    ("POST", f"/api/watchlists/{UUID}/entries"),
    ("DELETE", f"/api/watchlists/{UUID}/entries/{UUID}"),
    # Auth, SSE, market writes and admin routes outside feedback.
    ("POST", "/api/auth/login"),
    ("GET", "/api/stream/trades"),
    ("POST", "/api/orderbook"),
    ("PUT", f"/api/orderbook/{UUID}"),
    ("DELETE", f"/api/orderbook/{UUID}"),
    ("POST", "/api/trades/"),
    ("GET", "/api/admin/analytics"),
    ("GET", "/api/admin/users"),
    ("POST", "/api/kyc/submit"),
    ("POST", "/api/ai/chat"),
    # Extra path segments never match the operation-shaped matchers.
    ("GET", f"/api/watchlists/{UUID}/events/extra"),
    ("DELETE", f"/api/watchlists/{UUID}/targets/{UUID}/extra"),
    ("POST", f"/api/watchlists/{UUID}/entries/extra"),
    ("GET", "/api/notifications/1/read"),
    ("GET", "/api/referrals/my-code"),
    ("OPTIONS", "/api/alerts"),
    ("PROPFIND", "/api/alerts"),
    # Wrong method on a matched path stays on Python, and so does HEAD: the
    # canonical Python path has no trailing slash and Python owns redirects.
    ("PUT", "/api/notifications"),
    ("POST", "/api/alerts/extra/segments"),
    ("HEAD", "/api/alerts"),
    ("GET", "/api/unknown"),
    ("GET", "/health/ready"),
]



@contextlib.contextmanager
def upstream(owner):
    class Handler(http.server.BaseHTTPRequestHandler):
        def respond(self):
            if self.path == "/internal/admin-preauth":
                self.send_response(204)
                self.send_header("X-Verdaxis-Preauth-Counted", "1")
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            self.send_response(200)
            self.send_header("X-Route-Owner", owner)
            self.send_header("X-Seen-Host", self.headers["Host"])
            self.send_header("Content-Length", "0")
            self.end_headers()

        do_GET = do_HEAD = do_OPTIONS = do_POST = do_DELETE = do_PUT = respond
        do_PATCH = do_PROPFIND = respond

        def log_message(self, *_args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"127.0.0.1:{server.server_port}"
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


@pytest.fixture(scope="module", params=[None, *GROUPS, *CRUD_GROUPS])
def route_owner(request, tmp_path_factory):
    """Run Caddy and roll back each group independently, without a mock parser."""
    caddy = os.environ.get("CADDY_BIN") or shutil.which("caddy")
    if not caddy:
        caddy = str(ROOT / "work/rust-phase1/bin/caddy")
    assert Path(caddy).is_file(), "Set CADDY_BIN to the local Caddy executable"
    # The shared fixture and snippet retain live flush/keepalive policy for
    # either owner, including the MCP POST path during all-Python preparation.
    # The CRUD groups mirror that split: one group rolls back to Python while
    # every other group stays on Rust, so each handle is proven both ways.
    with upstream("python") as python, upstream("rust") as rust:
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        env = {
            **os.environ,
            "ROUTE_SNIPPET": str(ROOT / "deploy/caddy/rust-public-staging.caddy"),
            "CRUD_SNIPPET": str(ROOT / "deploy/caddy/rust-crud.caddy"),
            "CADDY_PORT": str(port), "PYTHON_UPSTREAM": python,
            # The single pre-auth authority is Rust in every mix; the stub
            # admits with 200, as the service does with 204.
            "PREAUTH_UPSTREAM": rust,
            **{f"{group}_UPSTREAM": python if group == request.param else rust
               for group in (*GROUPS, *CRUD_GROUPS)},
        }
        config = ROOT / "tests/rust_contract/Caddyfile"
        subprocess.run([caddy, "adapt", "--config", str(config), "--validate"],
                       env=env, check=True, capture_output=True)
        log_path = tmp_path_factory.mktemp("routing") / "caddy.log"
        with log_path.open("w") as log:
            process = subprocess.Popen([caddy, "run", "--config", str(config)],
                                       env=env, stdout=log, stderr=log)
            try:
                with httpx.Client(base_url=f"http://127.0.0.1:{port}",
                                  trust_env=False, follow_redirects=False) as client:
                    for _ in range(100):
                        assert process.poll() is None, log_path.read_text()
                        try:
                            client.get("/health/live")
                            break
                        except httpx.ConnectError:
                            time.sleep(0.05)
                    else:
                        pytest.fail("disposable Caddy did not start")

                    def owner(method, path):
                        response = client.request(method, path, headers={"Host": "routing.test"})
                        assert response.headers["X-Seen-Host"] == "routing.test"
                        return response.headers["X-Route-Owner"]

                    owner.rollback = request.param
                    yield owner
            finally:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


@pytest.mark.parametrize("group,path", [(g, p) for g, paths in GROUPS.items() for p in paths])
def test_inventory_and_method_matrix(route_owner, group, path):
    expected = "python" if route_owner.rollback == group else "rust"
    for method in ("GET", "HEAD", "OPTIONS", "POST", "DELETE", "PUT"):
        ported = method == "GET" or (group == "MCP" and method in ("POST", "DELETE"))
        assert route_owner(method, path) == (expected if ported else "python")
        assert route_owner(method, path + "/") == (
            expected if method == "GET" and group != "MCP" else "python"
        )


@pytest.mark.parametrize("path", [
    "/api/ports/a/b", "/api/unknown", "/unknown", "/openapi.json", "/docs", "/",
    "/health", "/health/ready", "/health/live", "/.well-known/verdaxis-disposable-test",
    "/api/availability", "/api/demand", "/api/producers", "/api/benchmarks",
    "/api/compliance/fuels", "/api/orderbook/snapshot/unknown", "/api/listings/unknown",
])
def test_excluded_paths_stay_python(route_owner, path):
    assert route_owner("GET", path) == "python"


@pytest.mark.parametrize(
    ("method", "path"),
    [
        ("GET", "/api/orderbook/my"),
        ("GET", "/api/orderbook/my/latest-ask-template"),
        ("POST", "/api/orderbook"),
        ("DELETE", "/api/orderbook/00000000-0000-0000-0000-000000000001"),
        ("GET", "/api/listings/my"),
        ("GET", "/api/stream/orderbook"),
    ],
)
def test_private_and_write_routes_stay_python(route_owner, method, path):
    assert route_owner(method, path) == "python"


@pytest.mark.parametrize(("method", "path", "group"), CRUD_CASES)
def test_crud_operations_follow_their_group(route_owner, method, path, group):
    """One group argument moves every one of its operations together."""
    expected = "python" if route_owner.rollback == group else "rust"
    assert route_owner(method, path) == expected
    # Slash variants stay on Python, which owns the redirect for every
    # canonical Phase 2 path.
    assert route_owner(method, path + "/") == "python"


@pytest.mark.parametrize(("method", "path"), PYTHON_OWNED)
def test_phase2_python_owned_paths_stay_python(route_owner, method, path):
    assert route_owner(method, path) == "python"


def test_feedback_post_and_admin_get_share_one_group_argument(route_owner):
    """F01 and F02 must move together: one group argument owns both."""
    rolled = route_owner.rollback == "FEEDBACK"
    assert route_owner("POST", "/api/feedback") == ("python" if rolled else "rust")
    assert route_owner("GET", "/api/admin/feedback") == ("python" if rolled else "rust")
    # Another admin path never follows the feedback group.
    assert route_owner("GET", "/api/admin/analytics") == "python"


def test_r05_moves_with_referrals_without_gaining_authentication(route_owner):
    """R05 is public resolution; the split must not add an auth requirement."""
    rolled = route_owner.rollback == "REFERRALS"
    # An unauthenticated probe still reaches the group's upstream: Caddy adds
    # no authentication, and a public route needs no bearer.
    assert route_owner("GET", "/api/referrals/resolve/CODE123") == (
        "python" if rolled else "rust")

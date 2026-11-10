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
# non-empty segment, as the router's converter does, so a malformed value is
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

# Paths that no snippet owns whatever the upstreams are set to: the site's
# own fallback answers them.
UNOWNED = [
    # Legacy entry adapters W09/W10 have no handle.
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
    # A wrong method on a matched path finds no handle, and so does HEAD: the
    # canonical path has no trailing slash and the site answers the variant.
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
    """Run Caddy and move each group's upstream independently, without a mock parser."""
    caddy = os.environ.get("CADDY_BIN") or shutil.which("caddy")
    if not caddy:
        caddy = str(ROOT / "work/rust-phase1/bin/caddy")
    assert Path(caddy).is_file(), "Set CADDY_BIN to the local Caddy executable"
    # The shared fixture and snippet retain live flush/keepalive policy for
    # either owner, including the MCP POST path. The CRUD groups mirror that
    # split: one group points at a second owner while every other group stays,
    # so each handle is proven both ways.
    with upstream("second") as second, upstream("primary") as primary:
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        env = {
            **os.environ,
            "ROUTE_SNIPPET": str(ROOT / "deploy/caddy/rust-public-staging.caddy"),
            "CRUD_SNIPPET": str(ROOT / "deploy/caddy/rust-crud.caddy"),
            "CADDY_PORT": str(port),
            # The single pre-auth authority follows the primary upstream.
            "PREAUTH_UPSTREAM": primary,
            **{f"{group}_UPSTREAM": second if group == request.param else primary
               for group in (*GROUPS, *CRUD_GROUPS)},
        }
        config = ROOT / "tests/tooling/Caddyfile"
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
                            client.get("/api/ports")
                            break
                        except httpx.ConnectError:
                            time.sleep(0.05)
                    else:
                        pytest.fail("disposable Caddy did not start")

                    def owner(method, path):
                        response = client.request(method, path, headers={"Host": "routing.test"})
                        assert response.headers["X-Seen-Host"] == "routing.test"
                        return response.headers["X-Route-Owner"]

                    owner.moved = request.param
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
    expected = "second" if route_owner.moved == group else "primary"
    for method in ("GET", "HEAD", "OPTIONS", "POST", "DELETE", "PUT"):
        owned = method == "GET" or (group == "MCP" and method in ("POST", "DELETE"))
        # A method no handle owns answers from the site's own fallback; the
        # stub upstreams never see it, so the owner stays the primary group.
        assert route_owner(method, path) == (expected if owned else "primary")
        assert route_owner(method, path + "/") == (
            expected if method == "GET" and group != "MCP" else "primary"
        )


@pytest.mark.parametrize("path", [
    "/api/ports/a/b", "/api/unknown", "/unknown", "/openapi.json", "/docs", "/",
    "/health", "/health/ready", "/health/live", "/.well-known/verdaxis-disposable-test",
    "/api/availability", "/api/demand", "/api/producers", "/api/benchmarks",
    "/api/compliance/fuels", "/api/orderbook/snapshot/unknown", "/api/listings/unknown",
])
def test_excluded_paths_reach_no_handle(route_owner, path):
    assert route_owner("GET", path) == "primary"


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
def test_private_and_write_routes_find_no_handle(route_owner, method, path):
    assert route_owner(method, path) == "primary"


@pytest.mark.parametrize(("method", "path", "group"), CRUD_CASES)
def test_crud_operations_follow_their_group(route_owner, method, path, group):
    """One group argument moves every one of its operations together."""
    expected = "second" if route_owner.moved == group else "primary"
    assert route_owner(method, path) == expected
    # Slash variants find no handle; the site answers them.
    assert route_owner(method, path + "/") == "primary"


@pytest.mark.parametrize(("method", "path"), UNOWNED)
def test_phase2_unowned_paths_reach_no_handle(route_owner, method, path):
    assert route_owner(method, path) == "primary"


def test_feedback_post_and_admin_get_share_one_group_argument(route_owner):
    """F01 and F02 must move together: one group argument owns both."""
    moved = route_owner.moved == "FEEDBACK"
    assert route_owner("POST", "/api/feedback") == ("second" if moved else "primary")
    assert route_owner("GET", "/api/admin/feedback") == ("second" if moved else "primary")


def test_r05_moves_with_referrals_without_gaining_authentication(route_owner):
    """R05 is public resolution; the split must not add an auth requirement."""
    moved = route_owner.moved == "REFERRALS"
    # An unauthenticated probe still reaches the group's upstream: Caddy adds
    # no authentication, and a public route needs no bearer.
    assert route_owner("GET", "/api/referrals/resolve/CODE123") == (
        "second" if moved else "primary")

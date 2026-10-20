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


@contextlib.contextmanager
def upstream(owner):
    class Handler(http.server.BaseHTTPRequestHandler):
        def respond(self):
            self.send_response(200)
            self.send_header("X-Route-Owner", owner)
            self.send_header("X-Seen-Host", self.headers["Host"])
            self.send_header("Content-Length", "0")
            self.end_headers()

        do_GET = do_HEAD = do_OPTIONS = do_POST = do_DELETE = do_PUT = respond

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


@pytest.fixture(scope="module", params=[None, *GROUPS])
def route_owner(request, tmp_path_factory):
    """Run Caddy and roll back each group independently, without a mock parser."""
    caddy = os.environ.get("CADDY_BIN") or shutil.which("caddy")
    if not caddy:
        caddy = str(ROOT / "work/rust-phase1/bin/caddy")
    assert Path(caddy).is_file(), "Set CADDY_BIN to the local Caddy executable"
    # The shared fixture and snippet retain live flush/keepalive policy for
    # either owner, including the MCP POST path during all-Python preparation.
    with upstream("python") as python, upstream("rust") as rust:
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        env = {
            **os.environ,
            "ROUTE_SNIPPET": str(ROOT / "deploy/caddy/rust-public-staging.caddy"),
            "CADDY_PORT": str(port), "PYTHON_UPSTREAM": python,
            **{f"{group}_UPSTREAM": python if group == request.param else rust
               for group in GROUPS},
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

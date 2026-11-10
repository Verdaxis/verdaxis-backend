"""Exercise the exact Phase 5 route row through a real disposable Caddy.

The site imports the first read groups and the Phase 5 snippet together, as the
reviewed site files do; every unmatched path answers from the site's own
fallback. Stub owners answer with their label: ``phase1`` (the first read
groups) and ``phase5`` (the Phase 5 upstream). The method and path row comes
from the neutral route inventory, so this file and the Rust routing cases pin
the same ten routes.

The disposable Caddy must be a real 2.11 binary: a missing binary fails the
test instead of skipping it.
"""

import contextlib
import http.server
import json
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
INVENTORY = json.loads((ROOT / "tests/tooling/phase5/routes.json").read_text())
VESSEL = "11111111-1111-4111-8111-111111111111"
PORT_ID = "SGSIN"
METHODS = ("GET", "HEAD", "OPTIONS", "POST", "PUT", "PATCH", "DELETE")

CADDYFILE = """\
{
	admin off
	auto_https off
}
import {$ROUTE_SNIPPET}
import {$PHASE5_SNIPPET}
http://:{$CADDY_PORT} {
	bind 127.0.0.1
	route {
		import verdaxis_rust_public {$PHASE1_UPSTREAM} {$PHASE1_UPSTREAM} {$PHASE1_UPSTREAM} {$PHASE1_UPSTREAM} {$PHASE1_UPSTREAM}
		import verdaxis_rust_phase5 {$PHASE5_UPSTREAM}
		handle {
			respond 404
		}
	}
}
"""


def concrete(path):
    return path.replace("{vessel_id}", VESSEL).replace("{port_id}", PORT_ID)


PHASE5_ROUTES = [(row["method"], concrete(row["path"]), row["auth"]) for row in INVENTORY["routes"]]
PHASE1_ROUTES = [concrete(row["path"]) for row in INVENTORY["excluded_routes"]]

# Paths that look like Phase 5 routes but are not in the row: longer, shorter,
# differently cased or with an empty or nested vessel segment.
NEIGHBOURS = [
    "/api/benchmark", "/api/benchmarksx", "/api/benchmarks/extra", "/api/availability/x",
    "/api/demand/x", "/api/fleet-intelligence/x", "/api/producers/1",
    "/api/compliance", "/api/compliance/", "/api/compliance/other",
    "/api/compliance/fuels/x", "/api/compliance/fleet/x",
    "/api/compliance/vessels", "/api/compliance/vessels/", "/api/compliance/vessels/x",
    "/api/compliance/vessels/x/", "/api/compliance/vessels//score",
    "/api/compliance/vessels/a/b/score", f"/api/compliance/vessels/{VESSEL}/score/extra",
    "/api/compliance/pricing-overlay/extra", "/api/compliance/scenario/x",
    "/API/BENCHMARKS", "/api/Compliance/Fleet", "/api/compliance/FLEET",
]
# Caddy merges repeated slashes before it matches a path, so these reach the
# Phase 5 owner; the Rust service answers them 404.
MERGED_SLASHES = ["/api//benchmarks", "/api/compliance//fleet"]


@contextlib.contextmanager
def upstream(owner):
    class Handler(http.server.BaseHTTPRequestHandler):
        def respond(self):
            length = int(self.headers.get("Content-Length") or 0)
            body = self.rfile.read(length) if length else b""
            self.send_response(200)
            self.send_header("X-Route-Owner", owner)
            self.send_header("X-Seen-Host", self.headers["Host"])
            self.send_header("X-Seen-Target", self.path)
            self.send_header("X-Seen-Forwarded-For", self.headers.get("X-Forwarded-For", ""))
            self.send_header("X-Seen-Body", body.decode())
            self.send_header("Content-Length", "0")
            self.end_headers()

        do_GET = do_HEAD = do_OPTIONS = do_POST = do_DELETE = do_PUT = do_PATCH = respond

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


def free_port():
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        return reservation.getsockname()[1]


@pytest.fixture(scope="module", params=("primary", "rust-stopped"))
def site(request, tmp_path_factory):
    """One Caddy per Phase 5 state: the normal owner, or Rust not running."""
    caddy = os.environ.get("CADDY_BIN") or shutil.which("caddy")
    assert caddy and Path(caddy).is_file(), "Set CADDY_BIN to the local Caddy executable"
    with upstream("phase1") as phase1, upstream("phase5") as phase5:
        phase5_upstream = {
            "primary": phase5,
            "rust-stopped": f"127.0.0.1:{free_port()}",
        }[request.param]
        port = free_port()
        work = tmp_path_factory.mktemp("phase5-routing")
        config = work / "Caddyfile"
        config.write_text(CADDYFILE)
        env = {
            **os.environ,
            "ROUTE_SNIPPET": str(ROOT / "deploy/caddy/rust-public-staging.caddy"),
            "PHASE5_SNIPPET": str(ROOT / "deploy/caddy/rust-phase5.caddy"),
            "CADDY_PORT": str(port),
            "PHASE1_UPSTREAM": phase1,
            "PHASE5_UPSTREAM": phase5_upstream,
        }
        subprocess.run([caddy, "adapt", "--config", str(config), "--validate"],
                       env=env, check=True, capture_output=True)
        log_path = work / "caddy.log"
        with log_path.open("w") as log:
            process = subprocess.Popen([caddy, "run", "--config", str(config)],
                                       env=env, stdout=log, stderr=log)
            try:
                with httpx.Client(base_url=f"http://127.0.0.1:{port}", trust_env=False,
                                  follow_redirects=False, timeout=30) as client:
                    for _ in range(100):
                        assert process.poll() is None, log_path.read_text()
                        try:
                            client.get("/health/live")
                            break
                        except httpx.ConnectError:
                            time.sleep(0.05)
                    else:
                        pytest.fail("disposable Caddy did not start")

                    def send(method, path, **kwargs):
                        headers = {"Host": "routing.test", **kwargs.pop("headers", {})}
                        return client.request(method, path, headers=headers, **kwargs)

                    send.phase5_owner = {"primary": "phase5",
                                         "rust-stopped": "unavailable"}[request.param]
                    yield send
            finally:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


def owner(site, method, path, **kwargs):
    response = site(method, path, **kwargs)
    detail = f"{method} {path} answered {response.status_code} {dict(response.headers)}"
    if response.status_code in (404, 502, 503):
        assert "X-Route-Owner" not in response.headers, detail
        return "fallback" if response.status_code == 404 else "unavailable"
    assert response.status_code == 200 and "X-Route-Owner" in response.headers, detail
    return response.headers["X-Route-Owner"]


def test_inventory_has_the_ten_routes():
    assert len(PHASE5_ROUTES) == 10
    assert sorted(method for method, _path, _auth in PHASE5_ROUTES) == ["GET"] * 8 + ["POST"] * 2
    assert len(PHASE1_ROUTES) == 27


@pytest.mark.parametrize("method,path,auth", PHASE5_ROUTES)
def test_each_route_moves_with_its_exact_method_only(site, method, path, auth):
    for candidate in METHODS:
        expected = site.phase5_owner if candidate == method else "fallback"
        assert owner(site, candidate, path) == expected, f"{candidate} {path}"


@pytest.mark.parametrize("method,path,auth", PHASE5_ROUTES)
def test_trailing_slash_follows_the_rust_redirect_rule(site, method, path, auth):
    # The Rust service redirects the slash form of a public read itself. An
    # authenticated route has no Rust redirect, so its slash form finds no
    # handle for every method.
    for candidate in METHODS:
        moves = auth == "public" and candidate == "GET"
        expected = site.phase5_owner if moves else "fallback"
        assert owner(site, candidate, path + "/") == expected, f"{candidate} {path}/"


@pytest.mark.parametrize("path", NEIGHBOURS)
def test_neighbouring_paths_find_no_handle(site, path):
    for candidate in ("GET", "POST", "PUT", "PATCH", "DELETE"):
        assert owner(site, candidate, path) == "fallback", f"{candidate} {path}"


@pytest.mark.parametrize("path", MERGED_SLASHES)
def test_repeated_slashes_follow_the_merged_path(site, path):
    assert owner(site, "GET", path) == site.phase5_owner
    assert owner(site, "POST", path) == "fallback"


@pytest.mark.parametrize("path", PHASE1_ROUTES)
def test_first_read_groups_keep_their_owner(site, path):
    # The Phase 5 state never changes the 27 existing reads.
    assert owner(site, "GET", path) == "phase1", path
    assert owner(site, "POST", path) == "fallback", path


def test_mcp_and_health_are_unchanged(site):
    assert owner(site, "POST", "/mcp") == "phase1"
    for path in ("/health/live", "/health/ready", "/health", "/", "/docs", "/openapi.json"):
        assert owner(site, "GET", path) == "fallback", path


def test_requests_reach_the_owner_unchanged(site):
    response = site("GET", "/api/producers?limit=2&q=a%20b", headers={"X-Forwarded-For": "203.0.113.9"})
    if site.phase5_owner == "unavailable":
        assert response.status_code == 502
        return
    assert response.headers["X-Route-Owner"] == site.phase5_owner
    assert response.headers["X-Seen-Host"] == "routing.test"
    assert response.headers["X-Seen-Target"] == "/api/producers?limit=2&q=a%20b"
    # A client-supplied forwarding header is replaced by the connecting address.
    assert response.headers["X-Seen-Forwarded-For"] == "127.0.0.1"
    body = json.dumps({"label": "x", "year": 2026})
    response = site("POST", "/api/compliance/scenario", content=body,
                    headers={"Content-Type": "application/json"})
    if site.phase5_owner == "unavailable":
        assert response.status_code == 502
        return
    assert response.headers["X-Route-Owner"] == site.phase5_owner
    assert response.headers["X-Seen-Body"] == body

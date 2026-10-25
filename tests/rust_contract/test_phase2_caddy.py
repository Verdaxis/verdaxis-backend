"""Real-Caddy decision-78 gate for the Phase 2 admin split (Caddy 2.11).

The authority is one Rust process that counts every ``/api/admin/*`` request
before Caddy chooses an owner. These tests run the real routing snippet against
small local upstreams: a counting authority, one Python owner and one Rust
owner. One case replaces the counting stub with the real Rust quota middleware.
They need a real Caddy 2.11 binary on the path and fail without one, so a gate
run can never pass by skipping them.
"""
from __future__ import annotations

import contextlib
import http.server
import os
import pathlib
import shutil
import socket
import subprocess
import threading
import time

import httpx
import pytest

ROOT = pathlib.Path(__file__).resolve().parents[2]
UUID = "11111111-1111-4111-8111-111111111111"
COUNTED = "X-Verdaxis-Preauth-Counted"
LIMIT = 300

CADDYFILE = """\
{
	admin off
	auto_https off
}
import {$CRUD_SNIPPET}

http://127.0.0.1:{$CADDY_PORT} {
	route {
		import verdaxis_rust_crud {$NOTIFICATIONS_UPSTREAM} {$PREFERENCES_UPSTREAM} {$FEEDBACK_UPSTREAM} {$WATCHLISTS_UPSTREAM} {$ALERTS_UPSTREAM} {$REFERRALS_UPSTREAM} {$PREAUTH_UPSTREAM} {$PYTHON_UPSTREAM}
		handle {
			reverse_proxy {$PYTHON_UPSTREAM} {
				header_up X-Forwarded-For {client_ip}
				lb_try_duration 15s
				lb_try_interval 250ms
				flush_interval -1
				transport http {
					keepalive off
				}
			}
		}
	}
}
"""


class _Server(http.server.ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True


class Stub:
    """A local upstream that records what it received and answers as told."""

    def __init__(self, label: str, *, python: bool = False,
                 authority: bool = False, crash: bool = False, port: int = 0,
                 authority_mark: bool = True):
        self.label = label
        self.python = python
        self.authority = authority
        self.authority_mark = authority_mark
        self.crash = crash
        self.requests: list[dict] = []
        self.lock = threading.Lock()
        self._start(port)

    def _start(self, port: int) -> None:
        parent = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *_args):
                pass

            def finish(self):
                try:
                    super().finish()
                except OSError:
                    pass

            def _body(self) -> bytes:
                length = int(self.headers.get("Content-Length") or 0)
                return self.rfile.read(length) if length else b""

            def _respond(self) -> None:
                body = self._body()
                path = self.path.split("?", 1)[0]
                with parent.lock:
                    parent.requests.append({
                        "method": self.command,
                        "path": self.path,
                        "headers": {k.lower(): v for k, v in self.headers.items()},
                        "body": body,
                    })
                if parent.crash:
                    # The connection succeeded and the request bytes arrived;
                    # close before any response so Caddy sees a round-trip error.
                    self.close_connection = True
                    try:
                        self.connection.shutdown(socket.SHUT_RDWR)
                    except OSError:
                        pass
                    return
                if parent.authority and path == "/internal/admin-preauth":
                    accepted = parent.authority_admits()
                    if not accepted:
                        payload = b'{"detail":"Rate limit exceeded"}'
                        self.send_response(429)
                        self.send_header("Content-Type", "application/json")
                        self.send_header("Retry-After", "60")
                        self.send_header("Content-Length", str(len(payload)))
                        self.end_headers()
                        self.wfile.write(payload)
                        return
                    self.send_response(204)
                    if parent.authority_mark:
                        self.send_header(COUNTED, "1")
                    self.send_header("Content-Length", "0")
                    self.end_headers()
                    return
                if parent.python and path == "/internal/admin-preauth":
                    # Python has no authority route; 404 is the fail-open signal.
                    self.send_response(404)
                    self.send_header("Content-Length", "0")
                    self.end_headers()
                    return
                self.send_response(200)
                self.send_header("X-Route-Owner", parent.label)
                if body:
                    # Echo the body so a test can prove it crossed Caddy intact.
                    self.send_header("Content-Type", "application/octet-stream")
                    self.send_header("Content-Length", str(len(body)))
                else:
                    self.send_header("Content-Length", "0")
                self.end_headers()
                if body:
                    self.wfile.write(body)

            do_GET = do_HEAD = do_OPTIONS = do_POST = do_DELETE = do_PUT = _respond
            do_PATCH = do_PROPFIND = _respond

        self.server = _Server(("127.0.0.1", port), Handler)
        self.port = self.server.server_port
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def stop(self) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=10)

    def restart(self) -> None:
        self._start(self.port)

    def requested(self, path: str | None = None) -> list[dict]:
        with self.lock:
            if path is None:
                return list(self.requests)
            return [item for item in self.requests if item["path"].split("?", 1)[0] == path]

    def authority_admits(self) -> bool:
        with self.lock:
            admitted = sum(item["path"] == "/internal/admin-preauth"
                           for item in self.requests) <= LIMIT
        return admitted


def unused_port(exclude: set[int] = frozenset()) -> int:
    while True:
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        if port not in exclude:
            return port


@contextlib.contextmanager
def harness(tmp_path, *, feedback_owner: str = "rust", authority_down: bool = False,
           feedback_crash: bool = False, trusted_proxy: bool = False,
           all_python: bool = False, shared_authority: bool = False,
           authority_mark: bool = True, real_authority: str | None = None):
    """Run the real CRUD snippet with one counting authority and two owners."""
    caddy = shutil.which("caddy")
    if not caddy:
        pytest.fail("real Caddy 2.11 is required for the test-host gate")
    rust = Stub("rust", authority=shared_authority)
    authority = rust if shared_authority else Stub(
        "authority", authority=True, authority_mark=authority_mark)
    python = Stub("python", python=True)
    crash = Stub("crash", crash=True) if feedback_crash else None
    used = {authority.port, rust.port, python.port}
    if crash is not None:
        used.add(crash.port)
    if authority_down:
        with authority.lock:
            used.add(authority.port)
        authority.stop()
    caddy_port = unused_port(used)
    config = tmp_path / f"Caddyfile-{caddy_port}"
    configuration = CADDYFILE
    if trusted_proxy:
        configuration = configuration.replace(
            "\tauto_https off\n", "\tauto_https off\n\tservers {\n"
            "\t\ttrusted_proxies static 127.0.0.1/32\n\t}\n")
    config.write_text(configuration)
    env = {
        **os.environ,
        "CRUD_SNIPPET": str(ROOT / "deploy/caddy/rust-crud.caddy"),
        "CADDY_PORT": str(caddy_port),
        "NOTIFICATIONS_UPSTREAM": f"127.0.0.1:{rust.port}",
        "PREFERENCES_UPSTREAM": f"127.0.0.1:{rust.port}",
        "WATCHLISTS_UPSTREAM": f"127.0.0.1:{rust.port}",
        "ALERTS_UPSTREAM": f"127.0.0.1:{rust.port}",
        "REFERRALS_UPSTREAM": f"127.0.0.1:{rust.port}",
        "FEEDBACK_UPSTREAM": (
            f"127.0.0.1:{crash.port}" if crash is not None
            else f"127.0.0.1:{(rust if feedback_owner == 'rust' else python).port}"),
        "PREAUTH_UPSTREAM": f"127.0.0.1:{authority.port}",
        "PYTHON_UPSTREAM": f"127.0.0.1:{python.port}",
    }
    if real_authority:
        env["PREAUTH_UPSTREAM"] = real_authority
        env["FEEDBACK_UPSTREAM"] = real_authority
    if all_python:
        for group in ("NOTIFICATIONS", "PREFERENCES", "FEEDBACK", "WATCHLISTS", "ALERTS", "REFERRALS"):
            env[f"{group}_UPSTREAM"] = env["PYTHON_UPSTREAM"]
    for verb in ("validate", "adapt"):
        result = subprocess.run(
            [caddy, verb, "--config", str(config), "--adapter", "caddyfile"],
            env=env, capture_output=True, text=True, timeout=30)
        assert result.returncode == 0, result.stderr
    log = (tmp_path / f"caddy-{caddy_port}.log").open("w")
    process = subprocess.Popen(
        [caddy, "run", "--config", str(config), "--adapter", "caddyfile"],
        env=env, stdout=log, stderr=log)
    try:
        with httpx.Client(
                base_url=f"http://127.0.0.1:{caddy_port}",
                trust_env=False,
                timeout=httpx.Timeout(30.0, read=15.0)) as client:
            for _ in range(200):
                if process.poll() is not None:
                    pytest.fail((tmp_path / f"caddy-{caddy_port}.log").read_text())
                try:
                    if client.get("/health/live").status_code == 200:
                        break
                except httpx.HTTPError:
                    pass
                time.sleep(0.05)
            else:
                pytest.fail("disposable Caddy did not start")
            yield client, authority, rust, python, crash
    finally:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        log.close()
        for stub in dict.fromkeys((authority, rust, python, crash)):
            if stub is not None:
                with contextlib.suppress(Exception):
                    stub.stop()


@pytest.mark.parametrize("feedback_owner", ["rust", "python"])
def test_trusted_proxy_preserves_client_for_gate_and_owner(tmp_path, feedback_owner):
    with harness(tmp_path, feedback_owner=feedback_owner, trusted_proxy=True) as (
            client, authority, rust, python, _crash):
        response = client.get(
            "/api/admin/feedback", headers={"X-Forwarded-For": "198.51.100.24"})
        assert response.status_code == 200
        owner = rust if feedback_owner == "rust" else python
        assert authority.requested()[0]["headers"]["x-forwarded-for"] == "198.51.100.24"
        assert owner.requested("/api/admin/feedback")[0]["headers"]["x-forwarded-for"] == "198.51.100.24"


def test_real_rust_authority_stops_mixed_caddy_at_301(tmp_path):
    environment = {**os.environ, "RELEASE_SHA": subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(), "SQLX_OFFLINE": "true"}
    subprocess.run([
        "cargo", "build", "--manifest-path", "rs/Cargo.toml", "--locked",
        "-p", "verdaxis-read", "--features", "contract-tests", "--example", "preauth_server",
    ], cwd=ROOT, env=environment, check=True, timeout=600)
    port = unused_port()
    log = (tmp_path / "rust-authority.log").open("w")
    process = subprocess.Popen(
        [str(Path(environment.get("CARGO_TARGET_DIR", ROOT / "rs/target")) / "debug/examples/preauth_server")], cwd=ROOT,
        env={**environment, "ENVIRONMENT": "test", "PHASE2_BIND": f"127.0.0.1:{port}"},
        stdout=log, stderr=log)
    try:
        with httpx.Client(trust_env=False) as probe:
            for _ in range(200):
                assert process.poll() is None, "Rust quota adapter stopped"
                try:
                    if probe.get(f"http://127.0.0.1:{port}/health/live").status_code == 200:
                        break
                except httpx.HTTPError:
                    pass
                time.sleep(0.05)
            else:
                pytest.fail("Rust quota adapter did not start")
        with harness(tmp_path, real_authority=f"127.0.0.1:{port}") as (
                client, _authority, _rust, python, _crash):
            for index in range(LIMIT):
                path = "/api/admin/feedback" if index % 2 else "/api/admin/analytics"
                response = client.get(path)
                assert response.status_code == 200, (index, response.text)
            refused = client.get("/api/admin/feedback")
            assert refused.status_code == 429
            assert refused.headers["Retry-After"] == "60"
            assert refused.json()["detail"]["code"] == "RATE_LIMITED"
            assert len(python.requested("/api/admin/analytics")) == LIMIT // 2
    finally:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        log.close()


def test_unmarked_authority_response_does_not_admit_an_owner(tmp_path):
    with harness(tmp_path, authority_mark=False) as (client, _authority, rust, python, _crash):
        response = client.get("/api/admin/feedback")
        assert response.status_code == 503
        assert not rust.requested("/api/admin/feedback")
        assert not python.requested("/api/admin/feedback")


def test_shared_authority_and_route_address_counts_once(tmp_path):
    with harness(tmp_path, shared_authority=True) as (client, authority, rust, python, _crash):
        assert authority is rust
        for index in range(LIMIT):
            path = "/api/admin/feedback" if index % 2 else "/api/admin/analytics"
            assert client.get(path).status_code == 200
        assert client.get("/api/admin/feedback").status_code == 429
        assert len(authority.requested("/internal/admin-preauth")) == LIMIT + 1
        assert len(rust.requested("/api/admin/feedback")) == LIMIT // 2
        assert len(python.requested("/api/admin/analytics")) == LIMIT // 2


def test_admin_split_stops_at_exactly_300_across_both_owners(tmp_path):
    """Python and Rust admin paths draw from one counter, once per request."""
    with harness(tmp_path) as (client, authority, rust, python, _crash):
        # A write also passes the gate. The gate's request copy is changed to
        # GET, so Caddy drops the body from the copy, not from the request the
        # owner receives.
        admin_body = b'{"probe":"the admin body survives the gate"}'
        posted = client.post("/api/admin/analytics", content=admin_body)
        assert posted.status_code == 200
        assert posted.headers["X-Route-Owner"] == "python"
        assert posted.content == admin_body
        assert python.requested("/api/admin/analytics")[0]["body"] == admin_body
        for index in range(LIMIT - 1):
            path = "/api/admin/analytics" if index % 2 == 0 else "/api/admin/feedback"
            response = client.get(path)
            assert response.status_code == 200, (index, path, response.text)
            expected = "python" if index % 2 == 0 else "rust"
            assert response.headers["X-Route-Owner"] == expected, (index, path)

        # One authority call per admin request, no amplification.
        assert len(authority.requested()) == LIMIT
        assert len(python.requested("/api/admin/analytics")) == 1 + LIMIT // 2
        assert len(rust.requested("/api/admin/feedback")) == (LIMIT - 1) // 2
        for owner in (python, rust):
            for item in owner.requested():
                # The readiness probe reaches Python before the gate exists.
                if item["path"].startswith("/api/admin/"):
                    assert item["headers"].get(COUNTED.lower()) == "1", item

        refused = client.get("/api/admin/analytics")
        assert refused.status_code == 429
        assert refused.headers["Retry-After"] == "60"
        assert refused.json() == {"detail": "Rate limit exceeded"}
        # The refused request reached no owner and the authority saw it once.
        assert len(authority.requested()) == LIMIT + 1
        assert len(python.requested("/api/admin/analytics")) == 1 + LIMIT // 2
        assert len(rust.requested("/api/admin/feedback")) == (LIMIT - 1) // 2

        # The authority sees the rewritten GET and the original client only.
        first = authority.requested()[0]
        assert first["method"] == "GET"
        assert first["path"] == "/internal/admin-preauth"
        assert first["body"] == b""
        assert first["headers"]["x-forwarded-for"] == "127.0.0.1"
        assert "x-forwarded-method" not in first["headers"]
        assert "x-forwarded-uri" not in first["headers"]


def test_python_owned_groups_recover_during_restart(tmp_path):
    with harness(tmp_path, all_python=True) as (client, _authority, _rust, python, _crash):
        python.stop()
        # Keep one request in flight across a short restart. A refused dial must
        # not quarantine the sole Python address for subsequent requests.
        timer = threading.Timer(0.3, python.restart)
        timer.start()
        try:
            response = client.post("/api/feedback", content=b"{}")
        finally:
            timer.join(timeout=10)
        assert response.status_code == 200
        for method, path in (("GET", "/api/notifications"),
                             ("POST", "/api/feedback"),
                             ("GET", "/api/alerts")):
            recovered = client.request(method, path)
            assert recovered.status_code == 200
            assert recovered.headers["X-Route-Owner"] == "python"


def test_crud_post_body_crosses_the_gate_intact_to_python(tmp_path):
    """A group rolled back to Python keeps its own counter and body."""
    with harness(tmp_path, feedback_owner="python") as (
            client, authority, _rust, python, _crash):
        body = b'{"message":"the body crosses Caddy byte for byte"}'
        response = client.post(
            "/api/feedback", content=body,
            headers={"Content-Type": "application/json"})
        assert response.status_code == 200
        assert response.headers["X-Route-Owner"] == "python"
        assert response.content == body
        seen = python.requested("/api/feedback")
        assert len(seen) == 1
        assert seen[0]["body"] == body
        # F01 is not an admin path, so the shared authority is not consulted:
        # one group has one owner, so only that owner counts the request.
        assert COUNTED.lower() not in seen[0]["headers"]
        assert authority.requested() == []


def test_write_is_retried_only_after_a_refused_connection(tmp_path):
    """A connection that succeeded is never replayed; a dial error is."""
    with harness(tmp_path, feedback_crash=True) as (
            client, _authority, _rust, python, crash):
        before = len(python.requested("/api/feedback"))
        response = client.post("/api/feedback", content=b'{"message":"once"}')
        assert response.status_code == 502
        assert len(crash.requested("/api/feedback")) == 1
        assert len(python.requested("/api/feedback")) == before

        # A refused connection is a dial error before any request bytes are
        # sent, so the write is retried exactly once on Python.
        crash.stop()
        retried = client.post("/api/feedback", content=b'{"message":"once"}')
        assert retried.status_code == 200
        assert retried.headers["X-Route-Owner"] == "python"
        assert len(python.requested("/api/feedback")) == before + 1


def test_admin_gate_fails_open_when_the_authority_is_down(tmp_path):
    """A stopped authority must not stop admin service or keep a stale mark."""
    with harness(tmp_path, authority_down=True) as (
            client, _authority, _rust, python, _crash):
        response = client.get(
            "/api/admin/analytics",
            headers={COUNTED: "1"})
        assert response.status_code == 200
        assert response.headers["X-Route-Owner"] == "python"
        forwarded = python.requested("/api/admin/analytics")
        assert len(forwarded) == 1
        assert COUNTED.lower() not in forwarded[0]["headers"]
        # The gate itself reached Python's missing authority route and got 404.
        assert python.requested("/internal/admin-preauth")

        # The next request takes the same fail-open path.
        again = client.get("/api/admin/analytics")
        assert again.status_code == 200
        assert again.headers["X-Route-Owner"] == "python"
        assert len(python.requested("/api/admin/analytics")) == 2


@pytest.mark.parametrize("feedback_owner", ["rust", "python"])
def test_one_group_rollback_keeps_the_gate_counting(tmp_path, feedback_owner):
    """Rolling one group back changes its owner, not the shared counter."""
    with harness(tmp_path, feedback_owner=feedback_owner) as (
            client, authority, rust, python, _crash):
        owner = rust if feedback_owner == "rust" else python
        for method, path in (("GET", "/api/admin/feedback"), ("POST", "/api/feedback")):
            response = client.request(method, path, content=b"{}",
                                      headers={"Content-Type": "application/json"})
            assert response.status_code == 200
            assert response.headers["X-Route-Owner"] == feedback_owner
            assert len(owner.requested(path)) == 1
        # Only the admin operation passes the authority gate; F01 has one owner
        # in either position, so its own counter cannot be shared or doubled.
        assert len(authority.requested()) == 1
        other = python if feedback_owner == "rust" else rust
        assert not other.requested("/api/admin/feedback")
        assert not other.requested("/api/feedback")


@pytest.mark.parametrize("method", ["HEAD", "OPTIONS", "PROPFIND"])
def test_admin_nonpreflight_methods_consume_the_shared_quota(tmp_path, method):
    with harness(tmp_path) as (client, authority, _rust, python, _crash):
        response = client.request(method, "/api/admin/analytics")
        assert response.status_code == 200
        assert len(authority.requested()) == 1
        assert len(python.requested("/api/admin/analytics")) == 1


def test_client_mark_does_not_bypass_feedback_quota(tmp_path):
    with harness(tmp_path) as (client, _authority, rust, _python, _crash):
        response = client.post("/api/feedback", headers={COUNTED: "1"})
        assert response.status_code == 200
        assert COUNTED.lower() not in rust.requested("/api/feedback")[0]["headers"]


def test_options_preflight_is_never_counted(tmp_path):
    """A CORS preflight creates no endpoint work and consumes no quota."""
    with harness(tmp_path) as (client, authority, rust, python, _crash):
        response = client.options(
            "/api/admin/feedback",
            headers={"Origin": "https://staging.verdaxis.exchange",
                     "Access-Control-Request-Method": "GET"})
        assert response.status_code == 200
        # OPTIONS matches no operation matcher, so Python owns it uncounted.
        assert response.headers["X-Route-Owner"] == "python"
        assert authority.requested() == []
        assert len(python.requested("/api/admin/feedback")) == 1
        assert not rust.requested("/api/admin/feedback")

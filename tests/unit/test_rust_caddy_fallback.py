"""Real-Caddy failover behavior for the Rust read groups (Caddy 2.11).

These tests need the system Caddy package on the staging test host and skip
elsewhere. They use two small local upstreams as Rust and Python, so a stopped
service is a real refused connection, not a mock.
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

CADDYFILE = """\
{
	admin off
	auto_https off
}
import {$ROUTE_SNIPPET}

http://127.0.0.1:{$CADDY_PORT} {
	route {
		import verdaxis_rust_public {$RUST_UPSTREAM} {$PYTHON_UPSTREAM} {$PYTHON_UPSTREAM} {$PYTHON_UPSTREAM} {$PYTHON_UPSTREAM} {$PYTHON_UPSTREAM}
		handle /post-dial {
			reverse_proxy {$DEAD_UPSTREAM} {$PYTHON_UPSTREAM} {
				lb_policy first
				lb_try_duration 5s
				lb_try_interval 250ms
				fail_duration 30s
				max_fails 1
				flush_interval -1
				transport http {
					keepalive off
				}
			}
		}
		handle /post-after-connect {
			reverse_proxy {$CRASH_UPSTREAM} {$PYTHON_UPSTREAM} {
				lb_policy first
				lb_try_duration 5s
				lb_try_interval 250ms
				fail_duration 30s
				max_fails 1
				flush_interval -1
				transport http {
					keepalive off
				}
			}
		}
		handle {
			reverse_proxy {$PYTHON_UPSTREAM} {
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
    """A local upstream that records method, path and body per request."""

    def __init__(self, label: str, mode: str = "ok", port: int = 0):
        self.label = label
        self.mode = mode
        self.requests: list[tuple[str, str, bytes]] = []
        self.lock = threading.Lock()
        self.first_event = threading.Event()
        self.second_event = threading.Event()
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

            def _record(self, body: bytes = b"") -> None:
                with parent.lock:
                    parent.requests.append((self.command, self.path, body))

            def do_GET(self):
                self._record()
                if self.path.split("?", 1)[0] == "/sse":
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.send_header("Cache-Control", "no-cache")
                    self.send_header("Connection", "close")
                    self.end_headers()
                    self.wfile.write(b"data: first\n\n")
                    self.wfile.flush()
                    parent.first_event.set()
                    parent.second_event.wait(10)
                    self.wfile.write(b"data: second\n\n")
                    self.wfile.flush()
                    return
                self.send_response(200)
                self.send_header("X-Route-Owner", parent.label)
                self.send_header("Content-Length", "0")
                self.end_headers()

            def do_POST(self):
                length = int(self.headers.get("Content-Length") or 0)
                body = self.rfile.read(length)
                self._record(body)
                if parent.mode == "crash":
                    # The connection succeeded and the request bytes arrived;
                    # close before any response so Caddy sees a round-trip error.
                    self.close_connection = True
                    try:
                        self.connection.shutdown(socket.SHUT_RDWR)
                    except OSError:
                        pass
                    return
                self.send_response(200)
                self.send_header("X-Route-Owner", parent.label)
                self.send_header("Content-Length", "0")
                self.end_headers()

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

    def count(self, method: str) -> int:
        with self.lock:
            return sum(1 for recorded, _, _ in self.requests if recorded == method)


def unused_address(exclude: set[int] = frozenset()) -> tuple[str, int]:
    while True:
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            address = reservation.getsockname()
        if address[1] not in exclude:
            return address


@contextlib.contextmanager
def harness(tmp_path):
    caddy = shutil.which("caddy")
    if not caddy:
        pytest.skip("real Caddy 2.11 is required for the test-host gate")
    rust = Stub("rust")
    python = Stub("python")
    crash = Stub("crash", mode="crash")
    used = {rust.port, python.port, crash.port}
    dead = unused_address(used)
    caddy_port = unused_address(used | {dead[1]})[1]
    config = tmp_path / "Caddyfile"
    config.write_text(CADDYFILE)
    env = {
        **os.environ,
        "ROUTE_SNIPPET": str(ROOT / "deploy/caddy/rust-public-staging.caddy"),
        "CADDY_PORT": str(caddy_port),
        "RUST_UPSTREAM": f"127.0.0.1:{rust.port}",
        "PYTHON_UPSTREAM": f"127.0.0.1:{python.port}",
        "CRASH_UPSTREAM": f"127.0.0.1:{crash.port}",
        "DEAD_UPSTREAM": f"127.0.0.1:{dead[1]}",
    }
    # The same document must pass Caddy's own parser and adapter before it runs.
    for verb in ("validate", "adapt"):
        result = subprocess.run(
            [caddy, verb, "--config", str(config), "--adapter", "caddyfile"],
            env=env, capture_output=True, text=True, timeout=30)
        print(f"fallback caddy {verb} EXIT={result.returncode}")
        assert result.returncode == 0, result.stderr
    log = (tmp_path / "caddy.log").open("w")
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
                    pytest.fail((tmp_path / "caddy.log").read_text())
                try:
                    if client.get("/health/live").status_code == 200:
                        break
                except httpx.HTTPError:
                    pass
                time.sleep(0.05)
            else:
                pytest.fail("disposable Caddy did not start")
            yield client, rust, python, crash
    finally:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        log.close()
        for stub in (rust, python, crash):
            stub.stop()


def test_rust_serves_a_get_and_python_serves_it_when_rust_is_down(tmp_path):
    with harness(tmp_path) as (client, rust, _python, _crash):
        response = client.get("/api/ports")
        assert response.status_code == 200
        assert response.headers["X-Route-Owner"] == "rust"
        rust.stop()
        # The first request retries the refused connection; passive health then
        # sends later requests straight to Python. Neither returns 502.
        for _ in range(2):
            response = client.get("/api/ports")
            assert response.status_code == 200
            assert response.headers["X-Route-Owner"] == "python"


def test_get_waits_for_python_to_come_back(tmp_path):
    with harness(tmp_path) as (client, _rust, python, _crash):
        python.stop()
        result = {}

        def request():
            try:
                with httpx.Client(trust_env=False,
                                  timeout=httpx.Timeout(30.0, read=20.0)) as waiting:
                    result["status"] = waiting.get(
                        f"{client.base_url}/health/live").status_code
            except Exception as exc:  # noqa: BLE001 - reported as a failure below
                result["error"] = repr(exc)

        thread = threading.Thread(target=request, daemon=True)
        started = time.monotonic()
        thread.start()
        time.sleep(5)
        python.restart()
        thread.join(timeout=25)
        assert not thread.is_alive(), result
        assert "error" not in result, result
        assert result["status"] == 200
        assert time.monotonic() - started >= 5


def test_post_is_sent_once_after_a_refused_connection(tmp_path):
    with harness(tmp_path) as (client, _rust, python, _crash):
        before = python.count("POST")
        response = client.post("/post-dial", content=b"payload")
        assert response.status_code == 200
        assert response.headers["X-Route-Owner"] == "python"
        assert python.count("POST") - before == 1


def test_post_is_not_replayed_after_the_connection_succeeded(tmp_path):
    with harness(tmp_path) as (client, _rust, python, crash):
        before = python.count("POST")
        response = client.post("/post-after-connect", content=b"payload")
        assert response.status_code == 502
        assert crash.count("POST") == 1
        assert python.count("POST") == before


def test_sse_response_streams_through_the_catch_all(tmp_path):
    with harness(tmp_path) as (client, _rust, python, _crash):
        with client.stream("GET", "/sse") as response:
            assert response.status_code == 200
            lines = response.iter_lines()
            assert next(lines) == "data: first"
            # The upstream is still holding the second event: the first reached
            # the client, so flush_interval -1 still streams.
            assert not python.second_event.is_set()
            python.second_event.set()
            assert next(lines) == ""
            assert next(lines) == "data: second"

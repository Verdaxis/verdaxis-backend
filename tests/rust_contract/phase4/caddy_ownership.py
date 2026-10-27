"""Qualify the committed selector against disposable loopback Caddy only."""
from __future__ import annotations

import http.server
import json
import pathlib
import re
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[3]


def main():
    class Owner(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.send_header("X-Owner", "rust")
            self.end_headers()
        do_POST = do_PUT = do_PATCH = do_DELETE = do_HEAD = do_GET
        def log_message(self, *args):
            pass
    upstream = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Owner)
    threading.Thread(target=upstream.serve_forever, daemon=True).start()
    with socket.socket() as reserved:
        reserved.bind(("127.0.0.1", 0))
        port = reserved.getsockname()[1]
    routes = json.loads((ROOT / "tests/rust_contract/phase4/routes.json").read_text())["routes"]
    paths = {}
    for route in routes:
        path = re.sub(r"\{[^}]+\}", "00000000-0000-0000-0000-000000000000", route["path"])
        paths.setdefault(path, set()).add(route["method"])
    with tempfile.TemporaryDirectory(prefix="phase4-caddy-") as directory:
        config = pathlib.Path(directory) / "Caddyfile"
        config.write_text("{\n admin off\n auto_https off\n}\n" + (ROOT / "deploy/caddy/rust-phase4.caddy").read_text() + f"\nhttp://127.0.0.1:{port} {{\n route {{\n import verdaxis_rust_phase4 127.0.0.1:{upstream.server_port}\n handle {{\n header X-Owner python\n respond 200\n }}\n }}\n}}\n")
        subprocess.run(["caddy", "validate", "--config", str(config), "--adapter", "caddyfile"], check=True)
        process = subprocess.Popen(["caddy", "run", "--config", str(config), "--adapter", "caddyfile"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            for _ in range(100):
                if process.poll() is not None:
                    raise AssertionError("disposable Caddy exited")
                try:
                    with socket.create_connection(("127.0.0.1", port), timeout=.1):
                        break
                except OSError:
                    time.sleep(.05)
            corpus = []
            for path, methods in paths.items():
                corpus.extend((method, path, "rust" if method in methods else "python") for method in ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"])
                corpus.extend((method, path + suffix, "python") for method in methods for suffix in ["/", "/extra"])
            corpus.extend((method, path, "python") for method, path in [("POST", "/api/auth/login"), ("POST", "/api/auth/refresh"), ("POST", "/api/auth/invitations/accept"), ("POST", "/api/ai/chat"), ("POST", "/api/orderbook"), ("POST", "/api/admin/market-support/authorizations"), ("GET", "/api/unknown")])
            for method, path, expected in corpus:
                expected = "rust" if any(
                    route["method"] == method and re.fullmatch(re.sub(r"\{[^}]+\}", "[^/]+", route["path"]), path)
                    for route in routes
                ) else "python"
                request = urllib.request.Request(f"http://127.0.0.1:{port}{path}", method=method)
                with urllib.request.urlopen(request, timeout=5) as response:
                    assert response.headers.get("X-Owner") == expected, (method, path, expected)
            print(f"CADDY_OWNERSHIP=ok OPERATIONS={len(routes)} CASES={len(corpus)}")
        finally:
            process.terminate()
            process.wait(timeout=10)
            upstream.shutdown()
            upstream.server_close()


if __name__ == "__main__":
    main()

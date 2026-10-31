"""Runner-owned loopback mail provider. Payloads stay in the private ledger."""
from __future__ import annotations

import copy
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class MailStub:
    def __init__(self):
        self._lock = threading.Lock()
        self._attempts = []
        self._status = 200
        self._body = b'{"id":"disposable-send"}'
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                parts = self.path.strip("/").split("/")
                if len(parts) != 2 or parts[1] != "emails":
                    self.send_error(404)
                    return
                self.connection.settimeout(5)
                try:
                    size = int(self.headers.get("content-length", "0"))
                    if not 0 < size <= 1024 * 1024:
                        raise ValueError("mail body size")
                    payload = json.loads(self.rfile.read(size))
                except (ValueError, OSError):
                    self.send_error(400)
                    return
                with owner._lock:
                    status, body = owner._status, owner._body
                    owner._attempts.append({"cell": parts[0], "payload": payload,
                                            "status": status,
                                            "idempotency_key": self.headers.get("idempotency-key")})
                self.send_response(status)
                self.send_header("content-type", "application/json")
                self.send_header("content-length", str(len(body)))
                self.end_headers()
                try:
                    self.wfile.write(body)
                except OSError:
                    pass

            def log_message(self, *_args):
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, name="phase2-mail")
        self.url = f"http://127.0.0.1:{self.server.server_port}"

    def respond(self, *, status=200, body=b'{"id":"disposable-send"}'):
        if not 200 <= status <= 599 or not isinstance(body, bytes):
            raise ValueError("invalid disposable provider response")
        with self._lock:
            self._status, self._body = status, body

    def ledger(self, cell):
        with self._lock:
            return [copy.deepcopy({key: value for key, value in row.items() if key != "cell"})
                    for row in self._attempts if row["cell"] == cell]

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_args):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=10)
        if self.thread.is_alive():
            raise RuntimeError("mail provider did not stop")

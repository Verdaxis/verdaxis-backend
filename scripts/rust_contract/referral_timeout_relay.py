"""Test relay: capture one real ledger send, then stall the provider response body."""
import threading
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

STALL_SECONDS = 15


def start(target):
    """Serve a loopback provider URL that forwards each send to the runner ledger.

    The relay answers with a status line and an announced body that never
    arrives, so a client that waits for the whole response times out while the
    ledger still holds exactly one real send for that attempt.
    """
    stopped = threading.Event()

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            body = self.rfile.read(int(self.headers['content-length']))
            headers = {'content-type': 'application/json'}
            if self.headers.get('idempotency-key'):
                headers['idempotency-key'] = self.headers['idempotency-key']
            request = urllib.request.Request(target, data=body, headers=headers)
            with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=5) as response:
                response.read()
            self.send_response(200)
            self.send_header('content-length', '100')
            self.end_headers()
            self.wfile.flush()
            stopped.wait(STALL_SECONDS)

        def log_message(self, *args):
            pass

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return f'http://127.0.0.1:{server.server_port}/emails'

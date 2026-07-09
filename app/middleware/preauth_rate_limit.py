"""Pre-auth rate limiting for sensitive path prefixes.

slowapi limits run inside the endpoint function, so a request that fails
authentication in a dependency (e.g. an invalid bearer token hitting
/api/admin/*) never reaches the limiter and can be replayed without bound
(Sprint 3 items 5 and 9). This middleware throttles by client IP before
routing/dependencies run, as a coarse backstop — the per-endpoint slowapi
limits remain the precise layer for authenticated traffic.

Notes:
- Counters are per-process (uvicorn runs multiple workers), so the
  effective ceiling is limit x workers. Acceptable for a backstop.
- A Caddy-level rate limit would sit even earlier, but stock Caddy lacks
  the rate_limit plugin and rebuilding the shared ingress binary risks
  prod; this app-level layer was chosen instead (2026-07-04 decision).
"""
import time

from fastapi import Request
from fastapi.responses import JSONResponse

# (path prefix, max requests, window seconds). Checked in order; first
# match wins. Limits are deliberately generous — they exist to stop
# unauthenticated brute-force/replay loops, not to shape normal traffic.
PREAUTH_LIMITS: tuple[tuple[str, int, int], ...] = (
    ("/api/auth/login", 30, 60),
    ("/api/auth/forgot-password", 15, 60),
    ("/api/admin/", 300, 60),
)

# {(prefix, client_ip): (window_start, count)}
_buckets: dict[tuple[str, str], tuple[float, int]] = {}
_MAX_BUCKETS = 50_000  # hard cap so a spoofed-IP flood cannot exhaust memory


def _check(prefix: str, limit: int, window: int, client_ip: str, now: float) -> bool:
    """Fixed-window counter. Returns True if the request is allowed."""
    key = (prefix, client_ip)
    started, count = _buckets.get(key, (now, 0))
    if now - started >= window:
        started, count = now, 0
    if count >= limit:
        return False
    if len(_buckets) >= _MAX_BUCKETS and key not in _buckets:
        _buckets.clear()  # crude but safe: reset under pathological load
    _buckets[key] = (started, count + 1)
    return True


def client_ip(request: Request) -> str:
    """Real client IP behind Caddy.

    uvicorn runs without --proxy-headers, so request.client.host is always
    the proxy (127.0.0.1). Caddy *appends* the peer address to any incoming
    X-Forwarded-For, so the LAST entry is the address Caddy actually saw —
    earlier entries are client-controlled and must not be trusted.
    """
    forwarded = request.headers.get("X-Forwarded-For", "")
    if forwarded:
        return forwarded.rsplit(",", 1)[-1].strip()
    return request.client.host if request.client else "unknown"


async def preauth_rate_limit_middleware(request: Request, call_next):
    path = request.url.path
    for prefix, limit, window in PREAUTH_LIMITS:
        if path.startswith(prefix):
            if not _check(prefix, limit, window, client_ip(request), time.monotonic()):
                return JSONResponse(
                    status_code=429,
                    content={"detail": "Rate limit exceeded"},
                    headers={"Retry-After": str(window)},
                )
            break
    return await call_next(request)

"""
Rate limiting via slowapi (per-IP, per-route granularity).

Usage in routers:
    from app.rate_limit import limiter
    
    @router.get("/endpoint")
    @limiter.limit("60/minute")
    async def my_endpoint(request: Request, ...):
"""
import ipaddress

from slowapi import Limiter


def client_ip(request) -> str:
    """Trust Caddy's final forwarded hop only from a loopback peer."""
    peer = request.client.host if request.client else "unknown"
    try:
        peer_is_loopback = ipaddress.ip_address(peer).is_loopback
    except ValueError:
        peer_is_loopback = False
    if peer_is_loopback:
        forwarded = request.headers.get("X-Forwarded-For", "")
        if forwarded:
            candidate = forwarded.rsplit(",", 1)[-1].strip()
            try:
                return str(ipaddress.ip_address(candidate))
            except ValueError:
                pass
    return peer

limiter = Limiter(key_func=client_ip)

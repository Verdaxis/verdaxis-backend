"""
Rate limiting via slowapi (per-IP, per-route granularity).

S7-006: Extended to support OAuth2 client-aware rate limiting.

Tiers:
  public (no auth / invalid token) : 30  req/min
  free   (user session or free OAuth2 client) : 120 req/min
  paid   (OAuth2 client with paid tier) : 600 req/min

Usage in routers:
    from app.rate_limit import limiter, oauth_key_func

    @router.get("/endpoint")
    @limiter.limit("60/minute")
    async def my_endpoint(request: Request, ...):
        ...

    # For tiered OAuth2 endpoints, use the tier-aware key function:
    @router.get("/data/reference-prices")
    @limiter.limit(get_tiered_rate_limit)
    async def tiered_endpoint(request: Request, ...):
        ...
"""
from typing import Optional

from fastapi import Request
from slowapi import Limiter
from slowapi.util import get_remote_address


# ---------------------------------------------------------------------------
# Tier-aware key function
# ---------------------------------------------------------------------------

def _extract_token(request: Request) -> Optional[str]:
    """Pull raw Bearer token from Authorization header."""
    auth = request.headers.get("Authorization", "")
    if auth.startswith("Bearer "):
        return auth[7:]
    return None


def _decode_safe(token: Optional[str]) -> Optional[dict]:
    if not token:
        return None
    try:
        from app.core.security import decode_token
        return decode_token(token)
    except Exception:
        return None


def oauth_key_func(request: Request) -> str:
    """
    Rate-limit key for OAuth2-aware endpoints.

    - OAuth2 client tokens: keyed by client_id (UUID string)
    - User session tokens: keyed by user UUID from sub claim
    - Unauthenticated: keyed by remote IP
    """
    token = _extract_token(request)
    payload = _decode_safe(token)
    if payload is None:
        return get_remote_address(request)

    token_kind = payload.get("token_kind")
    if token_kind == "oauth2_client":
        return f"client:{payload.get('sub', get_remote_address(request))}"

    sub = payload.get("sub")
    if sub:
        return f"user:{sub}"

    return get_remote_address(request)


def get_tiered_rate_limit(request: Request) -> str:
    """
    Return the slowapi limit string based on the caller's data tier.

    Used as the `limit` argument to `@limiter.limit(...)`.
    """
    token = _extract_token(request)
    payload = _decode_safe(token)
    if payload is None:
        return "30/minute"

    token_kind = payload.get("token_kind")
    if token_kind == "oauth2_client":
        tier = payload.get("rate_limit_tier", "free")
        if tier == "paid":
            return "600/minute"
        return "120/minute"

    # User session token
    return "120/minute"


# ---------------------------------------------------------------------------
# Primary limiter instance (IP-based by default)
# ---------------------------------------------------------------------------

limiter = Limiter(key_func=get_remote_address)

# OAuth-aware limiter for data-product endpoints
oauth_limiter = Limiter(key_func=oauth_key_func)

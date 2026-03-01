"""
Rate limiting via slowapi (per-IP, per-route granularity).

Usage in routers:
    from app.rate_limit import limiter
    
    @router.get("/endpoint")
    @limiter.limit("60/minute")
    async def my_endpoint(request: Request, ...):
"""
from slowapi import Limiter
from slowapi.util import get_remote_address

limiter = Limiter(key_func=get_remote_address)

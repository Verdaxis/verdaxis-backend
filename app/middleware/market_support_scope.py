"""Fail closed before any unclassified mutation can use a support context."""
from __future__ import annotations

from starlette.middleware.base import BaseHTTPMiddleware
from starlette.requests import Request
from starlette.responses import JSONResponse
from uuid import UUID

from app.services.request_party import (
    MARKET_SUPPORT_CONTEXT_HEADER,
    is_market_support_mutation_allowed,
)


class MarketSupportScopeMiddleware(BaseHTTPMiddleware):
    async def dispatch(self, request: Request, call_next):
        if not is_market_support_mutation_allowed(
            request.method, request.url.path, context_id=UUID(int=0)
        ) and request.method.upper() == "GET":
            return JSONResponse(
                status_code=405,
                content={
                    "detail": {
                        "code": "STATE_CHANGING_GET_NOT_ALLOWED",
                        "message": "State-changing operations must use their declared mutation method",
                    }
                },
            )
        raw_context = request.headers.get(MARKET_SUPPORT_CONTEXT_HEADER)
        if raw_context and not is_market_support_mutation_allowed(
            request.method, request.url.path, context_id=UUID(int=0)
        ):
            return JSONResponse(
                status_code=403,
                content={
                    "detail": {
                        "code": "MARKET_SUPPORT_MUTATION_NOT_ALLOWED",
                        "message": "This operation is not available in Market Support mode",
                    }
                },
            )
        return await call_next(request)

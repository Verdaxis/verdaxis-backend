"""Fail-fast admission for public market-data reads."""

from __future__ import annotations

import anyio
from starlette.responses import JSONResponse
from starlette.types import ASGIApp, Receive, Scope, Send


_PUBLIC_MARKET_READ_PATHS = frozenset(
    {
        "/availability",
        "/benchmarks",
        "/catalog/products",
        "/catalog/delivery-points",
        "/curves/forward",
        "/curves/forward/table",
        "/curves/forward/slice",
        "/curves/forward/board",
        "/curves/forward/export",
        "/demand",
        "/listings",
        "/orderbook",
        "/orderbook/snapshot",
        "/orderbook/bids",
        "/orderbook/asks",
        "/orderbook/with-ci",
        "/orderbook/aggregated",
        "/orderbook/product-counts",
        "/orderbook/map-summary",
        "/orderbook/map-summary/compact",
        "/orderbook/products",
        "/orderbook/regions",
        "/orderbook/fuel-types",
        "/ports",
        "/prices",
        "/prices/reference",
        "/prices/reference/export",
        "/producers",
        "/trade-tape",
    }
)
_BUSY_DETAIL = "Public market data is temporarily busy; retry shortly."


class PublicMarketReadAdmissionMiddleware:
    """Reserve bounded capacity for selected public market-data GETs."""

    def __init__(self, app: ASGIApp, *, capacity: int, api_prefix: str) -> None:
        if capacity < 0:
            raise ValueError("capacity must be non-negative")
        if not api_prefix.startswith("/"):
            raise ValueError("api_prefix must start with '/'")

        self.app = app
        self._api_prefix = api_prefix.rstrip("/")
        self._read_limiter = anyio.CapacityLimiter(capacity) if capacity else None

    def _is_selected_path(self, path: str) -> bool:
        prefix_with_separator = f"{self._api_prefix}/"
        if not path.startswith(prefix_with_separator):
            return False

        relative_path = path[len(self._api_prefix) :]
        if relative_path in _PUBLIC_MARKET_READ_PATHS:
            return True

        if relative_path.startswith("/ports/"):
            port_id = relative_path.removeprefix("/ports/")
            return bool(port_id) and "/" not in port_id

        return False

    async def _reject_busy(
        self,
        scope: Scope,
        receive: Receive,
        send: Send,
    ) -> None:
        response = JSONResponse(
            status_code=503,
            content={"detail": _BUSY_DETAIL},
            headers={"Retry-After": "1"},
        )
        await response(scope, receive, send)

    async def __call__(
        self,
        scope: Scope,
        receive: Receive,
        send: Send,
    ) -> None:
        if (
            scope["type"] != "http"
            or scope.get("method") != "GET"
            or not self._is_selected_path(scope["path"])
        ):
            await self.app(scope, receive, send)
            return

        if self._read_limiter is None:
            await self._reject_busy(scope, receive, send)
            return

        try:
            self._read_limiter.acquire_nowait()
        except anyio.WouldBlock:
            await self._reject_busy(scope, receive, send)
            return

        try:
            await self.app(scope, receive, send)
        finally:
            self._read_limiter.release()

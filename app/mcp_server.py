"""Public, read-only market tools for non-production MCP clients."""
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager
from datetime import UTC, datetime

import anyio
from mcp.server.mcpserver import MCPServer
from mcp.server.mcpserver.exceptions import ToolError
from mcp.server.transport_security import TransportSecuritySettings
from mcp_types import ToolAnnotations
from pydantic import ValidationError
from starlette.applications import Starlette
from starlette.responses import Response
from starlette.types import ASGIApp, Receive, Scope, Send
import structlog

from app.config import settings
from app.database import AsyncSessionLocal
from app.market_catalog import CANONICAL_DELIVERY_POINTS, CANONICAL_PRODUCTS
from app.schemas.voyage import (
    DeliveryPointInfo,
    MarketCatalog,
    MarketProductInfo,
    MarketSliceSnapshot,
    VoyagePlanRequest,
    VoyagePlanResult,
)
from app.services.availability_windows import normalize_availability_window
from app.services.voyage_fuel_plan import (
    EU_PORTS,
    VoyagePlanError,
    build_voyage_plan,
    required_price_lookups,
)
from app.services.voyage_prices import (
    MarketDataUnavailable,
    market_slice_snapshot,
    resolve_product_prices,
)

MCP_ENVIRONMENTS = frozenset({"development", "test", "staging"})
MCP_PATH = "/mcp"
# Below the per-worker pool of 3 checkouts plus 1 for in-memory work.
MCP_MAX_CONCURRENT_TOOL_CALLS = 4

tool_limiter = anyio.CapacityLimiter(MCP_MAX_CONCURRENT_TOOL_CALLS)
logger = structlog.get_logger(__name__)


def mcp_enabled(environment: str) -> bool:
    return environment in MCP_ENVIRONMENTS


def market_catalog() -> MarketCatalog:
    return MarketCatalog(
        products=tuple(
            MarketProductInfo(
                code=product.market_product.value,
                name=product.name,
                fuel_type=product.fuel_type,
                unit=product.unit,
                min_lot_size=product.min_lot_size,
                delivery_points=tuple(
                    point.name for point in CANONICAL_DELIVERY_POINTS
                    if product.available_delivery_point_ids is None
                    or point.id in product.available_delivery_point_ids
                ),
            )
            for product in CANONICAL_PRODUCTS
        ),
        delivery_points=tuple(
            DeliveryPointInfo(name=point.name, region=point.region, is_eu=point.name in EU_PORTS)
            for point in CANONICAL_DELIVERY_POINTS
        ),
    )


@asynccontextmanager
async def _tool_call(tool_name: str) -> AsyncIterator[None]:
    try:
        tool_limiter.acquire_nowait()
    except anyio.WouldBlock:
        raise ToolError("Verdaxis market tools are busy; retry shortly.") from None
    try:
        yield
    except MarketDataUnavailable:
        raise ToolError("Market data is temporarily unavailable.") from None
    except (VoyagePlanError, ValueError, ValidationError) as error:
        raise ToolError(str(error)) from None
    except Exception as error:
        # Exception text and tracebacks can include SQL parameters or caller data.
        logger.error("mcp_tool_failed", tool=tool_name, error_type=type(error).__name__)
        raise ToolError("Internal error") from None
    finally:
        tool_limiter.release()


def build_mcp_server() -> MCPServer:
    server = MCPServer(
        name="verdaxis-market",
        instructions=(
            "These tools are read-only. Prices come from Verdaxis market data and caller assumptions. "
            "State the provenance of each price. Never present a demo or benchmark price as executable. "
            "Results are indicative planning estimates, not quotes or offers."
        ),
    )
    annotations = ToolAnnotations(read_only_hint=True, destructive_hint=False, idempotent_hint=True)

    @server.tool(annotations=annotations)
    async def list_markets() -> MarketCatalog:
        """List supported products, delivery ports and EU scope before planning; no database access."""
        async with _tool_call("list_markets"):
            return market_catalog()

    @server.tool(annotations=annotations)
    async def get_market_slice(
        market_product: str, delivery_point: str, availability_window: str,
    ) -> MarketSliceSnapshot:
        """Read anonymous bids, asks and benchmark evidence for a catalog product, port and canonical window.

        Use product codes and port names from list_markets. Windows are SPOT, YYYY-MM or YYYY-QN.
        Preserve demo provenance; benchmarks and demo prices are not executable quotes.
        """
        async with _tool_call("get_market_slice"):
            window = normalize_availability_window(availability_window)
            async with AsyncSessionLocal() as db:
                return await market_slice_snapshot(
                    db, market_product=market_product, delivery_point=delivery_point,
                    availability_window=window, now=datetime.now(UTC),
                )

    @server.tool(annotations=annotations)
    async def plan_voyage_fuel(request: VoyagePlanRequest) -> VoyagePlanResult:
        """Compare indicative voyage fuel, EU ETS and FuelEU strategies, never place orders.

        Supply vessel consumption, connected catalog ports and a start date. Supply VLSFO prices,
        EUA price, pool price and USD/EUR rate when known; missing inputs leave costs unpriced.
        Report price provenance and the annual FuelEU caveat with the recommendation.
        """
        async with _tool_call("plan_voyage_fuel"):
            utcnow = datetime.now(UTC)
            lookups = required_price_lookups(request)
            async with AsyncSessionLocal() as db:
                prices = await resolve_product_prices(db, lookups, now=utcnow)
            return build_voyage_plan(request, prices, today=utcnow.date())

    return server


class _RejectBrowserOrigin:
    """The SDK treats an empty Origin as absent; our agent-only policy does not."""

    def __init__(self, app: ASGIApp) -> None:
        self.app = app

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] == "http" and any(name.lower() == b"origin" for name, _ in scope["headers"]):
            await Response("Invalid Origin header", status_code=403)(scope, receive, send)
            return
        await self.app(scope, receive, send)


def build_mcp_http_app(server: MCPServer) -> Starlette:
    allowed_hosts = (
        ["api-staging.verdaxis.exchange"] if settings.ENVIRONMENT == "staging"
        else ["127.0.0.1:*", "localhost:*", "testserver"]
    )
    app = server.streamable_http_app(
        streamable_http_path=MCP_PATH,
        stateless_http=True,
        json_response=True,
        transport_security=TransportSecuritySettings(
            enable_dns_rebinding_protection=True,
            allowed_hosts=allowed_hosts,
            allowed_origins=[],
        ),
    )
    app.add_middleware(_RejectBrowserOrigin)
    return app

"""Read-only MCP tools exercised through the real streamable HTTP transport."""
import asyncio
from contextlib import asynccontextmanager
from datetime import datetime
from decimal import Decimal
import runpy
import json
from unittest.mock import Mock

import anyio
import httpx
import pytest
from sqlalchemy import event
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine
from starlette.applications import Starlette
from starlette.routing import Route

from app import mcp_server
from app.database import Base
from app.models.user import UserRole, UserStatus
from app.services.voyage_prices import MarketDataUnavailable
from tests.unit.test_voyage_fuel_plan import golden_request
from tests.unit.test_voyage_prices import (
    NOW, REQUIRED_TABLES, _make_delivery_point, _make_order, _make_org,
    _make_product, _make_user,
)

HEADERS = {"Accept": "application/json, text/event-stream"}
SLICE = {"market_product": "UCOME_B100", "delivery_point": "Singapore", "availability_window": "2026-11"}


class FrozenDatetime(datetime):
    @classmethod
    def now(cls, tz=None):
        return NOW if tz else NOW.replace(tzinfo=None)


@pytest.fixture
async def seeded_market(monkeypatch):
    engine = create_async_engine("sqlite+aiosqlite://")
    tables = [Base.metadata.tables[name] for name in REQUIRED_TABLES]
    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all, tables=tables)
    factory = async_sessionmaker(engine, expire_on_commit=False, autoflush=False)
    async with factory() as db:
        org = await _make_org(db)
        user = await _make_user(db, org)
        org.verification_status = "APPROVED"
        user.role = UserRole.SUPPLIER
        user.status = UserStatus.APPROVED
        user.email_verified = True
        user.must_change_password = False
        product = await _make_product(db, "UCOME_B100")
        point = await _make_delivery_point(db)
        order = _make_order(
            org_id=org.id, owner_user_id=user.id, product_id=product.id,
            delivery_point_id=point.id, price="1445", window="2026-11",
            carbon_intensity=None,
        )
        order.fame_terms = {"side": "ASK", "certificate_valid_until": "2027-12-31"}
        db.add(order)
        await db.commit()
        identifiers = [str(org.id), str(user.id), str(order.id)]
    statements = []
    event.listen(engine.sync_engine, "before_cursor_execute", lambda conn, cursor, sql, *args: statements.append(sql))
    monkeypatch.setattr(mcp_server, "AsyncSessionLocal", factory)
    monkeypatch.setattr(mcp_server, "datetime", FrozenDatetime)
    monkeypatch.setattr("app.services.market_data_eligibility.datetime", FrozenDatetime)
    yield identifiers, statements
    await engine.dispose()


@pytest.fixture
async def client(monkeypatch):
    monkeypatch.setattr(mcp_server.settings, "ENVIRONMENT", "test")
    server = mcp_server.build_mcp_server()
    transport_app = mcp_server.build_mcp_http_app(server)

    @asynccontextmanager
    async def lifespan(app):
        async with server.session_manager.run():
            yield

    app = Starlette(routes=[Route(mcp_server.MCP_PATH, transport_app)], lifespan=lifespan)
    # pytest-asyncio runs fixture setup and teardown in separate tasks. Keep
    # the AnyIO lifespan's cancel scope in one task for its entire lifetime.
    ready = asyncio.Event()
    finished = asyncio.Event()

    async def run_lifespan():
        async with app.router.lifespan_context(app):
            ready.set()
            await finished.wait()

    lifespan_task = asyncio.create_task(run_lifespan())
    try:
        await asyncio.wait_for(ready.wait(), timeout=5)
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url="http://testserver", headers=HEADERS) as session:
            yield session
    finally:
        finished.set()
        await lifespan_task


async def rpc(client, method, params=None, **kwargs):
    return await client.post("/mcp", json={"jsonrpc": "2.0", "id": 1, "method": method, "params": params or {}}, **kwargs)


async def call_tool(client, name, arguments=None):
    response = await rpc(client, "tools/call", {"name": name, "arguments": arguments or {}})
    assert response.status_code == 200, response.text
    assert "location" not in response.headers
    return response.json()["result"]


def error_text(result):
    assert result["isError"] is True
    return " ".join(item["text"] for item in result["content"])


async def test_tools_are_listed(client):
    response = await rpc(client, "tools/list")
    assert response.status_code == 200
    tools = response.json()["result"]["tools"]
    assert {tool["name"] for tool in tools} == {"list_markets", "get_market_slice", "plan_voyage_fuel"}
    assert all(tool["inputSchema"] and tool["outputSchema"] for tool in tools)
    assert all(tool["annotations"]["readOnlyHint"] for tool in tools)


async def test_list_markets_has_eight_points_and_rotterdam_is_eu(client, monkeypatch):
    factory = Mock(side_effect=AssertionError("catalog must not open a database"))
    monkeypatch.setattr(mcp_server, "AsyncSessionLocal", factory)
    result = await call_tool(client, "list_markets")
    catalog = result["structuredContent"]
    assert len(catalog["delivery_points"]) == 8
    assert [p["name"] for p in catalog["delivery_points"] if p["is_eu"]] == ["Rotterdam"]
    assert next(p for p in catalog["products"] if p["code"] == "UCOME_B100")["delivery_points"] == ["Singapore"]
    factory.assert_not_called()


async def test_plan_voyage_fuel_golden_through_mcp(client, seeded_market):
    result = await call_tool(client, "plan_voyage_fuel", {"request": golden_request().model_dump(mode="json")})
    plan = result["structuredContent"]
    assert plan["recommended_strategy_id"] == "CONVENTIONAL_POOL"
    pool = next(s for s in plan["strategies"] if s["strategy_id"] == "CONVENTIONAL_POOL")
    assert pool["total_cost_usd"] == "1223044.44"
    green = next(s for s in plan["strategies"] if s["strategy_id"] == "UCOME_B100_FULL")
    price = next(u["price"] for u in green["fuel_uses"] if u["fuel"] == "UCOME_B100")
    assert price["source"] == "LIVE_ORDER"
    assert price["observed_at"] == "2026-10-14T12:00:00Z"


async def test_invalid_port_is_tool_error_not_500(client):
    request = golden_request().model_dump(mode="json")
    request["legs"][0]["from_port"] = "Hamburg"
    result = await call_tool(client, "plan_voyage_fuel", {"request": request})
    assert "Supported ports:" in error_text(result)
    assert "Rotterdam" in error_text(result)


async def test_no_identifiers_in_any_tool_output(client, seeded_market):
    identifiers, statements = seeded_market
    results = [
        await call_tool(client, "list_markets"),
        await call_tool(client, "get_market_slice", SLICE),
        await call_tool(client, "plan_voyage_fuel", {"request": golden_request().model_dump(mode="json")}),
    ]
    assert all(not result.get("isError") for result in results)
    output = json.dumps(results)
    assert all(identifier not in output for identifier in identifiers)
    assert statements and all(sql.lstrip().upper().startswith("SELECT") for sql in statements)


async def test_busy_limiter_returns_retry_message(client, monkeypatch):
    # AnyIO requires total_tokens >= 1. A zero-capacity double models refusal.
    limiter = Mock()
    limiter.acquire_nowait.side_effect = anyio.WouldBlock
    monkeypatch.setattr(mcp_server, "tool_limiter", limiter)
    for name, arguments in [("list_markets", {}), ("get_market_slice", SLICE), ("plan_voyage_fuel", {"request": golden_request().model_dump(mode="json")})]:
        result = await call_tool(client, name, arguments)
        assert "Verdaxis market tools are busy; retry shortly." in error_text(result)
    limiter.release.assert_not_called()


@pytest.mark.parametrize("origin", ["https://evil.example", "http://testserver", "", "null"])
async def test_browser_origin_rejected(client, origin):
    response = await rpc(client, "tools/list", headers={"Origin": origin})
    assert response.status_code == 403


async def test_no_origin_is_accepted_without_redirect(client):
    response = await rpc(client, "tools/list")
    assert response.status_code == 200
    assert not response.history
    assert "location" not in response.headers


@pytest.mark.parametrize(("host", "status"), [("127.0.0.1:43210", 200), ("localhost:43210", 200), ("testserver", 200), ("evil.example", 421), ("localhost.evil.example:43210", 421), ("localhost", 421)])
async def test_local_host_allowlist(client, host, status):
    response = await rpc(client, "tools/list", headers={"Host": host})
    assert response.status_code == status


@pytest.mark.parametrize(("exception", "message"), [(MarketDataUnavailable("secret database details"), "Market data is temporarily unavailable."), (ValueError("Unsupported slice"), "Unsupported slice"), (RuntimeError("secret request body"), "Internal error")])
async def test_tool_errors_are_sanitized(client, seeded_market, monkeypatch, exception, message):
    async def fail(*args, **kwargs):
        raise exception
    logger = Mock()
    monkeypatch.setattr(mcp_server, "market_slice_snapshot", fail)
    monkeypatch.setattr(mcp_server, "logger", logger)
    result = await call_tool(client, "get_market_slice", SLICE)
    assert message in error_text(result)
    assert "secret" not in json.dumps(result)
    assert "secret" not in str(logger.mock_calls)
    assert mcp_server.tool_limiter.borrowed_tokens == 0


async def test_slice_window_is_normalized(client, seeded_market):
    result = await call_tool(client, "get_market_slice", {**SLICE, "availability_window": " 2026-11 "})
    assert result["structuredContent"]["availability_window"] == "2026-11"
    assert Decimal(result["structuredContent"]["best_ask_usd_per_mt"]) == Decimal("1445")


def test_mcp_disabled_in_production(monkeypatch):
    monkeypatch.setattr(mcp_server.settings, "ENVIRONMENT", "production")
    assert not mcp_server.mcp_enabled("production")
    assert not mcp_server.mcp_enabled("unknown")
    assert all(mcp_server.mcp_enabled(env) for env in ("development", "test", "staging"))


def test_production_app_has_no_mcp_route(monkeypatch):
    monkeypatch.setattr(mcp_server.settings, "ENVIRONMENT", "production")
    namespace = runpy.run_module("app.main", run_name="mcp_production_test")
    assert namespace["mcp"] is None
    assert all(getattr(route, "path", None) != "/mcp" for route in namespace["app"].routes)


async def test_staging_host_allowlist(monkeypatch):
    monkeypatch.setattr(mcp_server.settings, "ENVIRONMENT", "staging")
    server = mcp_server.build_mcp_server()
    app = mcp_server.build_mcp_http_app(server)
    async with server.session_manager.run():
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url="https://api-staging.verdaxis.exchange", headers=HEADERS) as client:
            assert (await rpc(client, "tools/list")).status_code == 200
            for host in ("testserver", "localhost:8001", "api-staging.verdaxis.exchange:443", "evil.example"):
                assert (await rpc(client, "tools/list", headers={"Host": host})).status_code == 421


async def test_sdk_body_limit(client):
    response = await client.post("/mcp", content=b" " * (4 * 1024 * 1024 + 1), headers={"Content-Type": "application/json"})
    assert response.status_code == 413


async def test_capacity_is_released_after_cancellation(monkeypatch):
    limiter = anyio.CapacityLimiter(4)
    monkeypatch.setattr(mcp_server, "tool_limiter", limiter)
    admitted = asyncio.Event()

    async def hold():
        async with mcp_server._tool_call("list_markets"):
            admitted.set()
            await asyncio.Event().wait()

    task = asyncio.create_task(hold())
    await admitted.wait()
    assert limiter.borrowed_tokens == 1
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert limiter.borrowed_tokens == 0


async def test_main_mount_and_lifespan(monkeypatch):
    from app import main
    from app.services.market_event_dispatch import MarketEventDispatcher
    events = []

    async def start(self):
        events.append("start")

    async def stop(self):
        events.append("stop")

    monkeypatch.setattr(MarketEventDispatcher, "start", start)
    monkeypatch.setattr(MarketEventDispatcher, "stop", stop)
    async with main.app.router.lifespan_context(main.app):
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=main.app), base_url="http://testserver", headers=HEADERS) as client:
            response = await rpc(client, "tools/list")
            assert response.status_code == 200
            assert "location" not in response.headers
            assert events == ["start"]
    assert events == ["start", "stop"]

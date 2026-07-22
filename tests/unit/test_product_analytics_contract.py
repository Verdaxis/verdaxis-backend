"""Contract characterization for the Product Analytics workspace (plan Task 1).

Covers three frozen contracts:
1. The scenario fixture is complete and seeds cleanly (lifecycle states,
   every trade status, live/demo/unknown provenance) with internally
   consistent expected values.
2. The existing Umami aggregate contract, characterized through
   ``UmamiAnalyticsService`` with an ``httpx.MockTransport``.
3. The proposed event-data property routes: the canned fixture rows must
   satisfy the same validators the read-only staging smoke enforces against
   the installed Umami 3.2.0.

Service-level metric assertions (EXPECTED vs computed) land in Task 3.
"""

from __future__ import annotations

import importlib.util
import sys
from decimal import Decimal
from pathlib import Path

import httpx
import pytest
from sqlalchemy import func, select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.config import Settings
from app.database import Base
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide, Trade, TradeStatus
from app.models.orders import Commission, CommissionStatus
from app.models.user import User, UserRole, UserStatus
from app.services.behavioral_analytics import UmamiAnalyticsService
from app.services.demo_market import DEMO_MARKET_ORG_IDS
from tests.unit.fixtures import product_analytics as fixtures

_BACKEND_ROOT = Path(__file__).resolve().parents[2]
_SMOKE_PATH = _BACKEND_ROOT / "scripts" / "smoke_umami_product_analytics.py"


def _load_smoke_module():
    name = "smoke_umami_product_analytics"
    if name in sys.modules:
        return sys.modules[name]
    spec = importlib.util.spec_from_file_location(name, _SMOKE_PATH)
    module = importlib.util.module_from_spec(spec)
    # Register before exec so dataclass annotation resolution can find the
    # module through sys.modules.
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


# ---------------------------------------------------------------------------
# Scenario fixture integrity
# ---------------------------------------------------------------------------


def test_scenario_covers_every_lifecycle_state_and_trade_status():
    scenario = fixtures.build_scenario()

    assert {trade.status for trade in scenario.trades} == set(TradeStatus)
    assert {order.status for order in scenario.orders} == set(OrderBookStatus)
    assert {OrderSide.BID, OrderSide.ASK} == {order.side for order in scenario.orders}
    assert {user.status for user in scenario.users} == set(UserStatus)
    assert {user.role for user in scenario.users} == set(UserRole)
    assert {commission.status for commission in scenario.commissions} == set(CommissionStatus)

    assert fixtures.DEMO_ORG_ID in DEMO_MARKET_ORG_IDS
    live_org_ids = {
        fixtures.LIVE_BUYER_ORG_ID,
        fixtures.LIVE_SUPPLIER_ORG_ID,
        fixtures.RETURNING_ORG_ID,
        fixtures.PENDING_ORG_ID,
    }
    assert live_org_ids.isdisjoint(DEMO_MARKET_ORG_IDS)

    # The orderless CONFIRMED trade has no orderbook linkage but still carries
    # a complete immutable snapshot and economic timestamp.
    legacy = next(t for t in scenario.trades if t.id == fixtures.TRADE_IDS["legacy_confirmed"])
    assert legacy.confirmed_at is not None
    assert legacy.bid_order_id is None and legacy.ask_order_id is None
    assert legacy.market_product == "BIO_METHANOL"

    # PAID trade lifecycle is complete even when the separately modeled paid
    # commission intentionally lacks its accounting payment date.
    missing_paid_at = next(
        t for t in scenario.trades if t.id == fixtures.TRADE_IDS["paid_missing_paid_at"]
    )
    assert missing_paid_at.status == TradeStatus.PAID and missing_paid_at.paid_at is not None
    paid_commissions = [
        c for c in scenario.commissions if c.status == CommissionStatus.PAID
    ]
    assert sorted(c.payment_date is None for c in paid_commissions) == [False, True]


async def test_scenario_seeds_into_sqlite_cleanly():
    engine = create_async_engine("sqlite+aiosqlite://", echo=False)
    tables = fixtures.scenario_tables()
    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all, tables=list(tables))
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    try:
        async with factory() as session:
            scenario = await fixtures.seed_product_analytics_scenario(session)

            user_count = await session.scalar(select(func.count(User.id)))
            order_count = await session.scalar(select(func.count(OrderBookOrder.id)))
            trade_count = await session.scalar(select(func.count(Trade.id)))
            commission_count = await session.scalar(select(func.count(Commission.id)))

            assert user_count == len(scenario.users) == 9
            assert order_count == len(scenario.orders) == 11
            assert trade_count == len(scenario.trades) == 9
            assert commission_count == len(scenario.commissions) == 4
    finally:
        await engine.dispose()


def test_expected_metrics_are_internally_consistent():
    expected = fixtures.EXPECTED
    orders = expected["orders"]
    trades = expected["trades"]
    retention = expected["retention"]

    assert orders["live_bids_current"] + orders["live_asks_current"] == orders["live_current"]
    assert orders["execution_rate_numerator"] <= orders["execution_rate_denominator"]
    assert orders["execution_rate_denominator"] == orders["live_current"]

    assert (
        trades["confirmed_current_with_legacy_fallback"]
        == trades["confirmed_current_strict"] + trades["legacy_timestamp_fallback_count"]
    )
    strict_volume = Decimal(trades["confirmed_volume_current_strict_mt"])
    fallback_volume = Decimal(trades["confirmed_volume_current_with_legacy_fallback_mt"])
    assert fallback_volume == strict_volume

    participation_total = (
        retention["repeat_participation_1_day"]
        + retention["repeat_participation_2_days"]
        + retention["repeat_participation_3_plus_days"]
    )
    assert participation_total == orders["participating_organizations_current"]
    assert retention["retained_organizations"] <= orders["participating_organizations_current"]

    liquidity = expected["liquidity"]
    slice_expectations = liquidity["bm_sg_spot"]
    best_bid = Decimal(slice_expectations["best_bid_usd_per_mt"])
    best_ask = Decimal(slice_expectations["best_ask_usd_per_mt"])
    assert best_ask - best_bid == Decimal(slice_expectations["spread_usd_per_mt"])
    assert (best_ask + best_bid) / 2 == Decimal(slice_expectations["mid_usd_per_mt"])
    assert Decimal(slice_expectations["one_percent_bid_depth_mt"]) >= Decimal(
        slice_expectations["best_bid_depth_mt"]
    )

    behavioral = expected["behavioral"]
    stats = fixtures.UMAMI_STATS_PAYLOAD
    assert behavioral["average_session_duration_seconds"] == pytest.approx(
        stats["totaltime"] / stats["visits"]
    )


# ---------------------------------------------------------------------------
# Existing Umami aggregate contract (MockTransport characterization)
# ---------------------------------------------------------------------------


def _settings(**overrides) -> Settings:
    values = {
        "DATABASE_URL": "sqlite+aiosqlite:///:memory:",
        "DATABASE_PASSWORD": "test-password",
        "JWT_SECRET": "test-secret-key-for-testing-minimum-32-chars",
        "ANALYTICS_ENABLED": True,
        "UMAMI_BASE_URL": "https://analytics.example.com/",
        "UMAMI_WEBSITE_ID": fixtures.UMAMI_WEBSITE_ID,
        "UMAMI_API_USERNAME": "view-only-user",
        "UMAMI_API_PASSWORD": "view-only-password",
        "ANALYTICS_REQUEST_TIMEOUT_SECONDS": 1.0,
    }
    values.update(overrides)
    return Settings(**values)


def _umami_handler(request: httpx.Request) -> httpx.Response:
    path = request.url.path
    if path == "/api/auth/login":
        return httpx.Response(200, json={"token": "characterization-token"})
    assert path.startswith(f"/api/websites/{fixtures.UMAMI_WEBSITE_ID}/")
    assert "startAt" in request.url.params and "endAt" in request.url.params
    if path.endswith("/stats"):
        return httpx.Response(200, json=fixtures.UMAMI_STATS_PAYLOAD)
    if path.endswith("/pageviews"):
        assert request.url.params["unit"] == "day"
        assert request.url.params["timezone"] == "UTC"
        return httpx.Response(200, json=fixtures.UMAMI_PAGEVIEWS_PAYLOAD)
    if path.endswith("/events/series"):
        return httpx.Response(200, json=fixtures.UMAMI_EVENT_SERIES_PAYLOAD)
    if path.endswith("/metrics"):
        metric_type = request.url.params["type"]
        assert metric_type in {"entry", "referrer"}
        payload = (
            fixtures.UMAMI_ENTRY_METRICS_PAYLOAD
            if metric_type == "entry"
            else fixtures.UMAMI_REFERRER_METRICS_PAYLOAD
        )
        return httpx.Response(200, json=payload)
    raise AssertionError(f"unexpected Umami path: {path}")


async def test_existing_aggregate_contract_characterization():
    service = UmamiAnalyticsService(
        _settings(), transport=httpx.MockTransport(_umami_handler)
    )

    aggregate = await service.get_aggregate(30)

    assert aggregate.status == "available"
    assert aggregate.visitors == 120
    assert aggregate.visits == 90
    assert aggregate.pageviews == 300
    assert aggregate.total_time_seconds == 1800
    # Unknown event names are dropped from both totals and series.
    assert aggregate.event_totals == fixtures.EXPECTED_EVENT_TOTALS
    assert {point["event"] for point in aggregate.event_series} == set(
        fixtures.EXPECTED_EVENT_TOTALS
    )
    # Daily visitors come from the sessions series, bounded and date-keyed.
    assert aggregate.daily_visitors == [
        {"date": "2026-06-10", "value": 30},
        {"date": "2026-06-11", "value": 25},
    ]
    assert aggregate.top_entries[0] == {"name": "/signup", "value": 40}
    # Empty referrers survive as empty names: the UI renders Direct / unknown,
    # distinct from having no referrer data at all.
    assert {"name": "", "value": 10} in aggregate.top_referrers
    # Session duration definition frozen in §1.5: totaltime / visits.
    assert aggregate.total_time_seconds / aggregate.visits == pytest.approx(
        fixtures.EXPECTED["behavioral"]["average_session_duration_seconds"]
    )


# ---------------------------------------------------------------------------
# Proposed event-data routes share validators with the staging smoke
# ---------------------------------------------------------------------------


def test_event_data_fixtures_satisfy_smoke_validators():
    smoke = _load_smoke_module()
    contract = fixtures.UMAMI_EVENT_DATA_CONTRACT

    events_summary = smoke.validate_event_data_events(contract["events"])
    properties_summary = smoke.validate_event_data_properties(contract["properties"])
    values_summary = smoke.validate_event_data_values(contract["values"])

    assert events_summary["row_count"] == len(contract["events"])
    assert properties_summary["row_count"] == len(contract["properties"])
    assert values_summary["row_count"] == len(contract["values"])
    assert events_summary["types"]["total"] == "int"
    assert events_summary["types"]["propertyValue"] == "str"

    # The pivot route stays outside the implementation contract but its
    # envelope shape is still frozen for drift detection.
    envelope_summary = smoke.validate_event_data_pivot_envelope(
        {"count": 1, "data": [{"eventName": "signup_started"}], "isCapped": False,
         "page": 1, "pageSize": 20}
    )
    assert envelope_summary["row_count"] == 1


def test_smoke_validators_reject_malformed_shapes():
    smoke = _load_smoke_module()

    with pytest.raises(smoke.ContractViolation):
        smoke.validate_event_data_events({"not": "a list"})
    with pytest.raises(smoke.ContractViolation):
        smoke.validate_event_data_events([{"propertyName": "x", "total": 1}])
    with pytest.raises(smoke.ContractViolation):
        smoke.validate_event_data_values([{"value": {"nested": True}, "total": 1}])
    with pytest.raises(smoke.ContractViolation):
        smoke.validate_event_data_properties(
            [{"propertyName": "x", "total": 1, "surprise": "key"}]
        )
    # Filtered events rows must carry the per-value breakdown.
    with pytest.raises(smoke.ContractViolation):
        smoke.validate_event_data_events(
            [{"eventName": "x", "propertyName": "y", "total": 1}]
        )
    with pytest.raises(smoke.ContractViolation):
        smoke.validate_event_data_pivot_envelope({"data": [{"x": 1}]})


def test_smoke_refuses_non_loopback_urls_without_explicit_flag():
    smoke = _load_smoke_module()

    smoke.ensure_permitted_base_url("http://127.0.0.1:8700", allow_remote_readonly=False)
    smoke.ensure_permitted_base_url("http://localhost:8700", allow_remote_readonly=False)
    smoke.ensure_permitted_base_url(
        "https://analytics.example.com", allow_remote_readonly=True
    )

    with pytest.raises(smoke.ContractViolation):
        smoke.ensure_permitted_base_url(
            "https://analytics.example.com", allow_remote_readonly=False
        )
    with pytest.raises(smoke.ContractViolation):
        smoke.ensure_permitted_base_url(
            "http://user:secret@127.0.0.1:8700", allow_remote_readonly=False
        )

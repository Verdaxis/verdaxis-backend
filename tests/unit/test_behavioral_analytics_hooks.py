from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock
from uuid import uuid4

from app.services.behavioral_analytics import (
    analytics_service,
    organization_created_event,
    order_created_event,
    registration_completed_event,
    trade_created_event,
    track_analytics_event,
)


ROOT = Path(__file__).resolve().parents[2]


def test_conversion_event_builders_expose_only_contract_fields():
    user = SimpleNamespace(
        id=uuid4(),
        role=SimpleNamespace(value="BUYER"),
        email="private@example.test",
        first_name="Private",
    )
    order = SimpleNamespace(
        side=SimpleNamespace(value="BID"),
        market_product="BIO_METHANOL",
        delivery_point_name="Singapore",
        availability_window="SPOT",
        price_per_mt_usd="999",
        quantity_mt="5000",
        id=uuid4(),
    )

    events = [
        registration_completed_event(user),
        organization_created_event(user),
        order_created_event(user, order),
        trade_created_event(user, order=order),
    ]

    assert [event.name for event in events] == [
        "registration_completed",
        "organization_created",
        "order_created",
        "trade_created",
    ]
    for event in events:
        assert set(event.data()) <= {
            "role", "side", "canonical_product",
            "delivery_point", "availability_window",
        }
        serialized = repr(event.data()).lower()
        for forbidden in ["private@example", "price", "quantity", "order_id", "trade_id"]:
            assert forbidden not in serialized

    resting_ask = SimpleNamespace(
        side=SimpleNamespace(value="ASK"),
        market_product="BIO_METHANOL",
        delivery_point_name="Singapore",
        availability_window="SPOT",
    )
    assert trade_created_event(user, order=resting_ask).side == "BID"


def test_request_metadata_is_bounded_and_not_part_of_event_data():
    request = SimpleNamespace(
        headers={"user-agent": "Mozilla/5.0 " + "x" * 500},
        url=SimpleNamespace(hostname="api-staging.verdaxis.exchange"),
    )
    user = SimpleNamespace(id=uuid4(), role=SimpleNamespace(value="SUPPLIER"))

    event = registration_completed_event(user, request=request)

    assert event.hostname == "api-staging.verdaxis.exchange"
    assert len(event.user_agent) <= 256
    assert "user_agent" not in event.data()
    assert "hostname" not in event.data()


def test_authenticated_monitor_requests_are_not_tracked(monkeypatch):
    request = SimpleNamespace(headers={"x-monitor-token": "monitor-secret"})
    user = SimpleNamespace(id=uuid4(), role=SimpleNamespace(value="BUYER"))
    schedule = Mock()
    monkeypatch.setattr(analytics_service, "schedule_event", schedule)
    monkeypatch.setattr("app.services.behavioral_analytics.settings.MONITOR_TOKEN", "monitor-secret")

    track_analytics_event(registration_completed_event(user), request=request)

    schedule.assert_not_called()

    request.headers["x-monitor-token"] = "wrong-token"
    track_analytics_event(registration_completed_event(user), request=request)
    schedule.assert_called_once()


def test_registration_and_organization_events_are_wired_after_commits():
    source = (ROOT / "app/routers/auth_simple.py").read_text()
    domain_commit = source.index("await db.commit()", source.index("async def register("))
    domain_event = source.index("registration_completed_event", domain_commit)
    domain_return = source.index('return RegistrationResponse(status="created"', domain_commit)
    org_commit = source.index("await db.commit()", source.index("async def register_with_org("))
    org_event = source.index("organization_created_event", org_commit)
    org_registration_event = source.index("registration_completed_event", org_commit)

    assert domain_commit < domain_event < domain_return
    assert org_commit < org_event < org_registration_event


def test_order_and_all_trade_creation_paths_are_wired_after_commits():
    expectations = {
        "app/routers/orderbook.py": ("async def create_order(", "order_created_event"),
        "app/routers/trades.py": ("async def create_trade(", "trade_created_event"),
    }
    for path, (function_marker, event_marker) in expectations.items():
        source = (ROOT / path).read_text()
        function_start = source.index(function_marker)
        commit = source.index("await db.commit()", function_start)
        event = source.index(event_marker, commit)
        assert commit < event, path
        assert "request=" in source[event:source.index(")", event) + 1], path

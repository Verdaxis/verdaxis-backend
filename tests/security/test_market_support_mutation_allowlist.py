from uuid import uuid4

from app import main
from app.services.request_party import is_market_support_mutation_allowed


def test_every_unclassified_context_bearing_mutation_fails_closed():
    context_id = uuid4()
    candidates = [
        ("POST", "/api/rfq"),
        ("POST", "/api/inventory"),
        ("PATCH", f"/api/watchlists/{uuid4()}"),
        ("PUT", "/api/preferences/notifications"),
        ("POST", "/api/compliance/scenario"),
        ("DELETE", f"/api/orderbook/{uuid4()}"),
    ]

    assert all(
        not is_market_support_mutation_allowed(method, path, context_id=context_id)
        for method, path in candidates
    )


def test_known_state_changing_gets_are_explicitly_denied():
    for path in (
        "/api/auth/verify-email",
        "/api/referrals/my-code",
        "/api/watchlists/me",
    ):
        assert not is_market_support_mutation_allowed(
            "GET", path, context_id=uuid4()
        )


def test_route_inventory_requires_every_mutation_to_be_classified():
    context_id = uuid4()
    for route in main.app.routes:
        for method in getattr(route, "methods", set()):
            if method in {"POST", "PUT", "PATCH", "DELETE"}:
                concrete_path = (
                    route.path.replace("{order_id}", str(uuid4()))
                    .replace("{context_id}", str(context_id))
                    .replace("{organization_id}", str(uuid4()))
                    .replace("{id}", str(uuid4()))
                )
                assert is_market_support_mutation_allowed(
                    method, concrete_path, context_id=context_id
                ) is (
                    method == "POST"
                    and (
                        concrete_path == "/api/orderbook"
                        or concrete_path.startswith("/api/orderbook/")
                        and concrete_path.endswith("/cancel")
                        or "/contexts/" in concrete_path and concrete_path.endswith("/exit")
                    )
                ), f"unclassified context mutation: {method} {route.path}"

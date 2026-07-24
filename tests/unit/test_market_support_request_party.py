from uuid import uuid4

from app.models.orderbook import OrderCreationMethod
from app.services.request_party import (
    MARKET_SUPPORT_CONTEXT_HEADER,
    RequestPartyMode,
    is_market_support_mutation_allowed,
)


def test_context_header_is_the_only_supported_locator():
    assert MARKET_SUPPORT_CONTEXT_HEADER == "X-Verdaxis-Market-Support-Context"
    assert RequestPartyMode.MARKET_SUPPORT.value == "MARKET_SUPPORT"


def test_context_mutation_allowlist_is_deny_by_default():
    context_id = uuid4()
    assert is_market_support_mutation_allowed(
        "POST", "/api/orderbook", context_id=context_id
    )
    assert is_market_support_mutation_allowed(
        "POST", f"/api/orderbook/{uuid4()}/cancel", context_id=context_id
    )
    assert not is_market_support_mutation_allowed(
        "PUT", f"/api/orderbook/{uuid4()}", context_id=context_id
    )
    assert not is_market_support_mutation_allowed(
        "POST", "/api/trades/", context_id=context_id
    )
    assert not is_market_support_mutation_allowed(
        "PATCH", "/api/notifications/read-all", context_id=context_id
    )


def test_context_reads_and_lifecycle_exit_are_allowlisted():
    context_id = uuid4()
    assert is_market_support_mutation_allowed(
        "GET", "/api/orderbook/my", context_id=context_id
    )
    assert is_market_support_mutation_allowed(
        "GET", "/api/orderbook/my/latest-ask-template", context_id=context_id
    )
    assert is_market_support_mutation_allowed(
        "POST",
        f"/api/admin/market-support/contexts/{context_id}/exit",
        context_id=context_id,
    )
    assert OrderCreationMethod.MARKET_SUPPORT.value == "MARKET_SUPPORT"

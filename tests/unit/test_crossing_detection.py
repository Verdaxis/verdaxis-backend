"""Tests for crossing detection in orderbook responses."""
from decimal import Decimal


def test_bid_crosses_when_price_gte_best_ask():
    from app.routers.orderbook import compute_is_crossed
    assert compute_is_crossed("BID", Decimal("535"), Decimal("530")) is True


def test_bid_does_not_cross_when_price_lt_best_ask():
    from app.routers.orderbook import compute_is_crossed
    assert compute_is_crossed("BID", Decimal("525"), Decimal("530")) is False


def test_ask_crosses_when_price_lte_best_bid():
    from app.routers.orderbook import compute_is_crossed
    assert compute_is_crossed("ASK", Decimal("515"), Decimal("520")) is True


def test_ask_does_not_cross_when_no_opposing():
    from app.routers.orderbook import compute_is_crossed
    assert compute_is_crossed("ASK", Decimal("515"), None) is False


def test_bid_does_not_cross_when_no_opposing():
    from app.routers.orderbook import compute_is_crossed
    assert compute_is_crossed("BID", Decimal("535"), None) is False


def test_bid_at_exact_ask_price_is_crossed():
    from app.routers.orderbook import compute_is_crossed
    assert compute_is_crossed("BID", Decimal("530"), Decimal("530")) is True


def test_ask_at_exact_bid_price_is_crossed():
    from app.routers.orderbook import compute_is_crossed
    assert compute_is_crossed("ASK", Decimal("520"), Decimal("520")) is True

"""Unit tests for matchmaking scoring logic."""
from decimal import Decimal

from app.services.matchmaking import compute_match_score


class TestMatchScoring:
    def test_perfect_match(self):
        """Same fuel, same region, overlapping price, same availability."""
        score, reasons = compute_match_score(
            bid_fuel="Methanol", ask_fuel="Methanol",
            bid_region="Singapore", ask_region="Singapore",
            bid_price=Decimal("550"), ask_price=Decimal("540"),
            bid_qty=Decimal("1000"), ask_qty=Decimal("1000"),
            bid_availability_window="2026-Q3",
            ask_availability_window="2026-Q3",
        )
        assert score >= Decimal("80")
        assert "fuel_type_match" in reasons
        assert "region_match" in reasons
        assert "price_overlap" in reasons
        assert "availability_match" in reasons

    def test_fuel_mismatch_scores_zero(self):
        """Different fuels should score 0."""
        score, reasons = compute_match_score(
            bid_fuel="Methanol", ask_fuel="LNG",
            bid_region="Singapore", ask_region="Singapore",
            bid_price=Decimal("550"), ask_price=Decimal("540"),
            bid_qty=Decimal("1000"), ask_qty=Decimal("1000"),
        )
        assert score == Decimal("0")
        assert reasons == []

    def test_price_no_overlap(self):
        """Bid price < ask price should reduce score but not zero if fuel+region match."""
        score, reasons = compute_match_score(
            bid_fuel="Methanol", ask_fuel="Methanol",
            bid_region="Singapore", ask_region="Singapore",
            bid_price=Decimal("500"), ask_price=Decimal("550"),
            bid_qty=Decimal("1000"), ask_qty=Decimal("1000"),
        )
        assert score <= Decimal("80")
        assert "fuel_type_match" in reasons
        assert "price_overlap" not in reasons

    def test_partial_region_match(self):
        """ARA vs Rotterdam should count as partial match."""
        score, reasons = compute_match_score(
            bid_fuel="Methanol", ask_fuel="Methanol",
            bid_region="ARA", ask_region="Rotterdam",
            bid_price=Decimal("550"), ask_price=Decimal("540"),
            bid_qty=Decimal("1000"), ask_qty=Decimal("1000"),
        )
        assert score > Decimal("0")

    def test_different_availability_window_loses_window_credit(self):
        score, reasons = compute_match_score(
            bid_fuel="Methanol", ask_fuel="Methanol",
            bid_region="Singapore", ask_region="Singapore",
            bid_price=Decimal("550"), ask_price=Decimal("540"),
            bid_qty=Decimal("1000"), ask_qty=Decimal("1000"),
            bid_availability_window="2026-Q3",
            ask_availability_window="2026-Q4",
        )
        assert "availability_match" not in reasons

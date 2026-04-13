"""Unit tests for benchmark-aligned matchmaking scoring."""
from decimal import Decimal

from app.services.matchmaking import compute_match_score


class TestMatchScoring:
    def test_market_identity_match_scores_high_with_complete_docs(self):
        score, reasons = compute_match_score(
            target_market_product="BIO_METHANOL",
            candidate_market_product="BIO_METHANOL",
            target_delivery_point_id="sg",
            candidate_delivery_point_id="sg",
            target_price=Decimal("550"),
            candidate_price=Decimal("540"),
            target_qty=Decimal("1000"),
            candidate_qty=Decimal("1000"),
            target_availability_window="2026-Q3",
            candidate_availability_window="2026-Q3",
            candidate_certification_declared=True,
            candidate_certification_scheme="ISCC EU",
            candidate_specification_standard="IMPCA",
            candidate_msds_available=True,
        )
        assert score >= Decimal("85")
        assert "market_product_match" in reasons
        assert "delivery_point_match" in reasons
        assert "availability_match" in reasons
        assert "price_overlap" in reasons
        assert "documentation_complete" in reasons

    def test_delivery_point_mismatch_scores_zero(self):
        score, reasons = compute_match_score(
            target_market_product="BIO_METHANOL",
            candidate_market_product="BIO_METHANOL",
            target_delivery_point_id="sg",
            candidate_delivery_point_id="ara",
            target_price=Decimal("550"),
            candidate_price=Decimal("540"),
            target_qty=Decimal("1000"),
            candidate_qty=Decimal("1000"),
        )
        assert score == Decimal("0")
        assert reasons == []

    def test_off_spec_listing_is_excluded_by_default(self):
        score, reasons = compute_match_score(
            target_market_product="BIO_METHANOL",
            candidate_market_product="BIO_METHANOL",
            target_delivery_point_id="sg",
            candidate_delivery_point_id="sg",
            target_price=Decimal("550"),
            candidate_price=Decimal("540"),
            target_qty=Decimal("1000"),
            candidate_qty=Decimal("1000"),
            candidate_off_spec=True,
        )
        assert score == Decimal("0")
        assert reasons == []

    def test_metadata_does_not_fragment_market_key_when_docs_are_incomplete(self):
        score, reasons = compute_match_score(
            target_market_product="BIO_METHANOL",
            candidate_market_product="BIO_METHANOL",
            target_delivery_point_id="sg",
            candidate_delivery_point_id="sg",
            target_price=Decimal("550"),
            candidate_price=Decimal("540"),
            target_qty=Decimal("1000"),
            candidate_qty=Decimal("800"),
            target_availability_window="SPOT",
            candidate_availability_window="SPOT",
            candidate_certification_declared=True,
            candidate_certification_scheme=None,
            candidate_specification_standard=None,
            candidate_msds_available=False,
        )
        assert score > Decimal("0")
        assert "market_product_match" in reasons
        assert "documentation_complete" not in reasons

    def test_certification_scheme_mismatch_scores_zero_when_target_requires_scheme(self):
        score, reasons = compute_match_score(
            target_market_product="BIO_METHANOL",
            candidate_market_product="BIO_METHANOL",
            target_delivery_point_id="sg",
            candidate_delivery_point_id="sg",
            target_price=Decimal("550"),
            candidate_price=Decimal("540"),
            target_qty=Decimal("1000"),
            candidate_qty=Decimal("1000"),
            target_availability_window="SPOT",
            candidate_availability_window="SPOT",
            target_certification_scheme="ISCC EU",
            candidate_certification_declared=True,
            candidate_certification_scheme="RSB",
            candidate_specification_standard="IMPCA",
            candidate_msds_available=True,
        )
        assert score == Decimal("0")
        assert reasons == []


    def test_bid_candidate_with_matching_scheme_does_not_require_supplier_declaration(self):
        score, reasons = compute_match_score(
            target_market_product="BIO_METHANOL",
            candidate_market_product="BIO_METHANOL",
            target_delivery_point_id="sg",
            candidate_delivery_point_id="sg",
            target_price=Decimal("550"),
            candidate_price=Decimal("540"),
            target_qty=Decimal("1000"),
            candidate_qty=Decimal("1000"),
            target_availability_window="SPOT",
            candidate_availability_window="SPOT",
            target_certification_scheme="ISCC EU",
            candidate_side="BID",
            candidate_certification_declared=False,
            candidate_certification_scheme="ISCC EU",
        )
        assert score > Decimal("0")
        assert "market_product_match" in reasons


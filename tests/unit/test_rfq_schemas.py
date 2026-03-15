"""
Unit tests for RFQ Pydantic schemas.

Tests validation rules, default values, and serialization without
requiring a running database.
"""
import pytest
from decimal import Decimal
from uuid import uuid4
from datetime import datetime, UTC

from app.schemas.rfq import (
    RFQCreateRequest,
    RFQQuoteRequest,
    RFQQuoteResponse,
    RFQResponse,
    RFQListResponse,
)


class TestRFQCreateRequest:
    def test_valid_minimal(self):
        req = RFQCreateRequest(
            product_id=uuid4(),
            quantity_mt=Decimal("500"),
        )
        assert req.delivery_point_id is None
        assert req.target_price_per_mt is None
        assert req.availability_window == "Spot"
        assert req.notes is None
        assert req.is_anonymous is False
        assert req.expires_in_hours == 24

    def test_valid_all_fields(self):
        pid = uuid4()
        dpid = uuid4()
        req = RFQCreateRequest(
            product_id=pid,
            delivery_point_id=dpid,
            quantity_mt=Decimal("10000"),
            target_price_per_mt=Decimal("550.00"),
            availability_window="Q1 2025",
            notes="Need ISCC certified",
            is_anonymous=True,
            expires_in_hours=72,
        )
        assert req.product_id == pid
        assert req.delivery_point_id == dpid
        assert req.quantity_mt == Decimal("10000")
        assert req.target_price_per_mt == Decimal("550.00")
        assert req.is_anonymous is True
        assert req.expires_in_hours == 72

    def test_quantity_must_be_positive(self):
        with pytest.raises(Exception):
            RFQCreateRequest(
                product_id=uuid4(),
                quantity_mt=Decimal("0"),
            )

    def test_quantity_must_be_nonzero(self):
        with pytest.raises(Exception):
            RFQCreateRequest(
                product_id=uuid4(),
                quantity_mt=Decimal("-100"),
            )

    def test_quantity_max_100000(self):
        with pytest.raises(Exception):
            RFQCreateRequest(
                product_id=uuid4(),
                quantity_mt=Decimal("100001"),
            )

    def test_quantity_at_max_boundary(self):
        req = RFQCreateRequest(
            product_id=uuid4(),
            quantity_mt=Decimal("100000"),
        )
        assert req.quantity_mt == Decimal("100000")

    def test_target_price_must_be_positive(self):
        with pytest.raises(Exception):
            RFQCreateRequest(
                product_id=uuid4(),
                quantity_mt=Decimal("500"),
                target_price_per_mt=Decimal("0"),
            )

    def test_target_price_negative_rejected(self):
        with pytest.raises(Exception):
            RFQCreateRequest(
                product_id=uuid4(),
                quantity_mt=Decimal("500"),
                target_price_per_mt=Decimal("-10"),
            )

    def test_notes_max_length(self):
        with pytest.raises(Exception):
            RFQCreateRequest(
                product_id=uuid4(),
                quantity_mt=Decimal("500"),
                notes="x" * 501,
            )

    def test_notes_at_max_length(self):
        req = RFQCreateRequest(
            product_id=uuid4(),
            quantity_mt=Decimal("500"),
            notes="x" * 500,
        )
        assert len(req.notes) == 500

    def test_expires_in_hours_min(self):
        with pytest.raises(Exception):
            RFQCreateRequest(
                product_id=uuid4(),
                quantity_mt=Decimal("500"),
                expires_in_hours=0,
            )

    def test_expires_in_hours_max(self):
        with pytest.raises(Exception):
            RFQCreateRequest(
                product_id=uuid4(),
                quantity_mt=Decimal("500"),
                expires_in_hours=169,
            )

    def test_expires_in_hours_at_boundaries(self):
        req1 = RFQCreateRequest(
            product_id=uuid4(),
            quantity_mt=Decimal("500"),
            expires_in_hours=1,
        )
        assert req1.expires_in_hours == 1

        req168 = RFQCreateRequest(
            product_id=uuid4(),
            quantity_mt=Decimal("500"),
            expires_in_hours=168,
        )
        assert req168.expires_in_hours == 168


class TestRFQQuoteRequest:
    def test_valid_quote(self):
        req = RFQQuoteRequest(
            price_per_mt_usd=Decimal("520.50"),
            notes="Can deliver in 3 days",
        )
        assert req.price_per_mt_usd == Decimal("520.50")
        assert req.notes == "Can deliver in 3 days"

    def test_valid_quote_no_notes(self):
        req = RFQQuoteRequest(price_per_mt_usd=Decimal("500"))
        assert req.notes is None

    def test_price_must_be_positive(self):
        with pytest.raises(Exception):
            RFQQuoteRequest(price_per_mt_usd=Decimal("0"))

    def test_price_negative_rejected(self):
        with pytest.raises(Exception):
            RFQQuoteRequest(price_per_mt_usd=Decimal("-100"))

    def test_notes_max_length(self):
        with pytest.raises(Exception):
            RFQQuoteRequest(
                price_per_mt_usd=Decimal("500"),
                notes="x" * 501,
            )


class TestRFQQuoteResponse:
    def test_from_dict(self):
        qid = uuid4()
        org_id = uuid4()
        now = datetime.now(UTC)
        resp = RFQQuoteResponse(
            id=qid,
            seller_org_id=org_id,
            seller_org_name="Acme Fuels",
            price_per_mt_usd=Decimal("530.00"),
            notes="Premium grade",
            status="PENDING",
            created_at=now,
        )
        assert resp.id == qid
        assert resp.seller_org_name == "Acme Fuels"
        assert resp.status == "PENDING"

    def test_optional_fields(self):
        resp = RFQQuoteResponse(
            id=uuid4(),
            seller_org_id=uuid4(),
            price_per_mt_usd=Decimal("500"),
            status="PENDING",
            created_at=datetime.now(UTC),
        )
        assert resp.seller_org_name is None
        assert resp.notes is None


class TestRFQResponse:
    def test_full_response(self):
        rfq_id = uuid4()
        org_id = uuid4()
        pid = uuid4()
        now = datetime.now(UTC)

        resp = RFQResponse(
            id=rfq_id,
            buyer_org_id=org_id,
            buyer_org_name="Maritime Corp",
            product_id=pid,
            product_name="VLSFO 0.5%",
            quantity_mt=Decimal("2000"),
            availability_window="Spot",
            is_anonymous=False,
            status="OPEN",
            expires_at=now,
            created_at=now,
        )
        assert resp.quote_count == 0
        assert resp.quotes == []
        assert resp.delivery_point_id is None
        assert resp.delivery_point_name is None
        assert resp.target_price_per_mt is None
        assert resp.notes is None

    def test_with_quotes(self):
        now = datetime.now(UTC)
        quote = RFQQuoteResponse(
            id=uuid4(),
            seller_org_id=uuid4(),
            seller_org_name="Seller Inc",
            price_per_mt_usd=Decimal("510"),
            status="PENDING",
            created_at=now,
        )
        resp = RFQResponse(
            id=uuid4(),
            buyer_org_id=uuid4(),
            product_id=uuid4(),
            quantity_mt=Decimal("1000"),
            availability_window="Spot",
            is_anonymous=False,
            status="QUOTED",
            expires_at=now,
            created_at=now,
            quote_count=1,
            quotes=[quote],
        )
        assert len(resp.quotes) == 1
        assert resp.quote_count == 1


class TestRFQListResponse:
    def test_empty_list(self):
        resp = RFQListResponse(items=[], total=0)
        assert resp.items == []
        assert resp.total == 0

    def test_with_items(self):
        now = datetime.now(UTC)
        item = RFQResponse(
            id=uuid4(),
            buyer_org_id=uuid4(),
            product_id=uuid4(),
            quantity_mt=Decimal("500"),
            availability_window="Spot",
            is_anonymous=False,
            status="OPEN",
            expires_at=now,
            created_at=now,
        )
        resp = RFQListResponse(items=[item], total=1)
        assert len(resp.items) == 1
        assert resp.total == 1

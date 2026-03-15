"""Unit tests for RFQ model."""
import pytest
from app.models.rfq import RFQ, RFQQuote, RFQStatus, QuoteStatus


class TestRFQStatus:
    def test_status_values(self):
        assert RFQStatus.OPEN == "OPEN"
        assert RFQStatus.QUOTED == "QUOTED"
        assert RFQStatus.ACCEPTED == "ACCEPTED"
        assert RFQStatus.EXPIRED == "EXPIRED"
        assert RFQStatus.CANCELLED == "CANCELLED"


class TestQuoteStatus:
    def test_status_values(self):
        assert QuoteStatus.PENDING == "PENDING"
        assert QuoteStatus.ACCEPTED == "ACCEPTED"
        assert QuoteStatus.DECLINED == "DECLINED"
        assert QuoteStatus.WITHDRAWN == "WITHDRAWN"


class TestRFQModel:
    def test_rfq_has_required_columns(self):
        from sqlalchemy import inspect
        mapper = inspect(RFQ)
        columns = {c.key for c in mapper.column_attrs}
        expected = {
            "id", "buyer_org_id", "product_id", "delivery_point_id",
            "quantity_mt", "target_price_per_mt",
            "availability_window", "notes",
            "status", "expires_at", "created_at",
        }
        assert expected.issubset(columns)


class TestRFQQuoteModel:
    def test_quote_has_required_columns(self):
        from sqlalchemy import inspect
        mapper = inspect(RFQQuote)
        columns = {c.key for c in mapper.column_attrs}
        expected = {
            "id", "rfq_id", "seller_org_id",
            "price_per_mt_usd", "notes",
            "status", "created_at",
        }
        assert expected.issubset(columns)

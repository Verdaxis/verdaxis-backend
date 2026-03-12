"""
Unit tests for the CSV export endpoint on the reference price API.
Tests mock compute_reference_prices to avoid live DB dependency.
"""
import csv
import io
import pytest
from unittest.mock import AsyncMock, patch
from datetime import date
from decimal import Decimal
from uuid import uuid4

from httpx import AsyncClient, ASGITransport
from app.main import app
from app.schemas.orderbook import ReferencePriceItem


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _make_item(
    fuel_type="Methanol",
    region="Asia",
    product_name="Methanol Green",
    delivery_point_name="Singapore",
    vwap_usd="533.33",
    total_volume_mt="300.00",
    trade_count=2,
    trade_date=date(2026, 2, 15),
) -> ReferencePriceItem:
    return ReferencePriceItem(
        product_id=uuid4(),
        product_name=product_name,
        fuel_type=fuel_type,
        delivery_point_id=uuid4(),
        delivery_point_name=delivery_point_name,
        region=region,
        vwap_usd=Decimal(vwap_usd),
        total_volume_mt=Decimal(total_volume_mt),
        trade_count=trade_count,
        date=trade_date,
    )


EXPECTED_HEADERS = ["date", "product_name", "fuel_type", "delivery_point_name", "region",
                    "vwap_usd", "volume_mt", "trade_count"]


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

class TestReferencePriceExport:
    """Tests for GET /api/prices/reference/export."""

    @pytest.mark.asyncio
    async def test_returns_200_with_text_csv_content_type(self):
        async with AsyncClient(
            transport=ASGITransport(app=app), base_url="http://test"
        ) as client:
            with patch(
                "app.routers.price_discovery.compute_reference_prices",
                new_callable=AsyncMock,
                return_value=[],
            ):
                resp = await client.get("/api/prices/reference/export")

        assert resp.status_code == 200
        assert "text/csv" in resp.headers["content-type"]

    @pytest.mark.asyncio
    async def test_has_content_disposition_attachment_header(self):
        async with AsyncClient(
            transport=ASGITransport(app=app), base_url="http://test"
        ) as client:
            with patch(
                "app.routers.price_discovery.compute_reference_prices",
                new_callable=AsyncMock,
                return_value=[],
            ):
                resp = await client.get("/api/prices/reference/export")

        assert resp.status_code == 200
        disposition = resp.headers.get("content-disposition", "")
        assert "attachment" in disposition
        assert ".csv" in disposition

    @pytest.mark.asyncio
    async def test_empty_data_returns_headers_only(self):
        async with AsyncClient(
            transport=ASGITransport(app=app), base_url="http://test"
        ) as client:
            with patch(
                "app.routers.price_discovery.compute_reference_prices",
                new_callable=AsyncMock,
                return_value=[],
            ):
                resp = await client.get("/api/prices/reference/export")

        assert resp.status_code == 200
        reader = csv.DictReader(io.StringIO(resp.text))
        rows = list(reader)
        assert rows == []
        assert reader.fieldnames == EXPECTED_HEADERS

    @pytest.mark.asyncio
    async def test_csv_has_correct_column_headers(self):
        async with AsyncClient(
            transport=ASGITransport(app=app), base_url="http://test"
        ) as client:
            with patch(
                "app.routers.price_discovery.compute_reference_prices",
                new_callable=AsyncMock,
                return_value=[_make_item()],
            ):
                resp = await client.get("/api/prices/reference/export")

        reader = csv.DictReader(io.StringIO(resp.text))
        assert reader.fieldnames == EXPECTED_HEADERS

    @pytest.mark.asyncio
    async def test_csv_row_values_match_item(self):
        item = _make_item(
            fuel_type="Ammonia",
            region="Europe",
            product_name="Ammonia Green",
            delivery_point_name="ARA",
            vwap_usd="750.00",
            total_volume_mt="100.00",
            trade_count=1,
            trade_date=date(2026, 3, 1),
        )
        async with AsyncClient(
            transport=ASGITransport(app=app), base_url="http://test"
        ) as client:
            with patch(
                "app.routers.price_discovery.compute_reference_prices",
                new_callable=AsyncMock,
                return_value=[item],
            ):
                resp = await client.get("/api/prices/reference/export")

        rows = list(csv.DictReader(io.StringIO(resp.text)))
        assert len(rows) == 1
        row = rows[0]
        assert row["date"] == "2026-03-01"
        assert row["product_name"] == "Ammonia Green"
        assert row["fuel_type"] == "Ammonia"
        assert row["delivery_point_name"] == "ARA"
        assert row["region"] == "Europe"
        assert row["vwap_usd"] == "750.00"
        assert row["volume_mt"] == "100.00"
        assert row["trade_count"] == "1"

    @pytest.mark.asyncio
    async def test_multiple_rows_exported(self):
        items = [
            _make_item(fuel_type="Methanol", trade_date=date(2026, 2, 14)),
            _make_item(fuel_type="Ammonia", trade_date=date(2026, 2, 15)),
            _make_item(fuel_type="LNG", trade_date=date(2026, 2, 16)),
        ]
        async with AsyncClient(
            transport=ASGITransport(app=app), base_url="http://test"
        ) as client:
            with patch(
                "app.routers.price_discovery.compute_reference_prices",
                new_callable=AsyncMock,
                return_value=items,
            ):
                resp = await client.get("/api/prices/reference/export")

        rows = list(csv.DictReader(io.StringIO(resp.text)))
        assert len(rows) == 3
        fuel_types = {r["fuel_type"] for r in rows}
        assert fuel_types == {"Methanol", "Ammonia", "LNG"}

    @pytest.mark.asyncio
    async def test_query_params_forwarded_to_compute(self):
        """Filter params are forwarded to compute_reference_prices."""
        pid = uuid4()
        dpid = uuid4()

        with patch(
            "app.routers.price_discovery.compute_reference_prices",
            new_callable=AsyncMock,
            return_value=[],
        ) as mock_compute:
            async with AsyncClient(
                transport=ASGITransport(app=app), base_url="http://test"
            ) as client:
                resp = await client.get(
                    "/api/prices/reference/export",
                    params={
                        "product_id": str(pid),
                        "delivery_point_id": str(dpid),
                        "from_date": "2026-01-01",
                        "to_date": "2026-03-01",
                    },
                )

        assert resp.status_code == 200
        call_kwargs = mock_compute.call_args.kwargs
        assert call_kwargs["product_id"] == pid
        assert call_kwargs["delivery_point_id"] == dpid
        assert call_kwargs["date_from"] == date(2026, 1, 1)
        assert call_kwargs["date_to"] == date(2026, 3, 1)

    @pytest.mark.asyncio
    async def test_delivery_point_name_none_exported_as_empty_string(self):
        """Items with no delivery point (delivery_point_name=None) export cleanly."""
        item = _make_item(delivery_point_name=None)
        async with AsyncClient(
            transport=ASGITransport(app=app), base_url="http://test"
        ) as client:
            with patch(
                "app.routers.price_discovery.compute_reference_prices",
                new_callable=AsyncMock,
                return_value=[item],
            ):
                resp = await client.get("/api/prices/reference/export")

        rows = list(csv.DictReader(io.StringIO(resp.text)))
        assert len(rows) == 1
        assert rows[0]["delivery_point_name"] == ""

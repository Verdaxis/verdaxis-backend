"""
Integration tests for the /orderbook endpoints.

Tests against a running backend (Docker or remote).
Uses seeded user data and JWT tokens for authentication.
"""
import pytest
import os
import uuid
from httpx import AsyncClient
from jose import jwt
from datetime import datetime, timedelta

TEST_API_URL = os.environ.get("TEST_API_URL", "http://localhost:8000")
JWT_SECRET = "dev-secret-key-not-for-production"

# Seeded user IDs from scripts/seed.py
SUPPLIER_1_ID = "00000000-0000-0000-0000-000000000a01"
SUPPLIER_1_EMAIL = "supplier1@verdaxis.com"
SUPPLIER_2_ID = "00000000-0000-0000-0000-000000000a02"
SUPPLIER_2_EMAIL = "supplier2@verdaxis.com"
BUYER_1_ID = "00000000-0000-0000-0000-000000000b01"
BUYER_1_EMAIL = "buyer1@verdaxis.com"
BUYER_2_ID = "00000000-0000-0000-0000-000000000b02"
BUYER_2_EMAIL = "buyer2@verdaxis.com"


def create_test_token(user_id: str, email: str, role: str) -> str:
    """Create a local HS256 token for testing."""
    payload = {
        "sub": user_id,
        "email": email,
        "role": role,
        "exp": datetime.utcnow() + timedelta(hours=1),
    }
    return jwt.encode(payload, JWT_SECRET, algorithm="HS256")


def supplier_headers(supplier_id=SUPPLIER_1_ID, email=SUPPLIER_1_EMAIL):
    token = create_test_token(supplier_id, email, "SUPPLIER")
    return {"Authorization": f"Bearer {token}"}


def buyer_headers(buyer_id=BUYER_1_ID, email=BUYER_1_EMAIL):
    token = create_test_token(buyer_id, email, "BUYER")
    return {"Authorization": f"Bearer {token}"}


# ============================================================
# Public endpoints (no auth required)
# ============================================================


class TestListOrders:
    @pytest.mark.asyncio
    async def test_list_all_orders(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            resp = await client.get("/api/orderbook")
            assert resp.status_code == 200
            data = resp.json()
            assert isinstance(data, list)

    @pytest.mark.asyncio
    async def test_list_bids_only(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            resp = await client.get("/api/orderbook/bids")
            assert resp.status_code == 200
            data = resp.json()
            assert isinstance(data, list)
            for order in data:
                assert order["side"] == "BID"
                assert order["status"] in ("OPEN", "PARTIALLY_FILLED")

    @pytest.mark.asyncio
    async def test_list_asks_only(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            resp = await client.get("/api/orderbook/asks")
            assert resp.status_code == 200
            data = resp.json()
            assert isinstance(data, list)
            for order in data:
                assert order["side"] == "ASK"
                assert order["status"] in ("OPEN", "PARTIALLY_FILLED")

    @pytest.mark.asyncio
    async def test_filter_by_region(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            resp = await client.get("/api/orderbook", params={"region": "Singapore"})
            assert resp.status_code == 200
            data = resp.json()
            for order in data:
                assert "singapore" in order["region"].lower()

    @pytest.mark.asyncio
    async def test_filter_by_fuel_type(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            resp = await client.get("/api/orderbook/asks", params={"fuel_type": "Biofuel"})
            assert resp.status_code == 200
            data = resp.json()
            for order in data:
                assert "biofuel" in order["fuel_type"].lower()

    @pytest.mark.asyncio
    async def test_filter_by_side(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            resp = await client.get("/api/orderbook", params={"side": "ASK"})
            assert resp.status_code == 200
            data = resp.json()
            for order in data:
                assert order["side"] == "ASK"

    @pytest.mark.asyncio
    async def test_order_response_shape(self):
        """Verify the response schema has all expected fields."""
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            resp = await client.get("/api/orderbook")
            assert resp.status_code == 200
            data = resp.json()
            if data:
                order = data[0]
                expected_keys = {
                    "id", "side", "fuel_type", "fuel_grade", "region",
                    "quantity_mt", "remaining_quantity_mt", "price_per_mt_usd",
                    "availability_window", "certifications", "is_verdaxis_verified",
                    "tier_label", "status", "created_at",
                }
                assert expected_keys.issubset(set(order.keys())), \
                    f"Missing keys: {expected_keys - set(order.keys())}"
                # organization_id should NOT be in public response
                assert "organization_id" not in order


class TestAggregatedAndMetadata:
    @pytest.mark.asyncio
    async def test_aggregated(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            resp = await client.get("/api/orderbook/aggregated")
            assert resp.status_code == 200
            data = resp.json()
            assert isinstance(data, list)
            if data:
                entry = data[0]
                assert "region" in entry
                assert "fuel_type" in entry
                assert "side" in entry
                assert "min_price" in entry
                assert "max_price" in entry
                assert "total_quantity" in entry
                assert "order_count" in entry

    @pytest.mark.asyncio
    async def test_regions(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            resp = await client.get("/api/orderbook/regions")
            assert resp.status_code == 200
            data = resp.json()
            assert isinstance(data, list)

    @pytest.mark.asyncio
    async def test_fuel_types(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            resp = await client.get("/api/orderbook/fuel-types")
            assert resp.status_code == 200
            data = resp.json()
            assert isinstance(data, list)


# ============================================================
# Authenticated endpoints - Create / Update / Cancel
# ============================================================


class TestCreateOrder:
    @pytest.mark.asyncio
    async def test_supplier_creates_ask_order(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            resp = await client.post(
                "/api/orderbook",
                json={
                    "side": "ASK",
                    "fuel_type": "Methanol",
                    "fuel_grade": "Green",
                    "region": "Singapore",
                    "quantity_mt": "3000",
                    "price_per_mt_usd": "560",
                    "availability_window": "Spot",
                    "certifications": ["ISCC"],
                },
                headers=supplier_headers(),
            )
            assert resp.status_code == 201, f"Unexpected: {resp.text}"
            data = resp.json()
            assert data["side"] == "ASK"
            assert data["fuel_type"] == "Methanol"
            assert data["fuel_grade"] == "Green"
            assert data["status"] == "OPEN"
            assert float(data["remaining_quantity_mt"]) == 3000
            assert data["certifications"] == ["ISCC"]

    @pytest.mark.asyncio
    async def test_buyer_creates_bid_order(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            resp = await client.post(
                "/api/orderbook",
                json={
                    "side": "BID",
                    "fuel_type": "LNG",
                    "region": "Houston",
                    "quantity_mt": "2000",
                    "price_per_mt_usd": "1200",
                },
                headers=buyer_headers(),
            )
            assert resp.status_code == 201, f"Unexpected: {resp.text}"
            data = resp.json()
            assert data["side"] == "BID"
            assert data["status"] == "OPEN"

    @pytest.mark.asyncio
    async def test_buyer_cannot_create_ask(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            resp = await client.post(
                "/api/orderbook",
                json={
                    "side": "ASK",
                    "fuel_type": "LNG",
                    "region": "Houston",
                    "quantity_mt": "1000",
                    "price_per_mt_usd": "500",
                },
                headers=buyer_headers(),
            )
            assert resp.status_code == 403

    @pytest.mark.asyncio
    async def test_supplier_cannot_create_bid(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            resp = await client.post(
                "/api/orderbook",
                json={
                    "side": "BID",
                    "fuel_type": "Methanol",
                    "region": "ARA",
                    "quantity_mt": "1000",
                    "price_per_mt_usd": "500",
                },
                headers=supplier_headers(),
            )
            assert resp.status_code == 403

    @pytest.mark.asyncio
    async def test_unauthenticated_create_rejected(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            resp = await client.post(
                "/api/orderbook",
                json={
                    "side": "BID",
                    "fuel_type": "LNG",
                    "region": "Singapore",
                    "quantity_mt": "1000",
                    "price_per_mt_usd": "100",
                },
            )
            assert resp.status_code == 401


class TestMyOrders:
    @pytest.mark.asyncio
    async def test_list_my_orders(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            # Create an order first
            await client.post(
                "/api/orderbook",
                json={
                    "side": "ASK",
                    "fuel_type": "LSMGO",
                    "region": "ARA",
                    "quantity_mt": "500",
                    "price_per_mt_usd": "620",
                },
                headers=supplier_headers(),
            )

            # List my orders
            resp = await client.get("/api/orderbook/my", headers=supplier_headers())
            assert resp.status_code == 200
            data = resp.json()
            assert isinstance(data, list)
            assert len(data) > 0

            # My orders include organization_id and trade_count
            order = data[0]
            assert "organization_id" in order
            assert "trade_count" in order
            assert "updated_at" in order

    @pytest.mark.asyncio
    async def test_unauthenticated_my_orders_rejected(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            resp = await client.get("/api/orderbook/my")
            assert resp.status_code == 401


class TestUpdateOrder:
    @pytest.mark.asyncio
    async def test_update_own_order(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            headers = supplier_headers()

            # Create
            create_resp = await client.post(
                "/api/orderbook",
                json={
                    "side": "ASK",
                    "fuel_type": "Ammonia",
                    "region": "UAE",
                    "quantity_mt": "4000",
                    "price_per_mt_usd": "900",
                },
                headers=headers,
            )
            assert create_resp.status_code == 201
            order_id = create_resp.json()["id"]

            # Update price
            update_resp = await client.put(
                f"/api/orderbook/{order_id}",
                json={"price_per_mt_usd": "880"},
                headers=headers,
            )
            assert update_resp.status_code == 200
            assert float(update_resp.json()["price_per_mt_usd"]) == 880

    @pytest.mark.asyncio
    async def test_update_quantity_recalculates_remaining(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            headers = supplier_headers()

            create_resp = await client.post(
                "/api/orderbook",
                json={
                    "side": "ASK",
                    "fuel_type": "Biofuel",
                    "region": "Algeciras",
                    "quantity_mt": "5000",
                    "price_per_mt_usd": "760",
                },
                headers=headers,
            )
            order_id = create_resp.json()["id"]

            # Increase quantity (remaining should also increase)
            update_resp = await client.put(
                f"/api/orderbook/{order_id}",
                json={"quantity_mt": "6000"},
                headers=headers,
            )
            assert update_resp.status_code == 200
            data = update_resp.json()
            assert float(data["quantity_mt"]) == 6000
            assert float(data["remaining_quantity_mt"]) == 6000

    @pytest.mark.asyncio
    async def test_cannot_update_other_orgs_order(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            # Supplier 1 creates
            create_resp = await client.post(
                "/api/orderbook",
                json={
                    "side": "ASK",
                    "fuel_type": "LNG",
                    "region": "Singapore",
                    "quantity_mt": "1000",
                    "price_per_mt_usd": "1300",
                },
                headers=supplier_headers(SUPPLIER_1_ID, SUPPLIER_1_EMAIL),
            )
            order_id = create_resp.json()["id"]

            # Supplier 2 tries to update
            update_resp = await client.put(
                f"/api/orderbook/{order_id}",
                json={"price_per_mt_usd": "100"},
                headers=supplier_headers(SUPPLIER_2_ID, SUPPLIER_2_EMAIL),
            )
            assert update_resp.status_code == 403

    @pytest.mark.asyncio
    async def test_update_nonexistent_order(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            fake_id = str(uuid.uuid4())
            resp = await client.put(
                f"/api/orderbook/{fake_id}",
                json={"price_per_mt_usd": "100"},
                headers=supplier_headers(),
            )
            assert resp.status_code == 404


class TestCancelOrder:
    @pytest.mark.asyncio
    async def test_cancel_own_order(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            headers = supplier_headers()

            # Create
            create_resp = await client.post(
                "/api/orderbook",
                json={
                    "side": "ASK",
                    "fuel_type": "Methanol",
                    "region": "ARA",
                    "quantity_mt": "1000",
                    "price_per_mt_usd": "540",
                },
                headers=headers,
            )
            order_id = create_resp.json()["id"]

            # Cancel
            cancel_resp = await client.delete(
                f"/api/orderbook/{order_id}",
                headers=headers,
            )
            assert cancel_resp.status_code == 204

            # Verify it's cancelled (should not appear in public list)
            my_orders = await client.get("/api/orderbook/my", headers=headers)
            cancelled = [o for o in my_orders.json() if o["id"] == order_id]
            if cancelled:
                assert cancelled[0]["status"] == "CANCELLED"

    @pytest.mark.asyncio
    async def test_cannot_cancel_other_orgs_order(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            # Buyer 1 creates
            create_resp = await client.post(
                "/api/orderbook",
                json={
                    "side": "BID",
                    "fuel_type": "Methanol",
                    "region": "ARA",
                    "quantity_mt": "500",
                    "price_per_mt_usd": "550",
                },
                headers=buyer_headers(BUYER_1_ID, BUYER_1_EMAIL),
            )
            order_id = create_resp.json()["id"]

            # Buyer 2 tries to cancel
            cancel_resp = await client.delete(
                f"/api/orderbook/{order_id}",
                headers=buyer_headers(BUYER_2_ID, BUYER_2_EMAIL),
            )
            assert cancel_resp.status_code == 403

    @pytest.mark.asyncio
    async def test_cancel_nonexistent_order(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            fake_id = str(uuid.uuid4())
            resp = await client.delete(
                f"/api/orderbook/{fake_id}",
                headers=supplier_headers(),
            )
            assert resp.status_code == 404

    @pytest.mark.asyncio
    async def test_cannot_cancel_already_cancelled(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            headers = supplier_headers()

            create_resp = await client.post(
                "/api/orderbook",
                json={
                    "side": "ASK",
                    "fuel_type": "LSMGO",
                    "region": "Busan",
                    "quantity_mt": "500",
                    "price_per_mt_usd": "615",
                },
                headers=headers,
            )
            order_id = create_resp.json()["id"]

            # Cancel once
            await client.delete(f"/api/orderbook/{order_id}", headers=headers)

            # Try to cancel again
            resp = await client.delete(f"/api/orderbook/{order_id}", headers=headers)
            assert resp.status_code == 400

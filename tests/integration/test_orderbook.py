"""
Integration tests for the /orderbook endpoints.

Tests against a running backend (Docker or remote).
Uses seeded user data and JWT tokens for authentication.
"""
import pytest
import os
import uuid
from httpx import AsyncClient
import jwt
from datetime import datetime, timedelta

TEST_API_URL = os.environ.get("TEST_API_URL", "http://localhost:8000")
from app.config import settings
JWT_SECRET = settings.JWT_SECRET

# Seeded user IDs from scripts/seed.py
SUPPLIER_1_ID = "9e63f7a1-0000-4000-8000-000000000011"
SUPPLIER_1_EMAIL = "itest-seller@staging.verdaxis.exchange"
SUPPLIER_2_ID = "9e63f7a1-0000-4000-8000-000000000013"
SUPPLIER_2_EMAIL = "itest-seller2@staging.verdaxis.exchange"
BUYER_1_ID = "9e63f7a1-0000-4000-8000-000000000012"
BUYER_1_EMAIL = "itest-buyer@staging.verdaxis.exchange"
BUYER_2_ID = "9e63f7a1-0000-4000-8000-000000000014"
BUYER_2_EMAIL = "itest-buyer2@staging.verdaxis.exchange"

# Live staging catalog IDs (the original LNG/MGO/biofuel/ammonia products and
# ARA/Fujairah delivery points no longer exist — remapped 2026-07-04)
PRODUCT_METHANOL_GREEN = "f9b20492-b445-59cd-b292-a386d913f488"   # e-Methanol
PRODUCT_LNG_CONV = "c4a688be-f7c2-5edc-8f93-6b34e387609c"         # Bio-Ethanol
PRODUCT_MGO_CONV = "d186bffb-766d-5944-8825-989abbdcfc46"         # Syn-Ethanol
PRODUCT_BIOFUEL_BIO = "c4a688be-f7c2-5edc-8f93-6b34e387609c"      # Bio-Ethanol
PRODUCT_AMMONIA_GREEN = "d186bffb-766d-5944-8825-989abbdcfc46"    # Syn-Ethanol
DP_SINGAPORE = "73835e92-820e-584b-8280-bb61c63aa28e"
DP_ARA = "1379d36c-1ca9-55b7-9c0d-5235a0ba1f36"                   # Rotterdam
DP_HOUSTON = "a083db06-b050-56c2-a274-3897eac2fdae"
DP_FUJAIRAH = "262d36ae-6f35-5785-b9e8-9e221b0f1b78"              # Busan


def create_test_token(user_id: str, email: str, role: str) -> str:
    """Create a local HS256 token for testing."""
    payload = {
        "sub": user_id,
        "email": email,
        "role": role,
        "exp": datetime.utcnow() + timedelta(hours=1),
        "iat": datetime.utcnow(),
    }
    return jwt.encode(payload, JWT_SECRET, algorithm="HS256")


def supplier_headers(supplier_id=SUPPLIER_1_ID, email=SUPPLIER_1_EMAIL):
    token = create_test_token(supplier_id, email, "SUPPLIER")
    return {"Authorization": f"Bearer {token}"}


def buyer_headers(buyer_id=BUYER_1_ID, email=BUYER_1_EMAIL):
    token = create_test_token(buyer_id, email, "BUYER")
    return {"Authorization": f"Bearer {token}"}


# ASK orders require an explicit certification declaration + supplier details.
ASK_REQUIRED_FIELDS = {
    "certification_declared": True,
    "certification_scheme": "ISCC EU",
    "specification_standard": "ISO 8217",
    "msds_available": True,
    "carbon_intensity_gco2_mj": "20.5",
    "feedstock": "Waste-based",
    "origin": "Singapore",
}


def ask_payload(**overrides):
    payload = {"side": "ASK", **ASK_REQUIRED_FIELDS}
    payload.update(overrides)
    return payload


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
            body = resp.json()
            assert set(body.keys()) >= {"items", "total"}
            data = body["items"]
            for order in data:
                assert order["side"] == "BID"
                assert order["status"] in ("OPEN", "PARTIALLY_FILLED")

    @pytest.mark.asyncio
    async def test_list_asks_only(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            resp = await client.get("/api/orderbook/asks")
            assert resp.status_code == 200
            body = resp.json()
            assert set(body.keys()) >= {"items", "total"}
            data = body["items"]
            for order in data:
                assert order["side"] == "ASK"
                assert order["status"] in ("OPEN", "PARTIALLY_FILLED")

    @pytest.mark.asyncio
    async def test_filter_by_product_id(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            resp = await client.get("/api/orderbook", params={"product_id": PRODUCT_METHANOL_GREEN})
            assert resp.status_code == 200
            data = resp.json()
            for order in data:
                assert order["product_id"] == PRODUCT_METHANOL_GREEN

    @pytest.mark.asyncio
    async def test_filter_by_delivery_point_id(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            resp = await client.get("/api/orderbook/asks", params={"delivery_point_id": DP_SINGAPORE})
            assert resp.status_code == 200
            data = resp.json()["items"]
            for order in data:
                assert order["delivery_point_id"] == DP_SINGAPORE

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
                    "id", "side", "product_id", "product_name",
                    "fuel_type", "fuel_grade", "region",
                    "quantity_mt", "remaining_quantity_mt", "price_per_mt_usd",
                    "availability_window", "certifications", "is_verdaxis_verified",
                    "certification_declared", "certification_scheme", "specification_standard",
                    "msds_available", "carbon_intensity_gco2_mj", "carbon_intensity_method",
                    "feedstock", "origin", "off_spec", "off_spec_notes",
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
                assert "product_id" in entry
                assert "product_name" in entry
                assert "fuel_type" in entry
                assert "region" in entry
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
                    "product_id": PRODUCT_METHANOL_GREEN,
                    "delivery_point_id": DP_SINGAPORE,
                    "quantity_mt": "3000",
                    "price_per_mt_usd": "560",
                    "availability_window": "Spot",
                    "certifications": ["ISCC"],
                    "certification_declared": True,
                    "certification_scheme": "ISCC EU",
                    "specification_standard": "IMPCA",
                    "msds_available": True,
                    "carbon_intensity_gco2_mj": "19.5",
                    "carbon_intensity_method": "ISCC EU",
                    "feedstock": "Biogenic CO2",
                    "origin": "Iceland",
                    "off_spec": True,
                    "off_spec_notes": "Water content above nominal target",
                },
                headers=supplier_headers(),
            )
            assert resp.status_code == 201, f"Unexpected: {resp.text}"
            data = resp.json()
            assert data["side"] == "ASK"
            assert data["fuel_type"] == "Methanol"
            assert data["fuel_grade"] == "E"
            assert data["product_id"] == PRODUCT_METHANOL_GREEN
            assert data["status"] == "OPEN"
            assert float(data["remaining_quantity_mt"]) == 3000
            assert data["certifications"] == ["ISCC"]
            assert data["certification_declared"] is True
            assert data["certification_scheme"] == "ISCC EU"
            assert data["specification_standard"] == "IMPCA"
            assert data["msds_available"] is True
            assert float(data["carbon_intensity_gco2_mj"]) == 19.5
            assert data["carbon_intensity_method"] == "ISCC EU"
            assert data["feedstock"] == "Biogenic CO2"
            assert data["origin"] == "Iceland"
            assert data["off_spec"] is True
            assert data["off_spec_notes"] == "Water content above nominal target"

    @pytest.mark.asyncio
    async def test_buyer_creates_bid_order(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            resp = await client.post(
                "/api/orderbook",
                json={
                    "side": "BID",
                    "product_id": PRODUCT_LNG_CONV,
                    "delivery_point_id": DP_HOUSTON,
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
                    "product_id": PRODUCT_LNG_CONV,
                    "delivery_point_id": DP_HOUSTON,
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
                    "product_id": PRODUCT_METHANOL_GREEN,
                    "delivery_point_id": DP_ARA,
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
                    "product_id": PRODUCT_LNG_CONV,
                    "delivery_point_id": DP_SINGAPORE,
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
                json=ask_payload(
                    product_id=PRODUCT_MGO_CONV,
                    delivery_point_id=DP_ARA,
                    quantity_mt="500",
                    price_per_mt_usd="620",
                ),
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
                json=ask_payload(
                    product_id=PRODUCT_AMMONIA_GREEN,
                    delivery_point_id=DP_FUJAIRAH,
                    quantity_mt="4000",
                    price_per_mt_usd="900",
                ),
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
                json=ask_payload(
                    product_id=PRODUCT_BIOFUEL_BIO,
                    delivery_point_id=DP_ARA,
                    quantity_mt="5000",
                    price_per_mt_usd="760",
                ),
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
                json=ask_payload(
                    product_id=PRODUCT_LNG_CONV,
                    delivery_point_id=DP_SINGAPORE,
                    quantity_mt="1000",
                    price_per_mt_usd="1300",
                ),
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
                json=ask_payload(
                    product_id=PRODUCT_METHANOL_GREEN,
                    delivery_point_id=DP_ARA,
                    quantity_mt="1000",
                    price_per_mt_usd="540",
                ),
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
                    "product_id": PRODUCT_METHANOL_GREEN,
                    "delivery_point_id": DP_ARA,
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
                json=ask_payload(
                    product_id=PRODUCT_MGO_CONV,
                    delivery_point_id=DP_ARA,
                    quantity_mt="500",
                    price_per_mt_usd="615",
                ),
                headers=headers,
            )
            order_id = create_resp.json()["id"]

            # Cancel once
            await client.delete(f"/api/orderbook/{order_id}", headers=headers)

            # Try to cancel again
            resp = await client.delete(f"/api/orderbook/{order_id}", headers=headers)
            assert resp.status_code == 400

"""
Integration tests for the /trades endpoints.

Tests the full trade lifecycle: create -> confirm -> deliver -> pay,
plus decline, role-based access, and edge cases.

Tests against a running backend (Docker or remote).
"""
import pytest
import os
import uuid
from httpx import AsyncClient
import jwt
from datetime import datetime, timedelta

from tests.runtime_config import resolve_test_api_url

TEST_API_URL = resolve_test_api_url(os.environ, require_mutation_opt_in=True)
from app.config import settings
JWT_SECRET = settings.JWT_SECRET

# Dedicated integration-test users (seeded in the staging DB, NOT demo-market
# orgs — demo-org listings cannot be traded, which silently breaks the whole
# lifecycle suite if reused here).
SUPPLIER_1_ID = "9e63f7a1-0000-4000-8000-000000000011"
SUPPLIER_1_EMAIL = "itest-seller@staging.verdaxis.exchange"
SUPPLIER_2_ID = "9e63f7a1-0000-4000-8000-000000000013"
SUPPLIER_2_EMAIL = "itest-seller2@staging.verdaxis.exchange"
BUYER_1_ID = "9e63f7a1-0000-4000-8000-000000000012"
BUYER_1_EMAIL = "itest-buyer@staging.verdaxis.exchange"
BUYER_2_ID = "9e63f7a1-0000-4000-8000-000000000012"
BUYER_2_EMAIL = "itest-buyer@staging.verdaxis.exchange"

# Deterministic product/delivery point IDs from catalog_seed.py
PRODUCT_METHANOL_GREEN = "b0f9b249-1ae4-5e02-adf5-e4964788ad8e"
PRODUCT_E_METHANOL = "f9b20492-b445-59cd-b292-a386d913f488"
PRODUCT_BIO_ETHANOL = "c4a688be-f7c2-5edc-8f93-6b34e387609c"
PRODUCT_SYN_ETHANOL = "d186bffb-766d-5944-8825-989abbdcfc46"
DP_SINGAPORE = "73835e92-820e-584b-8280-bb61c63aa28e"
DP_ARA = "0f6b6006-61ef-5ef9-b096-71bf87d1d3d7"
DP_HOUSTON = "a083db06-b050-56c2-a274-3897eac2fdae"
DP_FUJAIRAH = "f4877150-d88e-5825-b154-3410dfc9f1f1"


def create_test_token(user_id: str, email: str, role: str) -> str:
    payload = {
        "sub": user_id,
        "email": email,
        "role": role,
        "exp": datetime.utcnow() + timedelta(hours=1),
        "iat": datetime.utcnow(),
    }
    return jwt.encode(payload, JWT_SECRET, algorithm="HS256")


def supplier_headers(sid=SUPPLIER_1_ID, email=SUPPLIER_1_EMAIL):
    return {"Authorization": f"Bearer {create_test_token(sid, email, 'SUPPLIER')}"}


def buyer_headers(bid=BUYER_1_ID, email=BUYER_1_EMAIL):
    return {"Authorization": f"Bearer {create_test_token(bid, email, 'BUYER')}"}


async def create_ask_order(client: AsyncClient, headers=None, **overrides) -> dict:
    """Helper: create an ASK order and return the response dict."""
    payload = {
        "side": "ASK",
        "product_id": PRODUCT_METHANOL_GREEN,
        "delivery_point_id": DP_SINGAPORE,
        "quantity_mt": "5000",
        "price_per_mt_usd": "560",
        "certification_declared": True,
        "certification_scheme": "ISCC EU",
        "specification_standard": "ISO 8217",
        "msds_available": True,
        "carbon_intensity_gco2_mj": "20.5",
        "feedstock": "Waste-based",
        "origin": "Singapore",
        **overrides,
    }
    resp = await client.post("/api/orderbook", json=payload, headers=headers or supplier_headers())
    assert resp.status_code == 201, f"Failed to create ASK: {resp.text}"
    return resp.json()


async def create_bid_order(client: AsyncClient, headers=None, **overrides) -> dict:
    """Helper: create a BID order and return the response dict."""
    payload = {
        "side": "BID",
        "product_id": PRODUCT_E_METHANOL,
        "delivery_point_id": DP_HOUSTON,
        "quantity_mt": "2000",
        "price_per_mt_usd": "1200",
        **overrides,
    }
    resp = await client.post("/api/orderbook", json=payload, headers=headers or buyer_headers())
    assert resp.status_code == 201, f"Failed to create BID: {resp.text}"
    return resp.json()


async def hit_order(client: AsyncClient, order_id: str, quantity: str, headers=None) -> dict:
    """Helper: hit an order to create a trade."""
    resp = await client.post(
        "/api/trades/",
        json={"order_id": order_id, "quantity_mt": quantity},
        headers=headers,
    )
    return resp


# ============================================================
# Trade creation (hitting orders)
# ============================================================


class TestCreateTrade:
    @pytest.mark.asyncio
    async def test_buyer_hits_ask_order(self):
        """Buyer hits a supplier's ASK order -> trade created as PENDING_CONFIRMATION."""
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            ask = await create_ask_order(client)
            resp = await hit_order(client, ask["id"], "1000", buyer_headers())

            assert resp.status_code == 200, f"Unexpected: {resp.text}"
            trade = resp.json()
            assert trade["status"] == "PENDING_CONFIRMATION"
            assert trade["initiated_by"] == "BUYER"
            assert trade["ask_order_id"] == ask["id"]
            assert trade["bid_order_id"] is None
            assert float(trade["quantity_mt"]) == 1000
            assert float(trade["price_per_mt_usd"]) == 560
            assert trade["buyer_name"] != ""
            assert trade["seller_name"] != ""
            assert trade["fuel_type"] == "Methanol"
            assert trade["product_name"] == "Methanol Green"

    @pytest.mark.asyncio
    async def test_seller_hits_bid_order(self):
        """Supplier hits a buyer's BID order -> trade created as PENDING_CONFIRMATION."""
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            bid = await create_bid_order(client)
            resp = await hit_order(client, bid["id"], "500", supplier_headers())

            assert resp.status_code == 200, f"Unexpected: {resp.text}"
            trade = resp.json()
            assert trade["status"] == "PENDING_CONFIRMATION"
            assert trade["initiated_by"] == "SELLER"
            assert trade["bid_order_id"] == bid["id"]
            assert trade["ask_order_id"] is None

    @pytest.mark.asyncio
    async def test_order_remaining_quantity_decreases(self):
        """Hitting an order should decrease remaining_quantity_mt."""
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            ask = await create_ask_order(client, quantity_mt="3000")
            await hit_order(client, ask["id"], "1000", buyer_headers())

            # Check the order's remaining quantity via my orders
            my_orders = await client.get("/api/orderbook/my", headers=supplier_headers())
            order = next(o for o in my_orders.json() if o["id"] == ask["id"])
            assert float(order["remaining_quantity_mt"]) == 2000
            assert order["status"] == "PARTIALLY_FILLED"

    @pytest.mark.asyncio
    async def test_order_fully_filled(self):
        """Hitting for the full quantity should set status to FILLED."""
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            ask = await create_ask_order(client, quantity_mt="1000")
            await hit_order(client, ask["id"], "1000", buyer_headers())

            my_orders = await client.get("/api/orderbook/my", headers=supplier_headers())
            order = next(o for o in my_orders.json() if o["id"] == ask["id"])
            assert float(order["remaining_quantity_mt"]) == 0
            assert order["status"] == "FILLED"

    @pytest.mark.asyncio
    async def test_cannot_exceed_remaining_quantity(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            ask = await create_ask_order(client, quantity_mt="1000")
            resp = await hit_order(client, ask["id"], "1500", buyer_headers())
            assert resp.status_code == 400
            assert "exceeds" in resp.json()["detail"].lower()

    @pytest.mark.asyncio
    async def test_cannot_self_trade(self):
        """Supplier cannot hit their own ASK order."""
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            ask = await create_ask_order(client, headers=supplier_headers())
            # Same supplier tries to hit (but they're SUPPLIER role, so they'd
            # need to hit a BID as a seller -- hitting ASK requires BUYER role)
            resp = await hit_order(client, ask["id"], "100", supplier_headers())
            assert resp.status_code == 403

    @pytest.mark.asyncio
    async def test_buyer_cannot_hit_bid(self):
        """Buyers cannot hit BID orders (only suppliers can)."""
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            bid = await create_bid_order(client, headers=buyer_headers())
            resp = await hit_order(client, bid["id"], "500", buyer_headers(BUYER_2_ID, BUYER_2_EMAIL))
            assert resp.status_code == 403

    @pytest.mark.asyncio
    async def test_supplier_cannot_hit_ask(self):
        """Suppliers cannot hit ASK orders (only buyers can)."""
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            ask = await create_ask_order(client, headers=supplier_headers())
            resp = await hit_order(client, ask["id"], "500", supplier_headers(SUPPLIER_2_ID, SUPPLIER_2_EMAIL))
            assert resp.status_code == 403

    @pytest.mark.asyncio
    async def test_cannot_hit_cancelled_order(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            ask = await create_ask_order(client)
            # Cancel the order
            await client.delete(f"/api/orderbook/{ask['id']}", headers=supplier_headers())
            # Try to hit
            resp = await hit_order(client, ask["id"], "500", buyer_headers())
            assert resp.status_code == 400

    @pytest.mark.asyncio
    async def test_hit_nonexistent_order(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            fake_id = str(uuid.uuid4())
            resp = await hit_order(client, fake_id, "100", buyer_headers())
            assert resp.status_code == 404

    @pytest.mark.asyncio
    async def test_unauthenticated_trade_rejected(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            resp = await client.post(
                "/api/trades/",
                json={"order_id": str(uuid.uuid4()), "quantity_mt": "100"},
            )
            assert resp.status_code == 401


# ============================================================
# List my trades
# ============================================================


class TestListMyTrades:
    @pytest.mark.asyncio
    async def test_list_trades_as_buyer(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            # Create an order and a trade
            ask = await create_ask_order(client)
            await hit_order(client, ask["id"], "500", buyer_headers())

            resp = await client.get("/api/trades/my", headers=buyer_headers())
            assert resp.status_code == 200
            body = resp.json()
            assert isinstance(body["items"], list)
            assert body["total"] > 0

    @pytest.mark.asyncio
    async def test_list_trades_as_seller(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            ask = await create_ask_order(client)
            await hit_order(client, ask["id"], "500", buyer_headers())

            resp = await client.get("/api/trades/my", headers=supplier_headers())
            assert resp.status_code == 200
            body = resp.json()
            assert body["total"] > 0

    @pytest.mark.asyncio
    async def test_trade_response_shape(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            ask = await create_ask_order(client)
            await hit_order(client, ask["id"], "500", buyer_headers())

            resp = await client.get("/api/trades/my", headers=buyer_headers())
            trade = resp.json()["items"][0]
            expected_keys = {
                "id", "buyer_id", "seller_id", "buyer_name", "seller_name",
                "initiated_by", "quantity_mt", "price_per_mt_usd", "status",
                "created_at", "fuel_type", "region", "product_id", "product_name",
            }
            assert expected_keys.issubset(set(trade.keys())), \
                f"Missing keys: {expected_keys - set(trade.keys())}"

    @pytest.mark.asyncio
    async def test_unauthenticated_my_trades_rejected(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            resp = await client.get("/api/trades/my")
            assert resp.status_code == 401


# ============================================================
# Confirm / Decline
# ============================================================


class TestConfirmTrade:
    @pytest.mark.asyncio
    async def test_counterparty_confirms(self):
        """Seller (counterparty) confirms a buyer-initiated trade."""
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            ask = await create_ask_order(client)
            trade_resp = await hit_order(client, ask["id"], "500", buyer_headers())
            trade_id = trade_resp.json()["id"]

            # Seller confirms
            resp = await client.put(
                f"/api/trades/{trade_id}/confirm",
                headers=supplier_headers(),
            )
            assert resp.status_code == 200, f"Unexpected: {resp.text}"
            data = resp.json()
            assert data["status"] == "CONFIRMED"
            assert data["confirmed_at"] is not None

    @pytest.mark.asyncio
    async def test_initiator_cannot_confirm(self):
        """Buyer (initiator) cannot confirm their own trade."""
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            ask = await create_ask_order(client)
            trade_resp = await hit_order(client, ask["id"], "500", buyer_headers())
            trade_id = trade_resp.json()["id"]

            # Buyer (initiator) tries to confirm
            resp = await client.put(
                f"/api/trades/{trade_id}/confirm",
                headers=buyer_headers(),
            )
            assert resp.status_code == 403

    @pytest.mark.asyncio
    async def test_cannot_confirm_non_pending_trade(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            ask = await create_ask_order(client)
            trade_resp = await hit_order(client, ask["id"], "500", buyer_headers())
            trade_id = trade_resp.json()["id"]

            # Confirm once
            await client.put(f"/api/trades/{trade_id}/confirm", headers=supplier_headers())

            # Try to confirm again
            resp = await client.put(f"/api/trades/{trade_id}/confirm", headers=supplier_headers())
            assert resp.status_code == 400


class TestDeclineTrade:
    @pytest.mark.asyncio
    async def test_counterparty_declines(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            ask = await create_ask_order(client, quantity_mt="2000")
            trade_resp = await hit_order(client, ask["id"], "1000", buyer_headers())
            trade_id = trade_resp.json()["id"]

            # Seller declines
            resp = await client.put(
                f"/api/trades/{trade_id}/decline",
                headers=supplier_headers(),
            )
            assert resp.status_code == 200
            assert resp.json()["status"] == "DECLINED"

    @pytest.mark.asyncio
    async def test_decline_restores_order_quantity(self):
        """Declining a trade should restore the order's remaining quantity."""
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            ask = await create_ask_order(client, quantity_mt="2000")
            trade_resp = await hit_order(client, ask["id"], "1000", buyer_headers())
            trade_id = trade_resp.json()["id"]

            # Verify remaining dropped to 1000
            my_orders = await client.get("/api/orderbook/my", headers=supplier_headers())
            order = next(o for o in my_orders.json() if o["id"] == ask["id"])
            assert float(order["remaining_quantity_mt"]) == 1000

            # Decline
            await client.put(f"/api/trades/{trade_id}/decline", headers=supplier_headers())

            # Verify remaining restored to 2000
            my_orders = await client.get("/api/orderbook/my", headers=supplier_headers())
            order = next(o for o in my_orders.json() if o["id"] == ask["id"])
            assert float(order["remaining_quantity_mt"]) == 2000
            assert order["status"] == "OPEN"

    @pytest.mark.asyncio
    async def test_initiator_cannot_decline(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            ask = await create_ask_order(client)
            trade_resp = await hit_order(client, ask["id"], "500", buyer_headers())
            trade_id = trade_resp.json()["id"]

            # Buyer (initiator) tries to decline
            resp = await client.put(
                f"/api/trades/{trade_id}/decline",
                headers=buyer_headers(),
            )
            assert resp.status_code == 403


# ============================================================
# Deliver
# ============================================================


class TestDeliverTrade:
    @pytest.mark.asyncio
    async def test_deliver_with_final_details(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            # Setup: create -> hit -> confirm
            ask = await create_ask_order(client)
            trade_resp = await hit_order(client, ask["id"], "1000", buyer_headers())
            trade_id = trade_resp.json()["id"]
            await client.put(f"/api/trades/{trade_id}/confirm", headers=supplier_headers())

            # Deliver
            resp = await client.put(
                f"/api/trades/{trade_id}/deliver",
                json={
                    "final_quantity_mt": "995",
                    "final_price_per_mt": "558",
                },
                headers=supplier_headers(),
            )
            assert resp.status_code == 200, f"Unexpected: {resp.text}"
            data = resp.json()
            assert data["status"] == "DELIVERED"
            assert float(data["final_quantity_mt"]) == 995
            assert float(data["final_price_per_mt"]) == 558
            # 995 * 558 = 555,210
            assert float(data["final_total_usd"]) == 555210
            assert data["delivered_at"] is not None
            # Commission = 555210 * 0.5 / 100 = 2776.05
            assert float(data["commission_amount_usd"]) == pytest.approx(2776.05, rel=0.01)

    @pytest.mark.asyncio
    async def test_cannot_deliver_unconfirmed_trade(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            ask = await create_ask_order(client)
            trade_resp = await hit_order(client, ask["id"], "500", buyer_headers())
            trade_id = trade_resp.json()["id"]

            # Try to deliver without confirming first
            resp = await client.put(
                f"/api/trades/{trade_id}/deliver",
                json={"final_quantity_mt": "500", "final_price_per_mt": "560"},
                headers=supplier_headers(),
            )
            assert resp.status_code == 400

    @pytest.mark.asyncio
    async def test_unauthorized_party_cannot_deliver(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            ask = await create_ask_order(client)
            trade_resp = await hit_order(client, ask["id"], "500", buyer_headers())
            trade_id = trade_resp.json()["id"]
            await client.put(f"/api/trades/{trade_id}/confirm", headers=supplier_headers())

            # A different supplier tries to deliver
            resp = await client.put(
                f"/api/trades/{trade_id}/deliver",
                json={"final_quantity_mt": "500", "final_price_per_mt": "560"},
                headers=supplier_headers(SUPPLIER_2_ID, SUPPLIER_2_EMAIL),
            )
            assert resp.status_code == 403

    @pytest.mark.asyncio
    async def test_final_price_outside_band_rejected(self):
        """final_price_per_mt more than 10% away from the confirmed trade
        price must be rejected (commission/GMV integrity)."""
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            ask = await create_ask_order(client)  # confirmed price 560
            trade_resp = await hit_order(client, ask["id"], "1000", buyer_headers())
            trade_id = trade_resp.json()["id"]
            await client.put(f"/api/trades/{trade_id}/confirm", headers=supplier_headers())

            for bad_price in ("700", "0.01", "503.99"):  # +25%, ~-100%, just past -10%
                resp = await client.put(
                    f"/api/trades/{trade_id}/deliver",
                    json={"final_quantity_mt": "1000", "final_price_per_mt": bad_price},
                    headers=supplier_headers(),
                )
                assert resp.status_code == 400, f"price {bad_price}: {resp.text}"
                assert "confirmed trade price" in resp.json()["detail"]

            # Trade must still be deliverable at a legitimate price afterwards
            resp = await client.put(
                f"/api/trades/{trade_id}/deliver",
                json={"final_quantity_mt": "1000", "final_price_per_mt": "560"},
                headers=supplier_headers(),
            )
            assert resp.status_code == 200, resp.text

    @pytest.mark.asyncio
    async def test_final_price_at_band_edge_allowed(self):
        """Exactly 10% deviation is inside the allowed band."""
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            ask = await create_ask_order(client)  # confirmed price 560
            trade_resp = await hit_order(client, ask["id"], "1000", buyer_headers())
            trade_id = trade_resp.json()["id"]
            await client.put(f"/api/trades/{trade_id}/confirm", headers=supplier_headers())

            resp = await client.put(
                f"/api/trades/{trade_id}/deliver",
                json={"final_quantity_mt": "1000", "final_price_per_mt": "616"},  # 560 * 1.10
                headers=supplier_headers(),
            )
            assert resp.status_code == 200, resp.text
            assert float(resp.json()["final_price_per_mt"]) == 616


# ============================================================
# Pay
# ============================================================


class TestPayTrade:
    @pytest.mark.asyncio
    async def test_seller_marks_paid(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            # Full lifecycle: create -> hit -> confirm -> deliver -> pay
            ask = await create_ask_order(client)
            trade_resp = await hit_order(client, ask["id"], "1000", buyer_headers())
            trade_id = trade_resp.json()["id"]
            await client.put(f"/api/trades/{trade_id}/confirm", headers=supplier_headers())
            await client.put(
                f"/api/trades/{trade_id}/deliver",
                json={"final_quantity_mt": "1000", "final_price_per_mt": "560"},
                headers=supplier_headers(),
            )

            # Pay
            resp = await client.post(
                f"/api/trades/{trade_id}/pay",
                headers=supplier_headers(),
            )
            assert resp.status_code == 200, f"Unexpected: {resp.text}"
            data = resp.json()
            assert data["status"] == "PAID"
            assert data["paid_at"] is not None

    @pytest.mark.asyncio
    async def test_buyer_cannot_mark_paid(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            ask = await create_ask_order(client)
            trade_resp = await hit_order(client, ask["id"], "1000", buyer_headers())
            trade_id = trade_resp.json()["id"]
            await client.put(f"/api/trades/{trade_id}/confirm", headers=supplier_headers())
            await client.put(
                f"/api/trades/{trade_id}/deliver",
                json={"final_quantity_mt": "1000", "final_price_per_mt": "560"},
                headers=buyer_headers(),
            )

            # Buyer tries to mark as paid
            resp = await client.post(
                f"/api/trades/{trade_id}/pay",
                headers=buyer_headers(),
            )
            assert resp.status_code == 403

    @pytest.mark.asyncio
    async def test_cannot_pay_undelivered_trade(self):
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            ask = await create_ask_order(client)
            trade_resp = await hit_order(client, ask["id"], "500", buyer_headers())
            trade_id = trade_resp.json()["id"]
            await client.put(f"/api/trades/{trade_id}/confirm", headers=supplier_headers())

            # Try to pay without delivering
            resp = await client.post(
                f"/api/trades/{trade_id}/pay",
                headers=supplier_headers(),
            )
            assert resp.status_code == 400


# ============================================================
# Full lifecycle test
# ============================================================


class TestFullTradeLifecycle:
    @pytest.mark.asyncio
    async def test_full_buyer_initiated_lifecycle(self):
        """
        End-to-end: Supplier posts ASK -> Buyer hits it -> Seller confirms ->
        Seller delivers -> Seller marks paid.
        """
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            # 1. Supplier creates ASK
            ask = await create_ask_order(
                client,
                product_id=PRODUCT_BIO_ETHANOL,
                delivery_point_id=DP_ARA,
                quantity_mt="10000",
                price_per_mt_usd="780",
            )
            assert ask["status"] == "OPEN"
            assert ask["side"] == "ASK"

            # 2. Buyer hits the ASK
            trade_resp = await hit_order(client, ask["id"], "3000", buyer_headers())
            assert trade_resp.status_code == 200
            trade = trade_resp.json()
            trade_id = trade["id"]
            assert trade["status"] == "PENDING_CONFIRMATION"
            assert trade["initiated_by"] == "BUYER"
            assert float(trade["quantity_mt"]) == 3000

            # 3. Verify order is partially filled
            my_orders = await client.get("/api/orderbook/my", headers=supplier_headers())
            order = next(o for o in my_orders.json() if o["id"] == ask["id"])
            assert order["status"] == "PARTIALLY_FILLED"
            assert float(order["remaining_quantity_mt"]) == 7000

            # 4. Seller confirms
            confirm_resp = await client.put(
                f"/api/trades/{trade_id}/confirm",
                headers=supplier_headers(),
            )
            assert confirm_resp.status_code == 200
            assert confirm_resp.json()["status"] == "CONFIRMED"

            # 5. Seller delivers
            deliver_resp = await client.put(
                f"/api/trades/{trade_id}/deliver",
                json={"final_quantity_mt": "2980", "final_price_per_mt": "782"},
                headers=supplier_headers(),
            )
            assert deliver_resp.status_code == 200
            delivered = deliver_resp.json()
            assert delivered["status"] == "DELIVERED"
            assert float(delivered["final_total_usd"]) == 2980 * 782

            # 6. Seller marks paid
            pay_resp = await client.post(
                f"/api/trades/{trade_id}/pay",
                headers=supplier_headers(),
            )
            assert pay_resp.status_code == 200
            assert pay_resp.json()["status"] == "PAID"

    @pytest.mark.asyncio
    async def test_full_seller_initiated_lifecycle(self):
        """
        End-to-end: Buyer posts BID -> Supplier hits it -> Buyer confirms ->
        Buyer delivers -> Seller marks paid.
        """
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            # 1. Buyer creates BID
            bid = await create_bid_order(
                client,
                product_id=PRODUCT_SYN_ETHANOL,
                delivery_point_id=DP_FUJAIRAH,
                quantity_mt="5000",
                price_per_mt_usd="850",
            )
            assert bid["side"] == "BID"

            # 2. Supplier hits the BID
            trade_resp = await hit_order(client, bid["id"], "2000", supplier_headers())
            assert trade_resp.status_code == 200
            trade = trade_resp.json()
            trade_id = trade["id"]
            assert trade["initiated_by"] == "SELLER"

            # 3. Buyer confirms
            confirm_resp = await client.put(
                f"/api/trades/{trade_id}/confirm",
                headers=buyer_headers(),
            )
            assert confirm_resp.status_code == 200

            # 4. Buyer delivers
            deliver_resp = await client.put(
                f"/api/trades/{trade_id}/deliver",
                json={"final_quantity_mt": "2000", "final_price_per_mt": "850"},
                headers=buyer_headers(),
            )
            assert deliver_resp.status_code == 200
            assert deliver_resp.json()["status"] == "DELIVERED"

            # 5. Seller marks paid
            pay_resp = await client.post(
                f"/api/trades/{trade_id}/pay",
                headers=supplier_headers(),
            )
            assert pay_resp.status_code == 200
            assert pay_resp.json()["status"] == "PAID"

    @pytest.mark.asyncio
    async def test_partial_fills_multiple_trades(self):
        """
        Multiple buyers can hit the same ASK order until it's fully filled.
        """
        async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
            ask = await create_ask_order(client, quantity_mt="3000")

            # Buyer 1 takes 1000
            resp1 = await hit_order(client, ask["id"], "1000", buyer_headers())
            assert resp1.status_code == 200

            # Buyer 2 takes 1500
            resp2 = await hit_order(client, ask["id"], "1500", buyer_headers(BUYER_2_ID, BUYER_2_EMAIL))
            assert resp2.status_code == 200

            # Check remaining = 500
            my_orders = await client.get("/api/orderbook/my", headers=supplier_headers())
            order = next(o for o in my_orders.json() if o["id"] == ask["id"])
            assert float(order["remaining_quantity_mt"]) == 500
            assert order["status"] == "PARTIALLY_FILLED"

            # Buyer 1 takes the last 500
            resp3 = await hit_order(client, ask["id"], "500", buyer_headers())
            assert resp3.status_code == 200

            # Now fully filled
            my_orders = await client.get("/api/orderbook/my", headers=supplier_headers())
            order = next(o for o in my_orders.json() if o["id"] == ask["id"])
            assert float(order["remaining_quantity_mt"]) == 0
            assert order["status"] == "FILLED"

            # Cannot hit anymore
            resp4 = await hit_order(client, ask["id"], "100", buyer_headers())
            assert resp4.status_code == 400

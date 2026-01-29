import pytest
from httpx import AsyncClient
from uuid import UUID
import os
from jose import jwt
from datetime import datetime, timedelta

# Fixtures and helpers
TEST_API_URL = os.environ.get("TEST_API_URL", "http://localhost:8000")
JWT_SECRET = "dev-secret-key-not-for-production"

def create_test_token(email: str, role: str) -> str:
    """Create a local HS256 token for testing."""
    payload = {
        "sub": email,
        "email": email,
        "role": role,
        "exp": datetime.utcnow() + timedelta(hours=1)
    }
    return jwt.encode(payload, JWT_SECRET, algorithm="HS256")

@pytest.mark.asyncio
async def test_inventory_publish_flow():
    """
    Test the flow: Create inventory -> Publish -> Verify Listing.
    """
    async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
        # 1. Login/Get Token for Supplier
        # Using a seeded supplier from seed.py
        supplier_email = "sales@globalenergy.com"
        token = create_test_token(supplier_email, "SUPPLIER")
        headers = {"Authorization": f"Bearer {token}"}

        # 2. Add Inventory Item
        inventory_data = {
            "port_id": "SGSIN",
            "fuel_type": "Biofuel",
            "product_name": "Test Sustainable Fuel",
            "current_stock_mt": 1000.0,
            "price_per_mt_usd": 850.0,
            "is_certified": True
        }
        resp = await client.post("/api/inventory", json=inventory_data, headers=headers)
        assert resp.status_code == 200, f"Failed to add inventory: {resp.text}"
        item = resp.json()
        item_id = item["id"]

        # 3. Publish Inventory Item
        publish_resp = await client.post(f"/api/inventory/{item_id}/publish", headers=headers)
        assert publish_resp.status_code == 200, f"Failed to publish inventory: {publish_resp.text}"

        # 4. Verify Public Listing exists
        # Get all listings (public endpoint)
        listings_resp = await client.get("/api/listings")
        assert listings_resp.status_code == 200
        listings = listings_resp.json()
        
        # Find our new listing
        found = any(l["fuel_type"] == "Biofuel" and float(l["quantity_mt"]) == 1000.0 for l in listings)
        assert found, "Published listing not found in public listings"

        # 5. Verify Supplier's My Listings
        my_listings_resp = await client.get("/api/listings/my", headers=headers)
        assert my_listings_resp.status_code == 200
        my_listings = my_listings_resp.json()
        
        # Verify match_count is present (even if 0)
        found_mine = next((l for l in my_listings if l["id"] == l["id"]), None) # Dummy find
        assert "match_count" in my_listings[0], "match_count missing from My Listings response"

@pytest.mark.asyncio
async def test_rfq_list_optimized():
    """
    Test that RFQ list endpoints return organization names (optimized queries).
    """
    async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
        # Use seeded buyer
        buyer_email = "manager@pacificoceanlines.com"
        token = create_test_token(buyer_email, "BUYER")
        headers = {"Authorization": f"Bearer {token}"}

        # Get my RFQ requests
        resp = await client.get("/api/rfq/my-requests", headers=headers)
        assert resp.status_code == 200
        matches = resp.json()
        
        if matches:
            match = matches[0]
            assert "supplier_name" in match
            assert "buyer_name" in match
            assert match["supplier_name"] != "Unknown"
            assert match["buyer_name"] != "Unknown"
        else:
            print("No RFQs found for seeded buyer, skipping detailed name check")

@pytest.mark.asyncio
async def test_unauthorized_publish():
    """Ensure buyers cannot publish inventory."""
    async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as client:
        buyer_email = "manager@pacificoceanlines.com"
        token = create_test_token(buyer_email, "BUYER")
        headers = {"Authorization": f"Bearer {token}"}
        
        import uuid
        fake_id = str(uuid.uuid4())
        resp = await client.post(f"/api/inventory/{fake_id}/publish", headers=headers)
        assert resp.status_code == 403

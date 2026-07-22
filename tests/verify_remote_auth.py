
import asyncio
import httpx
import os
import sys
from pathlib import Path
from uuid import uuid4

ROOT = str(Path(__file__).resolve().parents[1])
if ROOT not in sys.path:
    sys.path.insert(0, ROOT)
from tests.disposable_target import attest, from_environment  # noqa: E402


BASE_URL = ""

# User Data - Randomize to avoid collisions on repeated runs
SUPPLIER_EMAIL = f"remote_sup_{uuid4()}@disposable.invalid"
SUPPLIER_PASSWORD = "password123"
ADMIN_EMAIL_TEST = "itest-admin@disposable.invalid"

async def test_auth_flow():
    global BASE_URL
    target = from_environment()
    attest(target)
    admin_password = os.environ.get("DISPOSABLE_ITEST_PASSWORD", "")
    if not admin_password:
        raise RuntimeError("DISPOSABLE_ITEST_PASSWORD is required")
    BASE_URL = target.base_url
    async with httpx.AsyncClient(base_url=BASE_URL) as client:
        print(f"--- 1. Registering Supplier: {SUPPLIER_EMAIL} ---")
        response = await client.post("/api/auth/register", json={
            "email": SUPPLIER_EMAIL,
            "password": SUPPLIER_PASSWORD,
            "role": "SUPPLIER",
            "first_name": "Test",
            "last_name": "Supplier"
        })
        if response.status_code != 200:
            print(f"Registration Failed: {response.text}")
            return
        
        supplier_id = response.json()["id"]
        print(f"Supplier Registered. ID: {supplier_id}")

        print("\n--- 2. Attempting Login (Should fail with PENDING) ---")
        response = await client.post("/api/auth/login", json={
            "email": SUPPLIER_EMAIL,
            "password": SUPPLIER_PASSWORD
        })
        if response.status_code == 403:
            print(f"Success: Login blocked as expected. Detail: {response.json()['detail']}")
        else:
            print(f"Failure: Login response {response.status_code} - {response.text}")
            # return # Don't return, allow trying to fix

        print("\n--- 3. Logging in as Admin (Seeded) ---")
        # Login as admin
        response = await client.post("/api/auth/login", json={
            "email": ADMIN_EMAIL_TEST,
            "password": admin_password
        })
        if response.status_code != 200:
            print(f"Admin Login Failed: {response.text}")
            return
        
        admin_token = response.json()["access_token"]
        print("Admin Logged In.")

        print(f"\n--- 4. Approving Supplier: {supplier_id} ---")
        response = await client.put(
            f"/api/auth/approve/{supplier_id}",
            headers={"Authorization": f"Bearer {admin_token}"}
        )
        if response.status_code == 200:
            print(f"Supplier Approved: {response.json()['status']}")
        else:
            print(f"Approve Failed: {response.text}")
            return

        print("\n--- 5. Attempting Supplier Login (Should Success) ---")
        response = await client.post("/api/auth/login", json={
            "email": SUPPLIER_EMAIL,
            "password": SUPPLIER_PASSWORD
        })
        if response.status_code == 200:
            print("Success: Supplier Logged In!")
        else:
            print(f"Failure: Login response {response.status_code} - {response.text}")

if __name__ == "__main__":
    asyncio.run(test_auth_flow())

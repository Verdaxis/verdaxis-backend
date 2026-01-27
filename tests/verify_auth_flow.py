
import asyncio
import httpx
from uuid import uuid4

BASE_URL = "http://localhost:8000"

# User Data
SUPPLIER_EMAIL = f"supplier_{uuid4()}@test.com"
SUPPLIER_PASSWORD = "password123"
ADMIN_EMAIL = "admin@verdaxis.com" # Assuming this user exists or we need to creaet it differently? 
# Actually, for test simplicity, I'll rely on the seed data or create an admin if not exists.
# But wait, I don't know the admin credentials.
# I'll create a new ADMIN user directly in DB or use a known one if I can find one. 
# Let's assume I can register an ADMIN for testing purposes if I modify the code temporarily, 
# OR I can just manually insert an admin into DB. 
# Better: I'll register a user with ADMIN role first (since my register endpoint allows passing role).

# Seeded Admin
ADMIN_EMAIL_TEST = "admin@verdaxis.com"
ADMIN_PASSWORD = "admin123" 

async def test_auth_flow():
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
            "password": ADMIN_PASSWORD
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

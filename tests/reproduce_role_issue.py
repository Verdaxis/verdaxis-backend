import pytest
from fastapi.testclient import TestClient
from app.main import app
from app.schemas.user import UserRole
import uuid

client = TestClient(app)

def test_register_buyer_role():
    email = f"buyer_{uuid.uuid4()}@example.com"
    password = "password123"
    role = "BUYER"
    
    response = client.post("/api/auth/register", json={
        "email": email,
        "password": password,
        "first_name": "Test",
        "last_name": "Buyer",
        "role": role
    })
    
    assert response.status_code == 200, f"Registration failed: {response.text}"
    data = response.json()
    print(f"Registered user: {data}")
    
    assert data["role"] == role, f"Expected role {role}, got {data['role']}"

def test_register_supplier_role():
    email = f"supplier_{uuid.uuid4()}@example.com"
    password = "password123"
    role = "SUPPLIER"
    
    response = client.post("/api/auth/register", json={
        "email": email,
        "password": password,
        "first_name": "Test",
        "last_name": "Supplier",
        "role": role
    })
    
    assert response.status_code == 200, f"Registration failed: {response.text}"
    data = response.json()
    print(f"Registered user: {data}")
    
    assert data["role"] == role, f"Expected role {role}, got {data['role']}"

if __name__ == "__main__":
    # Manually run the tests if executed as a script
    try:
        test_register_buyer_role()
        print("Buyer registration test passed!")
        test_register_supplier_role()
        print("Supplier registration test passed!")
    except AssertionError as e:
        print(f"Test failed: {e}")
    except Exception as e:
        print(f"An error occurred: {e}")

"""
Integration tests for API endpoints.
Tests the full request/response cycle against the running Docker backend.

Run with: pytest tests/integration/ -v
Requires: Docker backend running on localhost:8000
"""
import pytest
from httpx import AsyncClient


# Skip all tests if backend is not running
pytestmark = pytest.mark.asyncio


async def is_backend_running(client: AsyncClient) -> bool:
    """Check if the backend is reachable."""
    try:
        response = await client.get("/health")
        return response.status_code == 200
    except Exception:
        return False


class TestHealthEndpoints:
    """Tests for health check endpoints."""
    
    async def test_root_endpoint(self, client: AsyncClient):
        """Root endpoint should return API info."""
        response = await client.get("/")
        
        assert response.status_code == 200
        data = response.json()
        assert "message" in data
        assert "Verdaxis" in data["message"]
    
    async def test_health_endpoint(self, client: AsyncClient):
        """Health endpoint should return ok status."""
        response = await client.get("/health")
        
        assert response.status_code == 200
        data = response.json()
        assert data["status"] == "ok"


class TestAuthEndpoints:
    """Tests for authentication endpoints."""
    
    async def test_register_new_user(self, client: AsyncClient, sample_user_data):
        """Should successfully register a new user."""
        response = await client.post("/api/auth/register", json=sample_user_data)
        
        assert response.status_code == 200
        data = response.json()
        assert data["status"] in ["created", "requires_org"]
        if data["status"] == "created":
            user_data = data["user"]
            assert user_data["email"] == sample_user_data["email"]
            assert user_data["status"] == "PENDING"
        else:
            assert "registration_token" in data
    
    async def test_login_pending_user(self, client: AsyncClient, sample_user_data):
        """Should reject login for pending (non-approved) users."""
        # Register user (will be PENDING)
        await client.post("/api/auth/register", json=sample_user_data)
        
        # Try to login
        response = await client.post("/api/auth/login", data={
            "username": sample_user_data["email"],
            "password": sample_user_data["password"]
        })
        
        assert response.status_code == 403
        # Email verification now gates login before the approval check; either
        # message proves an unapproved account cannot log in.
        detail = response.json()["detail"].lower()
        assert "pending" in detail or "verify your email" in detail
    
    async def test_login_wrong_password(self, client: AsyncClient, admin_credentials):
        """Should reject login with wrong password."""
        response = await client.post("/api/auth/login", data={
            "username": admin_credentials["email"],
            "password": "wrongpassword"
        })
        
        assert response.status_code == 401
    
    async def test_login_nonexistent_user(self, client: AsyncClient):
        """Should reject login for non-existent user."""
        response = await client.post("/api/auth/login", data={
            "username": "nonexistent@example.com",
            "password": "anypassword"
        })
        
        assert response.status_code == 401
    
    async def test_login_approved_admin(self, client: AsyncClient, admin_credentials):
        """Should allow login for approved admin user."""
        response = await client.post("/api/auth/login", data={
            "username": admin_credentials["email"],
            "password": admin_credentials["password"]
        })
        
        assert response.status_code == 200
        data = response.json()
        assert "access_token" in data
        assert data["token_type"] == "bearer"


class TestProtectedEndpoints:
    """Tests for endpoints that require authentication."""
    
    async def test_me_without_token(self, client: AsyncClient):
        """Should reject access to /auth/me without token."""
        response = await client.get("/api/auth/me")
        
        assert response.status_code == 401
    
    async def test_me_with_invalid_token(self, client: AsyncClient):
        """Should reject access with invalid token."""
        response = await client.get(
            "/api/auth/me",
            headers={"Authorization": "Bearer invalid.token.here"}
        )
        
        assert response.status_code == 401
    
    async def test_me_with_valid_token(self, client: AsyncClient, admin_credentials):
        """Should return user info with valid token."""
        # Login first
        login_response = await client.post("/api/auth/login", data={
            "username": admin_credentials["email"],
            "password": admin_credentials["password"]
        })
        assert login_response.status_code == 200
        token = login_response.json()["access_token"]
        
        # Access protected endpoint
        response = await client.get(
            "/api/auth/me",
            headers={"Authorization": f"Bearer {token}"}
        )
        
        assert response.status_code == 200
        data = response.json()
        assert data["email"] == admin_credentials["email"]


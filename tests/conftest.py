"""
Pytest configuration and fixtures for Verdaxis backend tests.

For integration tests, we test against the running Docker backend.
Unit tests use mocks and don't require the database.
"""
import pytest
import asyncio
import os
from typing import Generator
from httpx import AsyncClient


# Test against local Docker instance or remote
TEST_API_URL = os.environ.get("TEST_API_URL", "http://localhost:8000")


@pytest.fixture(scope="session")
def event_loop() -> Generator[asyncio.AbstractEventLoop, None, None]:
    """Create an instance of the default event loop for the test session."""
    loop = asyncio.get_event_loop_policy().new_event_loop()
    yield loop
    loop.close()


@pytest.fixture
async def client() -> AsyncClient:
    """
    Create an async HTTP client for testing API endpoints.
    Tests against the running backend (Docker or remote).
    """
    async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as ac:
        yield ac


@pytest.fixture
def sample_user_data():
    """Sample user data for testing registration."""
    import uuid
    unique_id = str(uuid.uuid4())[:8]
    return {
        "email": f"test_{unique_id}@verdaxis.com",
        "password": "securepassword123",
        "first_name": "Test",
        "last_name": "User",
        "role": "BUYER"
    }


@pytest.fixture
def admin_user_data():
    """Admin user data for testing admin functions."""
    return {
        "email": "admin@test.com",
        "password": "adminpassword123",
        "first_name": "Admin",
        "last_name": "User",
        "role": "ADMIN"
    }


@pytest.fixture
def admin_credentials():
    """Seeded admin credentials for the test database."""
    return {
        "email": "admin@verdaxis.com",
        "password": "***REMOVED***"
    }


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
    from tests.runtime_config import resolve_test_api_url

    test_api_url = resolve_test_api_url(os.environ)
    async with AsyncClient(base_url=test_api_url, timeout=10.0) as ac:
        yield ac


@pytest.fixture
def sample_user_data():
    """Sample user data for testing registration.

    Uses the itest org's domain so registration attaches the user to an
    existing organization instead of branching into the requires_org flow.
    """
    import uuid
    unique_id = str(uuid.uuid4())[:8]
    return {
        "email": f"test_{unique_id}@itest.staging.verdaxis.exchange",
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
def admin_credentials(itest_password):
    """Dedicated staging itest admin (the old seeded admin password was
    scrubbed from history and is unrecoverable)."""
    return {
        "email": "itest-admin@staging.verdaxis.exchange",
        "password": itest_password,
    }



@pytest.fixture(scope="session")
def itest_password() -> str:
    """Password for the dedicated itest-* staging users.

    Prefers the ITEST_PASSWORD env var; falls back to the secrets file kept
    next to the repos on the staging VPS (see PILOT-RUNBOOK §9).
    """
    pw = os.environ.get("ITEST_PASSWORD")
    if pw:
        return pw
    secret_path = os.path.join(os.path.dirname(__file__), "..", "..", "..", ".staging-itest-password")
    try:
        with open(secret_path) as fh:
            return fh.read().strip()
    except OSError:
        pytest.skip("No itest password available (set ITEST_PASSWORD or provision the secrets file)")

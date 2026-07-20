"""
Pytest configuration and fixtures for Verdaxis backend tests.

For integration tests, we test against an explicitly selected local disposable,
staging, or other read-only API target.
Unit tests use mocks and don't require the database.
"""
import pytest
import asyncio
import os
from typing import Generator
from httpx import AsyncClient

# Tests must opt into an isolated JWT namespace. Production/staging startup
# fails closed when these environment-bound values are absent.
os.environ.setdefault("ENVIRONMENT", "test")
os.environ.setdefault("RELEASE_SHA", "test")
os.environ.setdefault("JWT_SECRET", "test-secret-key-for-testing-minimum-32-chars")
os.environ.setdefault("JWT_ISSUER", "verdaxis-test-api")
os.environ.setdefault("JWT_AUDIENCE", "verdaxis-test-web")

# Black-box integration tests target an explicitly supplied running service.
# Never guess localhost: that turns a missing external harness into hundreds
# of misleading connection/setup failures during the self-contained suite.
TEST_API_URL = os.environ.get("TEST_API_URL", "").strip()


def pytest_collection_modifyitems(config, items):
    if TEST_API_URL:
        return
    skip_external = pytest.mark.skip(
        reason="TEST_API_URL is not configured for black-box integration tests"
    )
    for item in items:
        if "/tests/integration/" in str(item.path):
            item.add_marker(skip_external)


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
    Tests against the explicitly configured running backend.
    """
    if not TEST_API_URL:
        pytest.skip("TEST_API_URL is not configured")
    async with AsyncClient(base_url=TEST_API_URL, timeout=10.0) as ac:
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

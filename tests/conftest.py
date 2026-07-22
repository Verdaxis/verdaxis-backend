"""Pytest configuration and fixtures for Verdaxis backend tests.

Integration/E2E suites run only against an explicitly attested disposable
server (tests/disposable_target.py). Unit tests use mocks/SQLite and never
require a running API.
"""
import asyncio
import os
from pathlib import Path
from typing import Generator

import pytest
from httpx import AsyncClient

# Tests must opt into an isolated JWT namespace. Production/staging startup
# fails closed when these environment-bound values are absent.
os.environ.setdefault("ENVIRONMENT", "test")
os.environ.setdefault("RELEASE_SHA", "test")
os.environ.setdefault("JWT_SECRET", "test-secret-key-for-testing-minimum-32-chars")
os.environ.setdefault("JWT_ISSUER", "verdaxis-test-api")
os.environ.setdefault("JWT_AUDIENCE", "verdaxis-test-web")

from tests.disposable_target import (  # noqa: E402
    DisposableTargetError,
    attest,
    requires_disposable_target,
    validate_target,
)


def pytest_addoption(parser):
    group = parser.getgroup("verdaxis disposable integration")
    group.addoption(
        "--run-disposable-integration",
        action="store_true",
        help="run integration/E2E tests against an attested disposable server",
    )
    group.addoption("--disposable-target-url")
    group.addoption("--disposable-target-token")


def pytest_configure(config):
    config.addinivalue_line(
        "markers",
        "disposable_integration: mutating integration/E2E test requiring an attested disposable server",
    )
    if not config.getoption("--run-disposable-integration"):
        return
    try:
        target = validate_target(
            config.getoption("--disposable-target-url"),
            config.getoption("--disposable-target-token"),
        )
    except DisposableTargetError as exc:
        raise pytest.UsageError(str(exc)) from exc
    config._verdaxis_disposable_target = target
    os.environ["TEST_API_URL"] = target.base_url


def pytest_collection_modifyitems(config, items):
    guarded = [item for item in items if requires_disposable_target(Path(str(item.path)))]
    for item in guarded:
        item.add_marker(pytest.mark.disposable_integration)
    if not guarded:
        return
    if not config.getoption("--run-disposable-integration"):
        reason = "requires --run-disposable-integration and an attested disposable target"
        for item in guarded:
            item.add_marker(pytest.mark.skip(reason=reason))
        return
    target = getattr(config, "_verdaxis_disposable_target", None)
    if target is None:
        raise pytest.UsageError("explicit disposable target configuration is required")
    try:
        attest(target)
    except DisposableTargetError as exc:
        raise pytest.UsageError(str(exc)) from exc


@pytest.fixture(scope="session")
def event_loop() -> Generator[asyncio.AbstractEventLoop, None, None]:
    """Create an instance of the default event loop for the test session."""
    loop = asyncio.get_event_loop_policy().new_event_loop()
    yield loop
    loop.close()


@pytest.fixture
async def client(request) -> AsyncClient:
    """
    Create an async HTTP client for testing API endpoints.
    Tests only against the already-attested disposable backend.
    """
    target = getattr(request.config, "_verdaxis_disposable_target", None)
    if target is None:
        pytest.fail("integration client requested without an attested disposable target")
    async with AsyncClient(base_url=target.base_url, timeout=10.0) as ac:
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
        "email": f"test_{unique_id}@disposable.invalid",
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
        "email": "itest-admin@disposable.invalid",
        "password": itest_password,
    }


@pytest.fixture(scope="session")
def itest_password() -> str:
    """Password explicitly provisioned into the disposable test server."""
    pw = os.environ.get("DISPOSABLE_ITEST_PASSWORD")
    if pw:
        return pw
    pytest.skip("set DISPOSABLE_ITEST_PASSWORD for the disposable server")

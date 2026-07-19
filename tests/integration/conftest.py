"""Shared fail-closed policy for integration tests calling a running API."""

from __future__ import annotations

import os

import pytest
from httpx import AsyncClient

from tests.runtime_config import resolve_test_api_url


@pytest.fixture(scope="session")
def test_api_url() -> str:
    # The integration directory contains mutating flows. A remote target must
    # therefore be explicitly acknowledged even when selecting one test.
    return resolve_test_api_url(os.environ, require_mutation_opt_in=True)


@pytest.fixture
async def client(test_api_url: str) -> AsyncClient:
    async with AsyncClient(base_url=test_api_url, timeout=10.0) as ac:
        yield ac

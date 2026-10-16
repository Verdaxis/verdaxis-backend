"""Verify transport vectors against the local Python ASGI oracle, without SQL."""
import json

import pytest

from scripts.rust_contract.export_mcp_http import ROOT, vectors


@pytest.mark.asyncio
async def test_http_vectors_match_python():
    assert [item async for item in vectors()] == json.loads(
        (ROOT / "deploy/rust/mcp-http-vectors.json").read_text()
    )

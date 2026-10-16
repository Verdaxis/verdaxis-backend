"""Protocol schema and rejections remain tied to the pinned Python SDK."""
import json

from mcp_types.jsonrpc import jsonrpc_message_adapter
from mcp_types.version import MODERN_PROTOCOL_VERSIONS

from scripts.rust_contract.export_mcp_protocol import ROOT, vectors


def test_protocol_vectors_match_python():
    assert json.loads((ROOT / "deploy/rust/mcp-protocol-vectors.json").read_text()) == list(vectors())
    assert json.loads((ROOT / "deploy/rust/mcp-protocol-schema.json").read_text()) == {
        "schema": jsonrpc_message_adapter.json_schema(),
        "modern_versions": list(MODERN_PROTOCOL_VERSIONS),
    }

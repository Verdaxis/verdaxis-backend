"""Keep the MCP input-domain vectors tied to the unchanged Python oracle."""
import json

from scripts.rust_contract.export_mcp_validation import ROOT, vectors


def test_validation_vectors_match_python():
    stored = json.loads((ROOT / "deploy/rust/mcp-validation-vectors.json").read_text())
    assert list(vectors()) == stored

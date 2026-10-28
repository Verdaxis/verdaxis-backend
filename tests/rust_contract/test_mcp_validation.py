"""Keep the MCP input-domain vectors tied to the unchanged Python oracle."""
import json

from scripts.rust_contract.export_mcp_validation import ROOT, vectors


def test_validation_vectors_match_python():
    stored = json.loads((ROOT / "deploy/rust/mcp-validation-vectors.json").read_text())
    assert list(vectors()) == stored


def test_unicode_decimal_ranges_match_python():
    import unicodedata
    stored = json.loads((ROOT / "deploy/rust/python-decimal-digits.json").read_text())
    assert stored["unicode_version"] == unicodedata.unidata_version
    assert stored["zero_codepoints"] == [
        code for code in range(0x110000)
        if unicodedata.category(chr(code)) == "Nd" and unicodedata.decimal(chr(code)) == 0
    ]

"""The MCP privacy gate must reject private values under any field name."""
import pytest

from scripts.rust_contract.mcp_acceptance import privacy


@pytest.mark.parametrize("value", [
    {"note": "Organization private-org-id supplied this price"},
    {"id": "private-org-id"},
    ["private-org-id"],
    '{"note":"private-org-id"}',
    {"private-org-id": "renamed field"},
    {"nested": {"note": "private-certificate-number"}},
])
def test_private_values_are_rejected(value):
    with pytest.raises(AssertionError, match="private value"):
        privacy(value, {"private-org-id", "private-certificate-number"})


def test_public_values_and_keys_are_preserved():
    privacy({"market_product": "BIO_METHANOL", "port": "Singapore"}, {"private-org-id"})


def test_private_field_names_remain_rejected():
    with pytest.raises(AssertionError, match="private field"):
        privacy({"organization_id": None}, set())
